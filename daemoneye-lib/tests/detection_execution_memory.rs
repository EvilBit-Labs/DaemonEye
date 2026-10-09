//! Full-retention memory characterization for the `DataFusion` executor (U9; R13, R11, R9, KTD15).
//!
//! Every test here is `#[ignore]`d and driven by `just measure-detection-memory`: shared CI
//! runners are too inconsistent to gate on (GOTCHAS 3.1). The deliverable is the numbers each run
//! prints, recorded in `docs/decisions/`. Each measured run is its own test, hence its own
//! process under nextest, because a process's resident-set peak cannot be reset between runs.
//!
//! **What the runs are.** The 168- and 84-bucket runs scan a window spanning many buckets, so the
//! provider supplies the target partition count itself and no `RepartitionExec` is inserted:
//! that shape answers whether memory tracks the window, but production never executes it. A
//! production cycle's window sits inside one bucket, so the single-bucket run is the one that
//! carries the repartition and speaks to the 100 MiB target.
#![cfg(feature = "detection-engine")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "detection_execution_memory/fixture.rs"]
mod fixture;
#[path = "detection_execution_memory/rss.rs"]
mod rss;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::execution::completeness::{
    CollectorHealth, CycleSignals, IngestSnapshot,
};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::{CycleOutcome, RuleEvaluation, RuleExecutor};
use fixture::{
    BASE_MS, FULL_BUCKETS, FULL_SCAN_ID, HALF_BUCKETS, HOUR_MS, MAX_SIZE_ROWS, ROWS_PER_BUCKET,
};
use rss::{Sampler, SysinfoReader};

/// Cycles per run unless `DETECTION_MEMORY_CYCLES` says otherwise (development only).
const DEFAULT_CYCLES: usize = 300;
/// A back-half trend this large, in bytes, is a climb rather than allocator noise.
const PLATEAU_SLACK_BYTES: u64 = 4 * 1024 * 1024;
/// The lowered batch size for the batch run: below the 9,000-row bucket, above the floor.
const LOWERED_BATCH_SIZE: usize = 2_048;
const LOWERED_PARTITIONS: usize = 2;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("detection-memory")
}

fn retention_path() -> PathBuf {
    dir().join("retention.redb")
}

fn max_size_path() -> PathBuf {
    dir().join("max-size.redb")
}

