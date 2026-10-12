//! A predicate means the same thing whether the planner pushed it or left it residual (KTD5).
//!
//! `DataFusion` orders floats totally (NaN equals NaN and sorts above every number) and treats a
//! backslash in `LIKE` as an escape; the conformance reference does neither. Both are corrected
//! in `derive.rs`, and these tests drive the *residual* half of a rule, which the conformance
//! corpus (it builds `Predicate` protos) cannot reach. Every residual case is paired with the
//! same predicate pushed, and with a finite or matching row that must still be admitted.
#![cfg(feature = "detection-engine")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use daemoneye_lib::detection::catalog::{SchemaCatalog, verify_spawn_token};
use daemoneye_lib::detection::execution::derive::{CycleWindow, derive};
use daemoneye_lib::detection::execution::session::{ExecutorRuntime, LatencySink, session_state};
use daemoneye_lib::detection::{RegexCache, plan_rule};
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor, TableDescriptor,
};
use datafusion::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::datasource::MemTable;
use datafusion::execution::context::SessionContext;

const WIDE: CycleWindow = CycleWindow {
    after_ms: 0,
    through_ms: 1_000_000,
};
const FLOAT_OPS: [PredicateOp; 7] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::In,
];

/// `(pid, name, cpu_usage)`.
type Row = (u64, &'static str, f64);

fn table(rows: &[Row]) -> MemTable {
    let schema = Arc::new(Schema::new(vec![
        Field::new("pid", DataType::UInt64, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("cpu_usage", DataType::Float64, false),
        Field::new("collection_time", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.0))),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.1))),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|r| r.2))),
            Arc::new(Int64Array::from_iter_values(rows.iter().map(|_| 10))),
        ],
    )
    .unwrap();
    MemTable::try_new(schema, vec![vec![batch]]).unwrap()
}

fn column(name: &str, column_type: ColumnType, ops: &[PredicateOp]) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(column_type),
        nullable: false,
        supported_ops: ops.iter().copied().map(i32::from).collect(),
    }
}

/// A catalog whose `cpu_usage` and `name` advertise `ops` (every one conformance-passed), so
/// `ops` decides what the planner pushes and what stays residual.
fn catalog(float_ops: &[PredicateOp], name_ops: &[PredicateOp]) -> SchemaCatalog {
    let token = "a".repeat(64);
    let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
    let columns = vec![
        column("pid", ColumnType::Uint, &[]),
        column("name", ColumnType::String, name_ops),
        column("cpu_usage", ColumnType::Float, float_ops),
    ];
    let mut catalog = SchemaCatalog::new();
    catalog
        .register(
            &verified,
            SchemaDescriptor {
                collector_id: "procmond".to_owned(),
                descriptor_version: "v1".to_owned(),
                tables: vec![TableDescriptor {
                    name: "processes".to_owned(),
                    columns: columns.clone(),
                }],
                conformance_results: Vec::new(),
            },
        )
        .unwrap();
    for c in &columns {
        for op in &c.supported_ops {
            catalog.record_conformance_pass(
                "procmond",
                "processes",
                &c.name,
                PredicateOp::try_from(*op).unwrap(),
            );
        }
    }
    catalog
}

/// The pids a rule's `WHERE` admits over `rows`, and whether it ended up pushed or residual.
async fn run(catalog: &SchemaCatalog, predicate: &str, rows: &[Row]) -> (Vec<u64>, Half) {
    let rule = DetectionRule::new(
        "rule-1".to_owned(),
        "Parity".to_owned(),
        "Parity rule".to_owned(),
        format!("SELECT pid FROM processes WHERE {predicate}"),
        "test".to_owned(),
        AlertSeverity::Low,
    );
    let compiled = plan_rule(catalog, &RegexCache::new(), &rule, 3).unwrap();
    let half = Half {
        pushed: !compiled.plan().predicates.is_empty(),
        residual: compiled.residual().is_some(),
    };
    let state = session_state(
        ExecutorRuntime::new().unwrap().env(),
        Arc::new(LatencySink::default()),
        Arc::new(RegexCache::new()),
    )
    .unwrap();
    let ctx = SessionContext::new_with_state(state);
    let frame = derive(&ctx, Arc::new(table(rows)), &compiled, WIDE, 100).unwrap();
    let mut pids = Vec::new();
    for batch in frame.collect().await.unwrap() {
        let array = batch
            .column_by_name("pid")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .clone();
        pids.extend(array.iter().flatten());
    }
    pids.sort_unstable();
    (pids, half)
}

#[derive(Debug, PartialEq, Eq)]
struct Half {
    pushed: bool,
    residual: bool,
}

