//! Pushdown classification, pruning, partitioning and construction (R10, R11).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

// --- R10: pushdown classification -------------------------------------------------------------

fn classify_all(p: &EventStoreTableProvider, exprs: &[Expr]) -> Vec<Verdict> {
    let refs: Vec<&Expr> = exprs.iter().collect();
    p.supports_filters_pushdown(&refs).unwrap()
}

#[test]
fn provider_reports_inexact_for_every_consumed_predicate() {
    let fx = fixture();
    let p = provider(&fx);
    let consumed = vec![
        col("collection_time").gt_eq(lit(5_i64)),
        col("collection_time").gt(lit(5_i64)),
        col("collection_time").lt(lit(5_i64)),
        col("collection_time").lt_eq(lit(5_i64)),
        col("collection_time").eq(lit(5_i64)),
        lit(5_i64).lt(col("collection_time")),
        col("pid").eq(lit(7_u64)),
        col("ppid").eq(lit(7_u64)),
        col("name").eq(lit("bash")),
        col("executable_hash").eq(lit(HASH_A)),
        col("pid").in_list(vec![lit(1_u64), lit(2_u64)], false),
        col("name").in_list(vec![lit("a"), lit("b")], false),
        lit("bash").eq(col("name")),
    ];
    let got = classify_all(&p, &consumed);
    assert_eq!(got, vec![Verdict::Inexact; consumed.len()]);
}

#[test]
fn provider_reports_unsupported_for_everything_it_cannot_consume() {
    let fx = fixture();
    let p = provider(&fx);
    let refused = vec![
        col("command_line").eq(lit("x")),
        col("pid").gt(lit(7_u64)),
        col("pid").not_eq(lit(7_u64)),
        col("name").in_list(vec![lit("a")], true),
        col("pid").eq(col("ppid")),
        col("name").like(lit("b%")),
        col("pid").eq(lit(1_u64)).or(col("pid").eq(lit(2_u64))),
        col("executable_hash").eq(lit("not-hex")),
        col("pid").eq(lit(u64::MAX)),
        col("collection_time").eq(lit("soon")),
        col("pid").in_list(vec![lit(1_u64), col("ppid")], false),
        lit(true),
    ];
    let got = classify_all(&p, &refused);
    assert_eq!(got, vec![Verdict::Unsupported; refused.len()]);
}

fn arb_leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        proptest::sample::select(vec![
            "pid",
            "ppid",
            "name",
            "executable_hash",
            "collection_time",
            "command_line",
            "nonexistent",
        ])
        .prop_map(col),
        any::<i64>().prop_map(lit),
        any::<u64>().prop_map(lit),
        "[a-f0-9]{0,70}".prop_map(lit),
        Just(lit(ScalarValue::Null)),
    ]
}

fn arb_operator() -> impl Strategy<Value = Operator> {
    proptest::sample::select(vec![
        Operator::Eq,
        Operator::NotEq,
        Operator::Lt,
        Operator::LtEq,
        Operator::Gt,
        Operator::GtEq,
        Operator::And,
        Operator::Or,
        Operator::Plus,
        Operator::LikeMatch,
        Operator::IsDistinctFrom,
    ])
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    arb_leaf().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            (inner.clone(), arb_operator(), inner.clone()).prop_map(|(l, op, r)| {
                Expr::BinaryExpr(BinaryExpr::new(Box::new(l), op, Box::new(r)))
            }),
            (
                inner.clone(),
                proptest::collection::vec(inner.clone(), 0..4),
                any::<bool>()
            )
                .prop_map(|(e, list, negated)| e.in_list(list, negated)),
            inner.prop_map(|e| !e),
        ]
    })
}

proptest! {
    /// Half of a pair: no generated expression is ever `Exact`. The other half is
    /// `provider_reports_inexact_for_every_consumed_predicate`, which fails if the provider
    /// answers `Unsupported` for everything.
    #[test]
    fn provider_never_reports_exact(exprs in proptest::collection::vec(arb_expr(), 1..8)) {
        let fx = fixture();
        let p = provider(&fx);
        let got = classify_all(&p, &exprs);
        prop_assert_eq!(got.len(), exprs.len());
        prop_assert!(got.iter().all(|d| *d != Verdict::Exact));
    }
}

// --- R11: pruning and partitioning ------------------------------------------------------------