fn cycles() -> usize {
    std::env::var("DETECTION_MEMORY_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CYCLES)
}

fn mib(bytes: u64) -> f64 {
    f64::from(u32::try_from(bytes.saturating_div(1024)).unwrap_or(u32::MAX)) / 1024.0
}

fn base_config() -> DetectionConfig {
    DetectionConfig {
        // The plan's cap: the 1,000 planted matches must not be cut off.
        max_matches_per_rule: 100_000,
        ..DetectionConfig::default()
    }
}

fn healthy() -> CycleSignals {
    CycleSignals {
        collection: [("procmond".to_owned(), Ok(()))].into(),
        heartbeat: [("procmond".to_owned(), CollectorHealth::Healthy)].into(),
        ingest: IngestSnapshot::default(),
    }
}

/// The window covering buckets `lo..hi`; the bounds are `(after, through]`.
fn window(lo: u64, hi: u64) -> CycleWindow {
    let edge = |bucket: u64| {
        BASE_MS
            .saturating_add(bucket.saturating_mul(HOUR_MS))
            .saturating_sub(1)
    };
    CycleWindow {
        after_ms: edge(lo),
        through_ms: edge(hi),
    }
}

fn median(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted
        .get(sorted.len().saturating_div(2))
        .copied()
        .unwrap_or(0)
}

fn quarter(series: &[u64], n: usize) -> &[u64] {
    let q = series.len().saturating_div(4);
    series
        .get(q.saturating_mul(n)..q.saturating_mul(n.saturating_add(1)))
        .unwrap_or(&[])
}

/// The series' back half must not climb: the last quarter's median stays within slack of the
/// third's. Returns both medians for the record.
fn assert_plateau(series: &[u64]) -> (u64, u64) {
    let third = median(quarter(series, 2));
    let fourth = median(quarter(series, 3));
    assert!(
        fourth <= third.saturating_add(PLATEAU_SLACK_BYTES),
        "RSS climbed across the back half of the cycles"
    );
    (third, fourth)
}

fn percentile(sorted: &[Duration], pct: usize) -> Duration {
    let idx = sorted
        .len()
        .saturating_mul(pct)
        .saturating_div(100)
        .min(sorted.len().saturating_sub(1));
    sorted.get(idx).copied().unwrap_or_default()
}

/// What one run needs to know about the window it scans.
struct Shape {
    label: &'static str,
    config: DetectionConfig,
    lo: u64,
    hi: u64,
    /// Whether the plan must carry a `RepartitionExec` (the production shape) or none.
    expect_repartition: Option<bool>,
}

fn partitions_in(plan: &str) -> String {
    plan.lines()
        .find(|l| l.contains("BucketScanExec"))
        .unwrap_or("no BucketScanExec line")
        .trim_matches(|c: char| c == '|' || c.is_whitespace())
        .to_owned()
}

async fn measure(shape: Shape) {
    let buckets = shape.hi.saturating_sub(shape.lo);
    let win = window(shape.lo, shape.hi);
    let (store, engine) = fixture::engine_over(&retention_path(), &shape.config);
    let rules = engine.runnable_rules();
    let exec = RuleExecutor::new(store, engine.regex_cache(), &shape.config).unwrap();
    let full_rule = rules
        .iter()
        .find(|r| r.rule.id.raw() == FULL_SCAN_ID)
        .expect("the full-scan rule is loaded");
    let plan = exec.explain(full_rule, win).await.unwrap();
    let has_repartition = plan.contains("RepartitionExec");
    if let Some(expected) = shape.expect_repartition {
        assert_eq!(has_repartition, expected, "the plan has the expected shape");
    }

    let mut sampler = Sampler::start(SysinfoReader::new().unwrap(), SysinfoReader::new().unwrap());
    let baseline = sampler.sample().unwrap();
    let (mut latencies, mut series) = (Vec::new(), Vec::new());
    let mut batches = 0;
    for cycle in 0..cycles() {
        let started = Instant::now();
        let outcome = exec.evaluate(&rules, win, &healthy()).await;
        latencies.push(started.elapsed());
        series.push(sampler.sample().unwrap());
        if cycle == 0 {
            batches = check_first_cycle(
                &outcome,
                shape.lo,
                shape.hi,
                shape.config.executor_batch_size,
            );
        }
    }
    let peak = sampler.finish().expect("a trustworthy RSS trace");

    latencies.sort_unstable();
    let (third, fourth) = assert_plateau(&series);
    let per_bucket = batches.checked_div(buckets).unwrap_or(0);
    eprintln!(
        "RUN {} | buckets={buckets} partitions_cfg={} batch_size={} cycles={} | baseline_mib={:.2} \
         peak_mib={:.2} | p50_ms={:.2} max_ms={:.2} | batches_per_bucket={per_bucket} \
         repartition={has_repartition} | q3_median_mib={:.2} q4_median_mib={:.2} | scan: {}",
        shape.label,
        shape.config.executor_target_partitions,
        shape.config.executor_batch_size,
        series.len(),
        mib(baseline),
        mib(peak),
        percentile(&latencies, 50).as_secs_f64() * 1000.0,
        latencies.last().copied().unwrap_or_default().as_secs_f64() * 1000.0,
        mib(third),
        mib(fourth),
        partitions_in(&plan),
    );
}

/// Assert the first cycle scanned what it should, and return the full-scan rule's batch count.
fn check_first_cycle(outcome: &CycleOutcome, lo: u64, hi: u64, batch_size: usize) -> u64 {
    let buckets = hi.saturating_sub(lo);
    let expected = fixture::planted_in(lo, hi);
    for evaluation in &outcome.evaluations {
        eprintln!(
            "CYCLE0 rule={} alerts={} rows_read={} batches={} oversized={} failed={}",
            evaluation.rule_id,
            evaluation.alerts.len(),
            evaluation.scan.rows_read,
            evaluation.scan.batches,
            evaluation.scan.oversized_rows,
            evaluation.failure.is_some(),
        );
        assert!(evaluation.failure.is_none(), "no rule failed");
        assert!(evaluation.result_capped.is_none(), "no rule was capped");
        assert_eq!(
            evaluation.alerts.len(),
            expected,
            "every planted row matched"
        );
        assert_eq!(evaluation.scan.oversized_rows, 0, "no row was excluded");
    }
    let full = full_scan(outcome);
    assert_eq!(
        full.scan.rows_read,
        buckets.saturating_mul(ROWS_PER_BUCKET),
        "the full-scan rule read every row of every bucket"
    );
    // Batches fill across bucket boundaries within a partition, so the floor is by rows.
    let min_batches = full
        .scan
        .rows_read
        .div_ceil(u64::try_from(batch_size).unwrap());
    assert!(
        full.scan.batches >= min_batches,
        "the scan batched at the configured size"
    );
    // The alerts carry only the projected columns, so the bucket comes from the planted pid.
    let hit_buckets: std::collections::BTreeSet<u64> = full
        .alerts
        .iter()
        .map(|a| fixture::bucket_of_planted_pid(a.process_record.pid.raw()))
        .collect();
    fixture::assert_spread(hit_buckets.len(), buckets);
    full.scan.batches
}

fn full_scan(outcome: &CycleOutcome) -> &RuleEvaluation {
    outcome
        .evaluations
        .iter()
        .find(|e| e.rule_id == FULL_SCAN_ID)
        .expect("the full-scan rule was evaluated")
}

// --- fixtures (run first, in their own process, so building never inflates a measured peak) -----

#[test]
#[ignore = "measurement fixture; run via `just measure-detection-memory`"]
fn build_fixtures() {
    std::fs::create_dir_all(dir()).unwrap();
    for path in [retention_path(), max_size_path()] {
        let _removed = std::fs::remove_file(path);
    }
    fixture::build_retention_store(&retention_path());
    fixture::build_max_size_store(&max_size_path());
    for path in [retention_path(), max_size_path()] {
        let bytes = std::fs::metadata(&path).unwrap().len();
        eprintln!("FIXTURE {} on_disk_mib={:.1}", path.display(), mib(bytes));
    }
}

// --- the measured runs ----------------------------------------------------------------------------

#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_1_default_168_buckets() {
    measure(Shape {
        label: "default-168",
        config: base_config(),
        lo: 0,
        hi: FULL_BUCKETS,
        expect_repartition: Some(false),
    })
    .await;
}

