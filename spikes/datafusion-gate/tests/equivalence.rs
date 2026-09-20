//! U4/U5/U6 verification.
//!
//! R18 is the gate on every number this spike produces: a measurement counts
//! only when both arms return the same result set. A partition-pruning bug that
//! drops rows looks like a fast result rather than a failure, so these tests
//! are what make the measurements trustworthy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use daemoneye_lib::storage::EventStore;
use datafusion_gate::datafusion_arm::{SessionSettings, build_context, lineage_sql, run};
use datafusion_gate::fixture::{FixtureSpec, HOUR_MS};
use datafusion_gate::provider::EventTable;
use datafusion_gate::{control, fixture};
use std::sync::Arc;
use tempfile::TempDir;

const SPEC: FixtureSpec = FixtureSpec {
    rows: 6_000,
    span_hours: 25,
    planted: 40,
    start_ms: 1_767_225_600_000,
};

/// One fixture plus a single shared handle.
///
/// redb holds an exclusive file lock and `EventStore::open` always opens
/// read-write, so a second handle on the same path fails with
/// `DatabaseAlreadyOpen`. Both arms share one handle.
struct Fx {
    _dir: TempDir,
    store: Arc<EventStore>,
    start: u64,
    end: u64,
}

fn fx() -> Fx {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("f.redb");
    let stats = fixture::generate(&path, SPEC).unwrap();
    let store = Arc::new(EventStore::open(&path).unwrap());
    Fx {
        _dir: dir,
        store,
        start: stats.start_ms,
        end: SPEC.start_ms + SPEC.span_hours * HOUR_MS,
    }
}

#[tokio::test]
async fn both_arms_return_the_same_match_set() {
    let f = fx();
    let control = control::run(&f.store, f.start, f.end).unwrap();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let df = run(&ctx, f.start.cast_signed(), f.end.cast_signed())
        .await
        .unwrap();

    assert_eq!(
        df, control.matches,
        "R18: the arms disagree, so no number from this run counts"
    );
}

#[tokio::test]
async fn the_match_set_size_equals_the_planted_constant() {
    let f = fx();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let df = run(&ctx, f.start.cast_signed(), f.end.cast_signed())
        .await
        .unwrap();
    assert_eq!(
        u64::try_from(df.len()).unwrap(),
        SPEC.planted,
        "the join must find exactly the planted matches, not noise"
    );
}

#[tokio::test]
async fn a_single_bucket_range_agrees_between_arms() {
    let f = fx();
    let start = f.start;
    let end = f.start + HOUR_MS;
    let control = control::run(&f.store, start, end).unwrap();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let df = run(&ctx, start.cast_signed(), end.cast_signed())
        .await
        .unwrap();
    assert_eq!(
        df, control.matches,
        "arms must agree on a one-bucket window"
    );
}

#[tokio::test]
async fn narrowing_the_range_monotonically_shrinks_the_match_set() {
    let f = fx();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();

    let wide = run(&ctx, f.start.cast_signed(), f.end.cast_signed())
        .await
        .unwrap();
    let mid = f.start + (f.end - f.start) / 2;
    let narrow = run(&ctx, f.start.cast_signed(), mid.cast_signed())
        .await
        .unwrap();

    assert!(
        narrow.len() <= wide.len(),
        "narrowing must not grow the result: {} > {}",
        narrow.len(),
        wide.len()
    );
    assert!(
        narrow.is_subset(&wide),
        "a narrowed window must return a subset of the wider window"
    );
}

#[tokio::test]
async fn a_mid_bucket_boundary_still_returns_the_correct_rows() {
    // The provider claims `Inexact`, so DataFusion must re-filter rows that a
    // surviving bucket carries outside the predicate. Claiming `Exact` here
    // would silently drop rows; this is the test that catches that.
    let f = fx();
    let start = f.start + HOUR_MS / 2;
    let end = f.start + HOUR_MS * 3 + HOUR_MS / 3;
    let control = control::run(&f.store, start, end).unwrap();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let df = run(&ctx, start.cast_signed(), end.cast_signed())
        .await
        .unwrap();
    assert_eq!(
        df, control.matches,
        "mid-bucket boundaries must agree, proving the Inexact claim is honest"
    );
}