const PUSHED: Half = Half {
    pushed: true,
    residual: false,
};
const RESIDUAL: Half = Half {
    pushed: false,
    residual: true,
};

/// One predicate planned both ways must admit `expected`, and each plan must really be the half
/// it claims to be.
async fn assert_halves_agree(sql: &str, rows: &[Row], expected: &[u64]) {
    let (pushed, pushed_half) = run(&catalog(&FLOAT_OPS, &[PredicateOp::Like]), sql, rows).await;
    let (residual, residual_half) = run(&catalog(&[], &[]), sql, rows).await;
    assert_eq!(
        pushed_half, PUSHED,
        "the pushed catalog must push the predicate"
    );
    assert_eq!(
        residual_half, RESIDUAL,
        "the bare catalog must leave it residual"
    );
    assert_eq!(pushed, expected, "pushed half diverges from the reference");
    assert_eq!(
        residual, expected,
        "residual half diverges from the reference"
    );
}

const CPU_ROWS: [Row; 3] = [(1, "a", 95.0), (2, "b", f64::NAN), (3, "c", 10.0)];

#[tokio::test]
async fn residual_nan_is_unknown_under_a_comparison_like_the_pushed_half() {
    assert_halves_agree("cpu_usage > 90", &CPU_ROWS, &[1]).await;
    assert_halves_agree("cpu_usage >= 10", &CPU_ROWS, &[1, 3]).await;
    assert_halves_agree("cpu_usage <> 5", &CPU_ROWS, &[1, 3]).await;
    assert_halves_agree("cpu_usage < 100", &CPU_ROWS, &[1, 3]).await;
}

#[tokio::test]
async fn residual_nan_is_unknown_under_in() {
    assert_halves_agree("cpu_usage IN (95.0, 10.0)", &CPU_ROWS, &[1, 3]).await;
}

#[tokio::test]
async fn residual_nan_is_unknown_under_between() {
    // `BETWEEN` plans to its own node, not a `BinaryExpr`; under a total float order NaN would
    // fall outside every range and `NOT BETWEEN` would admit it.
    let (inside, half) = run(&catalog(&[], &[]), "cpu_usage BETWEEN 0 AND 50", &CPU_ROWS).await;
    assert_eq!(half, RESIDUAL);
    assert_eq!(inside, vec![3]);
    let (outside, _) = run(
        &catalog(&[], &[]),
        "cpu_usage NOT BETWEEN 0 AND 50",
        &CPU_ROWS,
    )
    .await;
    assert_eq!(
        outside,
        vec![1],
        "NaN is UNKNOWN, so NOT BETWEEN does not admit it"
    );
}

#[tokio::test]
async fn residual_nan_stays_unknown_beneath_not_and_or() {
    let (negated, half) = run(&catalog(&[], &[]), "NOT (cpu_usage > 90)", &CPU_ROWS).await;
    assert_eq!(half, RESIDUAL);
    assert_eq!(negated, vec![3], "NOT UNKNOWN is UNKNOWN, not TRUE");

    let (either, _) = run(&catalog(&[], &[]), "cpu_usage > 90 OR pid = 3", &CPU_ROWS).await;
    assert_eq!(either, vec![1, 3]);
}

#[tokio::test]
async fn residual_nan_is_unknown_inside_arithmetic() {
    let (pids, half) = run(&catalog(&[], &[]), "cpu_usage + 1 > 50", &CPU_ROWS).await;
    assert_eq!(half, RESIDUAL);
    assert_eq!(pids, vec![1]);
}

const NAME_ROWS: [Row; 4] = [
    (1, r"a\_b", 0.0),
    (2, "a_b", 0.0),
    (3, r"a\Xb", 0.0),
    (4, "aXb", 0.0),
];

/// The reference has no escape: `\` is a backslash and `_` still matches one character.
#[tokio::test]
async fn residual_like_treats_a_backslash_as_a_literal_like_the_pushed_half() {
    assert_halves_agree(r"name LIKE 'a\_b'", &NAME_ROWS, &[1, 3]).await;
    assert_halves_agree(
        r"name LIKE 'a\\_b'",
        &[(1, r"a\\_b", 0.0), (2, r"a\_b", 0.0)],
        &[1],
    )
    .await;
}

#[tokio::test]
async fn residual_like_without_a_backslash_is_unchanged() {
    assert_halves_agree("name LIKE 'a_b'", &NAME_ROWS, &[2, 4]).await;
    let (negated, half) = run(&catalog(&[], &[]), r"name NOT LIKE 'a\_b'", &NAME_ROWS).await;
    assert_eq!(half, RESIDUAL);
    assert_eq!(negated, vec![2, 4]);
}
