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