#[tokio::test]
async fn a_range_entirely_outside_the_fixture_returns_nothing() {
    let f = fx();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let far = f.end + HOUR_MS * 1_000;
    let df = run(&ctx, far.cast_signed(), (far + HOUR_MS).cast_signed())
        .await
        .unwrap();
    assert!(df.is_empty(), "an out-of-range window must plan zero rows");
}

#[tokio::test]
async fn a_full_range_count_matches_the_control_arms_decoded_rows() {
    let f = fx();
    let decoded = control::decode_only(&f.store, f.start, f.end).unwrap();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let batches = ctx
        .sql("SELECT count(*) AS n FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let n = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<datafusion::arrow::array::Int64Array>()
        .unwrap()
        .value(0);
    assert_eq!(
        usize::try_from(n).unwrap(),
        decoded,
        "the provider must expose exactly the rows the control arm decodes"
    );
}

#[tokio::test]
async fn a_query_naming_an_unknown_column_fails_at_planning() {
    let f = fx();
    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let err = ctx
        .sql("SELECT nonexistent_column FROM events")
        .await
        .expect_err("planning must reject an unknown column rather than return empty");
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent_column"),
        "the error must name the bad column, got: {msg}"
    );
}

#[test]
fn the_provider_discovers_every_bucket_the_store_reports() {
    let f = fx();
    let expected = f.store.list_buckets().unwrap().len();
    let table = EventTable::try_new(Arc::clone(&f.store)).unwrap();
    assert_eq!(
        table.bucket_count(),
        expected,
        "the provider must see every live bucket"
    );
    assert!(expected >= 24, "fixture should span >=24 buckets");
}

#[test]
fn the_granularity_assertion_passes_on_an_hourly_store() {
    let f = fx();
    let table = EventTable::try_new(Arc::clone(&f.store)).unwrap();
    table
        .assert_granularity()
        .expect("default retention keeps the store hourly; a daily store must fail loudly");
}

#[test]
fn the_lineage_sql_carries_the_time_predicate_the_provider_prunes_on() {
    let sql = lineage_sql(100, 200);
    assert!(
        sql.contains("c.ts_ms >= 100"),
        "lower bound must be present"
    );
    assert!(sql.contains("c.ts_ms < 200"), "upper bound must be present");
    assert!(sql.contains("JOIN"), "the measured query must be a join");
}

/// Plan a scan with an explicit `ts_ms` range and report how many partitions
/// survived pruning. This is the direct check that pruning happens at all —
/// without it, "partition pruning" is an untested claim.
async fn partitions_for(f: &Fx, lo: i64, hi: i64) -> usize {
    use datafusion::catalog::TableProvider;
    use datafusion::logical_expr::{col, lit};
    use datafusion::physical_plan::ExecutionPlanProperties;
    use datafusion::prelude::SessionContext;

    let table = EventTable::try_new(Arc::clone(&f.store)).unwrap();
    let ctx = SessionContext::new();
    let state = ctx.state();
    let filters = vec![col("ts_ms").gt_eq(lit(lo)), col("ts_ms").lt(lit(hi))];
    let plan = table.scan(&state, None, &filters, None).await.unwrap();
    plan.output_partitioning().partition_count()
}

#[tokio::test]
async fn a_two_hour_predicate_plans_exactly_the_buckets_covering_those_hours() {
    let f = fx();
    let lo = f.start.cast_signed();
    let hi = (f.start + 2 * HOUR_MS).cast_signed();
    let n = partitions_for(&f, lo, hi).await;
    assert_eq!(
        n, 2,
        "a two-hour window must plan exactly 2 hourly buckets, planned {n}"
    );
}