#[tokio::test]
async fn provider_prunes_buckets_before_the_time_filter() {
    let fx = fixture();
    put_all(
        &fx,
        (10..=12)
            .map(|b| (b * HOUR + 5, record(b * HOUR + 5, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let filter = col("collection_time").gt_eq(lit(ms(11 * HOUR)));
    let plan = plan_for(&p, None, &[filter]).await;
    assert_eq!(runs_of(&plan).concat(), vec![11, 12]);
}

#[tokio::test]
async fn provider_prunes_buckets_past_an_upper_bound() {
    let fx = fixture();
    put_all(
        &fx,
        (10..=12)
            .map(|b| (b * HOUR + 5, record(b * HOUR + 5, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let filter = col("collection_time").lt(lit(ms(11 * HOUR)));
    let plan = plan_for(&p, None, &[filter]).await;
    assert_eq!(runs_of(&plan).concat(), vec![10]);
}

#[tokio::test]
async fn provider_groups_twenty_buckets_into_four_contiguous_disjoint_runs() {
    let fx = fixture();
    put_all(
        &fx,
        (0..20)
            .map(|b| (b * HOUR, record(b * HOUR, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let plan = plan_for(&p, None, &[]).await;
    let runs = runs_of(&plan);
    assert_eq!(runs.len(), 4);
    assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), vec![5; 4]);
    assert_eq!(runs.concat(), (0..20).collect::<Vec<u64>>());
    assert_eq!(plan.properties().output_partitioning().partition_count(), 4);
}

#[tokio::test]
async fn provider_never_emits_more_partitions_than_buckets_or_target() {
    let fx = fixture();
    put_all(
        &fx,
        (0..5)
            .map(|b| (b * HOUR, record(b * HOUR, 1, "bash")))
            .collect(),
    );
    let p = provider(&fx);
    let runs = runs_of(&plan_for(&p, None, &[]).await);
    assert_eq!(
        runs.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![2, 1, 1, 1]
    );

    let single = provider_with(
        &fx,
        ScanLimits {
            target_partitions: 1,
            ..ScanLimits::default()
        },
    );
    let one_run = runs_of(&plan_for(&single, None, &[]).await);
    assert_eq!(one_run, vec![vec![0, 1, 2, 3, 4]]);
}

#[tokio::test]
async fn provider_emits_one_partition_for_a_one_bucket_window() {
    let fx = fixture();
    put_all(&fx, vec![(5 * HOUR, record(5 * HOUR, 1, "bash"))]);
    let p = provider(&fx);
    let plan = plan_for(&p, None, &[]).await;
    assert_eq!(plan.properties().output_partitioning().partition_count(), 1);
}

#[tokio::test]
async fn provider_returns_empty_exec_when_no_bucket_survives() {
    let fx = fixture();
    let p = provider(&fx);
    let empty_store = plan_for(&p, None, &[]).await;
    assert!(empty_store.downcast_ref::<EmptyExec>().is_some());

    put_all(&fx, vec![(5 * HOUR, record(5 * HOUR, 1, "bash"))]);
    let far = col("collection_time").gt_eq(lit(ms(900 * HOUR)));
    let pruned = plan_for(&p, None, &[far]).await;
    assert!(pruned.downcast_ref::<EmptyExec>().is_some());
}

// --- construction -----------------------------------------------------------------------------

#[test]
fn provider_new_rejects_zero_limits_and_unbackable_descriptors() {
    let fx = fixture();
    for limits in [
        ScanLimits {
            target_partitions: 0,
            ..ScanLimits::default()
        },
        ScanLimits {
            batch_size: 0,
            ..ScanLimits::default()
        },
        ScanLimits {
            batch_max_bytes: 0,
            ..ScanLimits::default()
        },
    ] {
        let err = EventStoreTableProvider::new(
            Arc::clone(&fx.store),
            Arc::clone(&fx.cache),
            &process_table(),
            limits,
        )
        .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidLimit(_)));
    }
    let mut table = process_table();
    table
        .columns
        .push(column("no_such_column", ColumnType::Int, true));
    let err = EventStoreTableProvider::new(
        Arc::clone(&fx.store),
        Arc::clone(&fx.cache),
        &table,
        ScanLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(err, ProviderError::Schema(_)));
}