#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_2_default_84_buckets() {
    measure(Shape {
        label: "default-84",
        config: base_config(),
        lo: FULL_BUCKETS - HALF_BUCKETS,
        hi: FULL_BUCKETS,
        expect_repartition: Some(false),
    })
    .await;
}

#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_3_partitions_lowered_168_buckets() {
    measure(Shape {
        label: "partitions-2-168",
        config: DetectionConfig {
            executor_target_partitions: LOWERED_PARTITIONS,
            ..base_config()
        },
        lo: 0,
        hi: FULL_BUCKETS,
        expect_repartition: Some(false),
    })
    .await;
}

#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_4_batch_size_lowered_168_buckets() {
    measure(Shape {
        label: "batch-2048-168",
        config: DetectionConfig {
            executor_batch_size: LOWERED_BATCH_SIZE,
            ..base_config()
        },
        lo: 0,
        hi: FULL_BUCKETS,
        expect_repartition: Some(false),
    })
    .await;
}

/// The production shape (R3): the window sits inside one bucket, the provider supplies one
/// partition against a target of four, and the optimizer inserts a `RepartitionExec`.
#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_5_default_single_bucket_production_shape() {
    measure(Shape {
        label: "default-1-production",
        config: base_config(),
        lo: FULL_BUCKETS - 1,
        hi: FULL_BUCKETS,
        expect_repartition: Some(true),
    })
    .await;
}

/// R9's byte bound under worst-case field sizes: 24 rows of about 1 MiB each must close batches on
/// `executor_batch_max_bytes`, never on the 8,192-row bound.
#[tokio::test]
#[ignore = "measurement; run via `just measure-detection-memory`"]
async fn run_6_max_size_rows_close_batches_on_the_byte_bound() {
    let config = base_config();
    let (store, engine) = fixture::engine_over(&max_size_path(), &config);
    let rules = engine.runnable_rules();
    let exec = RuleExecutor::new(store, engine.regex_cache(), &config).unwrap();
    let mut sampler = Sampler::start(SysinfoReader::new().unwrap(), SysinfoReader::new().unwrap());
    sampler.sample().unwrap();
    let mut batches = 0;
    for _ in 0..cycles().min(50) {
        let outcome = exec.evaluate(&rules, window(0, 1), &healthy()).await;
        let full = full_scan(&outcome);
        assert!(full.failure.is_none(), "the full-scan rule succeeded");
        assert_eq!(
            full.scan.rows_read, MAX_SIZE_ROWS,
            "every max-size row was read"
        );
        assert_eq!(full.scan.oversized_rows, 0, "no max-size row was excluded");
        batches = full.scan.batches;
        sampler.sample().unwrap();
    }
    let peak = sampler.finish().expect("a trustworthy RSS trace");
    let rows_per_batch_cap = u64::try_from(
        config
            .executor_batch_max_bytes
            .checked_div(DetectionConfig::ASSUMED_WORST_CASE_ROW_BYTES)
            .unwrap_or(1),
    )
    .unwrap()
    .max(1);
    let row_bound_batches =
        MAX_SIZE_ROWS.div_ceil(u64::try_from(config.executor_batch_size).unwrap());
    assert!(
        batches >= MAX_SIZE_ROWS.div_ceil(rows_per_batch_cap) && batches > row_bound_batches,
        "batches closed on the byte bound, not the row bound"
    );
    eprintln!(
        "RUN max-size | rows={MAX_SIZE_ROWS} row_bytes~{} batch_max_bytes={} batches={batches} \
         row_bound_would_give={row_bound_batches} | peak_mib={:.2}",
        fixture::MAX_COMMAND_LINE_BYTES,
        config.executor_batch_max_bytes,
        mib(peak),
    );
}