#[tokio::test]
async fn an_unbounded_query_plans_every_bucket() {
    let f = fx();
    let all = f.store.list_buckets().unwrap().len();
    let n = partitions_for(&f, 0, i64::MAX).await;
    assert_eq!(n, all, "an unbounded predicate must plan every bucket");
}

#[tokio::test]
async fn pruning_actually_reduces_the_partitions_read() {
    let f = fx();
    let all = f.store.list_buckets().unwrap().len();
    let pruned = partitions_for(
        &f,
        f.start.cast_signed(),
        (f.start + 3 * HOUR_MS).cast_signed(),
    )
    .await;
    assert!(
        pruned < all,
        "pruning must read fewer partitions than the store holds: {pruned} vs {all}"
    );
}

#[tokio::test]
async fn strict_and_inclusive_operators_prune_the_same_buckets_as_their_pairs() {
    // `>` and `<=` are recognized as prunable alongside `>=` and `<`, but the
    // measured query only ever uses the latter pair. Without this, a bug in the
    // Gt or LtEq arm would prune wrong for any future query and no test would
    // notice.
    use datafusion::catalog::TableProvider;
    use datafusion::logical_expr::{col, lit};
    use datafusion::physical_plan::ExecutionPlanProperties;
    use datafusion::prelude::SessionContext;

    let f = fx();
    let lo = f.start.cast_signed();
    let hi = (f.start + 2 * HOUR_MS).cast_signed();

    let table = EventTable::try_new(Arc::clone(&f.store)).unwrap();
    let ctx = SessionContext::new();
    let state = ctx.state();

    let inclusive = table
        .scan(
            &state,
            None,
            &[col("ts_ms").gt_eq(lit(lo)), col("ts_ms").lt(lit(hi))],
            None,
        )
        .await
        .unwrap()
        .output_partitioning()
        .partition_count();
    let strict = table
        .scan(
            &state,
            None,
            &[col("ts_ms").gt(lit(lo)), col("ts_ms").lt_eq(lit(hi))],
            None,
        )
        .await
        .unwrap()
        .output_partitioning()
        .partition_count();

    assert!(
        strict > 0 && inclusive > 0,
        "both forms must prune to something"
    );
    assert!(
        strict.abs_diff(inclusive) <= 1,
        "strict and inclusive bounds over the same window must select nearly the same buckets: \
         {strict} vs {inclusive}"
    );
}

#[tokio::test]
async fn the_tightest_of_several_lower_bounds_wins() {
    // scan() merges multiple bounds with max()/min(); only one bound per side is
    // ever supplied by the measured query, so this is the only check on that merge.
    use datafusion::catalog::TableProvider;
    use datafusion::logical_expr::{col, lit};
    use datafusion::physical_plan::ExecutionPlanProperties;
    use datafusion::prelude::SessionContext;

    let f = fx();
    let table = EventTable::try_new(Arc::clone(&f.store)).unwrap();
    let ctx = SessionContext::new();
    let state = ctx.state();

    let loose = f.start.cast_signed();
    let tight = (f.start + 20 * HOUR_MS).cast_signed();
    let both = table
        .scan(
            &state,
            None,
            &[
                col("ts_ms").gt_eq(lit(loose)),
                col("ts_ms").gt_eq(lit(tight)),
            ],
            None,
        )
        .await
        .unwrap()
        .output_partitioning()
        .partition_count();
    let tight_only = table
        .scan(&state, None, &[col("ts_ms").gt_eq(lit(tight))], None)
        .await
        .unwrap()
        .output_partitioning()
        .partition_count();

    assert_eq!(
        both, tight_only,
        "two lower bounds must collapse to the tighter one, not the looser"
    );
}

#[tokio::test]
async fn an_out_of_range_predicate_plans_zero_partitions_not_just_zero_rows() {
    // The end-to-end check only proves the result is empty. This proves the
    // provider never even planned a partition to read.
    let f = fx();
    let far = (f.end + HOUR_MS * 1_000).cast_signed();
    let n = partitions_for(&f, far, far + HOUR_MS.cast_signed()).await;
    assert_eq!(
        n, 0,
        "an out-of-range window must plan no partitions at all"
    );
}

#[tokio::test]
async fn provider_rows_match_scan_range_field_for_field_on_one_bucket() {
    // Field-level correctness of the Arrow conversion. Everything else infers
    // it from join results, which would not catch a swapped pid/ppid column.
    use datafusion::arrow::array::{Array, Int64Array, StringArray, UInt32Array};

    let f = fx();
    let bucket_start = f.start;
    let bucket_end = f.start + HOUR_MS;
    let mut expected = f.store.scan_range(bucket_start, bucket_end).unwrap();
    expected.sort_by_key(|r| r.pid.raw());

    let (ctx, _) = build_context(Arc::clone(&f.store), SessionSettings::default()).unwrap();
    let batches = ctx
        .sql(&format!(
            "SELECT ts_ms, pid, ppid, name FROM events \
             WHERE ts_ms >= {} AND ts_ms < {} ORDER BY pid",
            bucket_start.cast_signed(),
            bucket_end.cast_signed()
        ))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let mut got = Vec::new();
    for b in &batches {
        let ts = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
        let pid = b.column(1).as_any().downcast_ref::<UInt32Array>().unwrap();
        let ppid = b.column(2).as_any().downcast_ref::<UInt32Array>().unwrap();
        let name = b.column(3).as_any().downcast_ref::<StringArray>().unwrap();
        for i in 0..b.num_rows() {
            got.push((
                ts.value(i),
                pid.value(i),
                if ppid.is_null(i) {
                    None
                } else {
                    Some(ppid.value(i))
                },
                name.value(i).to_owned(),
            ));
        }
    }

    assert_eq!(got.len(), expected.len(), "row counts must match");
    for (g, e) in got.iter().zip(expected.iter()) {
        assert_eq!(g.0, e.collection_time.timestamp_millis(), "ts_ms mismatch");
        assert_eq!(g.1, e.pid.raw(), "pid mismatch");
        assert_eq!(
            g.2,
            e.ppid.map(daemoneye_lib::models::process::ProcessId::raw),
            "ppid mismatch"
        );
        assert_eq!(g.3, e.name, "name mismatch");
    }
}

/// R18 on the spec the recorded numbers actually came from.
///
/// Every other equivalence test runs a small spec. The measured run uses
/// `FixtureSpec::default()` (120k rows, 26 buckets, 500 planted), and until
/// this test existed the "500 = 500" in the decision artifact was a human
/// reading two stdout lines from two independent processes. This makes the gate
/// a check the suite enforces at the measured scale.
#[tokio::test]
async fn both_arms_agree_on_the_default_spec_the_measurements_use() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("default.redb");
    let spec = fixture::FixtureSpec::default();
    let stats = fixture::generate(&path, spec).unwrap();
    let store = Arc::new(EventStore::open(&path).unwrap());

    let start = stats.start_ms;
    let end = spec.end_ms();

    let control = control::run(&store, start, end).unwrap();
    let (ctx, buckets) = build_context(Arc::clone(&store), SessionSettings::default()).unwrap();
    let df = run(&ctx, start.cast_signed(), end.cast_signed())
        .await
        .unwrap();

    assert_eq!(
        df,
        control.matches,
        "R18 at the measured scale: the arms must return the same set, or no \
         number recorded from this spec counts. {} pid(s) differ: {:?}",
        df.symmetric_difference(&control.matches).count(),
        df.symmetric_difference(&control.matches)
            .take(10)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        u64::try_from(df.len()).unwrap(),
        spec.planted,
        "and that set must be exactly the planted matches"
    );
    assert_eq!(
        u64::try_from(control.rows_decoded).unwrap(),
        spec.rows,
        "the control arm must decode every generated row at this scale"
    );
    assert!(
        buckets >= 24,
        "the measured spec must span >=24 buckets, got {buckets}"
    );
}
