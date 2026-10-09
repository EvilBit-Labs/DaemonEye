//! `RuleExecutor::evaluate` over a real event store (U8b; R1, R5, R7, KTD6).
//!
//! Each test builds a store, an engine that issues the `RunnableRule`s, and an executor over the
//! same configuration, so a `Generation` is only ever one the engine handed out.
#![cfg(feature = "detection-engine")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::verify_spawn_token;
use daemoneye_lib::detection::execution::completeness::{
    CollectorHealth, CycleSignals, IngestSnapshot,
};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::{
    CycleOutcome, EvaluationFailure, RuleEvaluation, RuleExecutor,
};
use daemoneye_lib::models::{AlertSeverity, DetectionRule, ProcessRecord};
use daemoneye_lib::proto::{ColumnDescriptor, ColumnType, SchemaDescriptor, TableDescriptor};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::IngestRecord;
use tempfile::TempDir;

const HOUR: u64 = 3_600_000;
const WIDE: CycleWindow = CycleWindow {
    after_ms: 0,
    through_ms: 100 * HOUR,
};
/// The smallest batch the configuration accepts, so a few thousand rows are dozens of batches.
const SMALL_BATCH: usize = DetectionConfig::EXECUTOR_BATCH_SIZE_MIN;

struct Fixture {
    _dir: TempDir,
    store: Arc<EventStore>,
    engine: DetectionEngine,
    config: DetectionConfig,
}

fn column(name: &str, column_type: ColumnType) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(column_type),
        nullable: false,
        supported_ops: Vec::new(),
    }
}

fn schema() -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![
                column("pid", ColumnType::Uint),
                column("name", ColumnType::String),
                ColumnDescriptor {
                    nullable: true,
                    ..column("command_line", ColumnType::String)
                },
                column("collection_time", ColumnType::Int),
            ],
        }],
        conformance_results: Vec::new(),
    }
}

fn fixture(config: DetectionConfig) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("exec.redb")).unwrap());
    let mut engine = DetectionEngine::with_config(&config);
    let token = "a".repeat(64);
    let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
    engine.register_collector(&verified, schema()).unwrap();
    Fixture {
        _dir: dir,
        store,
        engine,
        config,
    }
}

fn config() -> DetectionConfig {
    DetectionConfig {
        // Out of the way unless a test is about the latency stop.
        pattern_latency_threshold_ms: DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX,
        ..DetectionConfig::default()
    }
}

fn record(ts_ms: u64, pid: u32, name: &str) -> ProcessRecord {
    let mut r = ProcessRecord::new(pid, name.to_owned());
    r.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
        .unwrap();
    r
}

fn put(fx: &Fixture, rows: Vec<ProcessRecord>) {
    let batch: Vec<IngestRecord> = rows
        .into_iter()
        .enumerate()
        .map(|(i, record)| {
            IngestRecord::new(
                "test",
                u64::try_from(i).unwrap(),
                u32::try_from(i).unwrap(),
                record,
            )
            .unwrap()
        })
        .collect();
    fx.store.put_batch(&batch).unwrap();
}

/// `count` rows named `name`, pids `1..=count`, one millisecond apart.
fn rows_named(name: &str, count: u32) -> Vec<ProcessRecord> {
    (1..=count)
        .map(|pid| record(10 * HOUR + u64::from(pid), pid, name))
        .collect()
}

fn load(fx: &mut Fixture, id: &str, sql: &str) {
    let rule = DetectionRule::new(
        id.to_owned(),
        format!("Rule {id}"),
        "executor fixture".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::High,
    );
    fx.engine.load_rule(rule).unwrap();
}

/// Every collector reporting well: the signals that add no completeness reason.
fn healthy() -> CycleSignals {
    CycleSignals {
        collection: [("procmond".to_owned(), Ok(()))].into(),
        heartbeat: [("procmond".to_owned(), CollectorHealth::Healthy)].into(),
        ingest: IngestSnapshot::default(),
    }
}

fn executor(fx: &Fixture) -> RuleExecutor {
    RuleExecutor::new(Arc::clone(&fx.store), fx.engine.regex_cache(), &fx.config).unwrap()
}

async fn run(fx: &Fixture) -> CycleOutcome {
    executor(fx)
        .evaluate(&fx.engine.runnable_rules(), WIDE, &healthy())
        .await
}

fn only(outcome: &CycleOutcome) -> &RuleEvaluation {
    assert_eq!(
        outcome.evaluations.len(),
        1,
        "exactly one rule was evaluated"
    );
    &outcome.evaluations[0]
}

fn alert_pids(evaluation: &RuleEvaluation) -> Vec<u32> {
    evaluation
        .alerts
        .iter()
        .map(|a| a.process_record.pid.raw())
        .collect()
}

// --- R1: matches become alerts -----------------------------------------------------------------

#[tokio::test]
async fn five_matching_rows_of_fifty_become_five_alerts_naming_rule_and_pid() {
    let mut fx = fixture(config());
    // Pids 101..=150; every tenth is `nc`, so 110, 120, 130, 140, 150 match.
    let rows: Vec<ProcessRecord> = (101..=150_u32)
        .map(|pid| {
            let name = if pid % 10 == 0 { "nc" } else { "bash" };
            record(10 * HOUR + u64::from(pid), pid, name)
        })
        .collect();
    put(&fx, rows);
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name = 'nc' AND pid > 100",
    );

    let outcome = run(&fx).await;

    let evaluation = only(&outcome);
    assert_eq!(evaluation.rule_id, "r1");
    assert!(evaluation.failure.is_none());
    assert!(evaluation.result_capped.is_none());
    assert_eq!(alert_pids(evaluation), vec![110, 120, 130, 140, 150]);
    assert!(
        evaluation
            .alerts
            .iter()
            .all(|a| a.detection_rule_id == "r1")
    );
}

#[tokio::test]
async fn a_row_outside_the_window_is_not_evaluated() {
    let mut fx = fixture(config());
    put(&fx, rows_named("nc", 4));
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let window = CycleWindow {
        after_ms: 10 * HOUR + 2,
        through_ms: 10 * HOUR + 3,
    };

    let outcome = executor(&fx)
        .evaluate(&fx.engine.runnable_rules(), window, &healthy())
        .await;

    assert_eq!(alert_pids(only(&outcome)), vec![3]);
}

// --- R5: the cap and its overflow --------------------------------------------------------------

async fn capped_run(matching_rows: u32) -> RuleEvaluation {
    let mut fx = fixture(DetectionConfig {
        max_matches_per_rule: 3,
        ..config()
    });
    put(&fx, rows_named("nc", matching_rows));
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    only(&run(&fx).await).clone()
}

#[tokio::test]
async fn exactly_cap_matches_are_all_returned_and_not_flagged() {
    let evaluation = capped_run(3).await;

    assert_eq!(alert_pids(&evaluation), vec![1, 2, 3]);
    assert!(evaluation.result_capped.is_none());
}

#[tokio::test]
async fn one_more_than_cap_returns_the_first_cap_and_flags_the_cap() {
    let evaluation = capped_run(4).await;

    assert_eq!(alert_pids(&evaluation), vec![1, 2, 3]);
    assert_eq!(evaluation.result_capped, Some(3));
}

#[tokio::test]
async fn five_matches_with_a_cap_of_three_yield_three_alerts() {
    let evaluation = capped_run(5).await;

    assert_eq!(alert_pids(&evaluation), vec![1, 2, 3]);
    assert_eq!(evaluation.result_capped, Some(3));
}

// --- R7: latency reports carry the issued generation -------------------------------------------

#[tokio::test]
async fn a_matching_regexp_over_twenty_thousand_rows_reports_per_yielded_batch_with_the_rules_generation()
 {
    let mut fx = fixture(DetectionConfig {
        // Every row matches, so the default cap of 1,000 would end the scan after one batch.
        max_matches_per_rule: DetectionConfig::MAX_MATCHES_PER_RULE_MAX,
        ..config()
    });
    put(&fx, rows_named("bash", 20_000));
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name REGEXP '^b'",
    );
    let issued = fx.engine.runnable_rules()[0].generation;

    let outcome = run(&fx).await;

    assert_eq!(only(&outcome).alerts.len(), 20_000);
    assert!(outcome.reports.len() >= 3, "one report per yielded batch");
    for report in &outcome.reports {
        assert_eq!(report.generation, issued);
        assert_eq!(report.rule_id, "r1");
        assert_eq!(report.pattern, "^b");
    }
}

#[tokio::test]
async fn a_regexp_that_matches_nothing_still_reports_its_worst_batch_when_the_scan_ends() {
    let mut fx = fixture(config());
    put(&fx, rows_named("bash", 20_000));
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name REGEXP '^zzz$'",
    );
    let issued = fx.engine.runnable_rules()[0].generation;

    let outcome = run(&fx).await;

    assert!(only(&outcome).alerts.is_empty());
    // Within its threshold (config() parks it at the maximum): every scan batch is read and the
    // single report still arrives when the stream ends. This is the control for the selective
    // breach test below.
    assert!(!only(&outcome).stopped_on_latency);
    assert_eq!(only(&outcome).scan.batches, 3);
    assert_eq!(outcome.reports.len(), 1);
    assert_eq!(outcome.reports[0].generation, issued);
    assert_eq!(outcome.reports[0].pattern, "^zzz$");
}

#[tokio::test]
async fn a_report_carries_the_generation_of_a_reloaded_rule() {
    let mut fx = fixture(config());
    put(&fx, rows_named("bash", 10));
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name REGEXP '^b'",
    );
    let first = fx.engine.runnable_rules()[0].generation;
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name REGEXP '^b'",
    );
    let second = fx.engine.runnable_rules()[0].generation;
    assert_ne!(first, second);

    let outcome = run(&fx).await;

    assert!(!outcome.reports.is_empty());
    assert!(outcome.reports.iter().all(|r| r.generation == second));
    assert_eq!(only(&outcome).generation, second);
}

// --- KTD6: the in-evaluate stop ----------------------------------------------------------------

const BATCHES: usize = 40;
/// The scan feeds a two-slot channel, so a producer can be a few batches ahead of a consumer that
/// stopped; the stop is "bounded by one batch" at the consumer, not "zero further reads".
const PRODUCER_LEAD: u64 = 6;

fn stop_fixture() -> Fixture {
    let mut fx = fixture(DetectionConfig {
        max_matches_per_rule: DetectionConfig::MAX_MATCHES_PER_RULE_MAX,
        executor_batch_size: SMALL_BATCH,
        ..config()
    });
    put(
        &fx,
        rows_named("bash", u32::try_from(SMALL_BATCH * BATCHES).unwrap()),
    );
    load(
        &mut fx,
        "a_regexp",
        "SELECT name FROM processes WHERE name REGEXP '.'",
    );
    load(
        &mut fx,
        "b_plain",
        "SELECT name FROM processes WHERE pid > 0",
    );
    fx
}

fn by_id<'a>(outcome: &'a CycleOutcome, id: &str) -> &'a RuleEvaluation {
    outcome
        .evaluations
        .iter()
        .find(|e| e.rule_id == id)
        .unwrap()
}

#[tokio::test]
async fn a_breaching_rule_stops_draining_in_the_same_call_while_the_next_rule_runs_to_completion() {
    let fx = stop_fixture();
    let mut rules = fx.engine.runnable_rules();
    for rule in &mut rules {
        if rule.rule.id.raw() == "a_regexp" {
            // Any measured batch exceeds zero, so the first batch breaches deterministically.
            rule.pattern_latency_threshold = Duration::ZERO;
        }
    }

    let outcome = executor(&fx).evaluate(&rules, WIDE, &healthy()).await;

    let breaching = by_id(&outcome, "a_regexp");
    let control = by_id(&outcome, "b_plain");
    let total = u64::try_from(BATCHES).unwrap();
    assert!(breaching.stopped_on_latency);
    assert!(
        breaching.scan.batches <= PRODUCER_LEAD,
        "stopped after the breaching batch"
    );
    assert!(breaching.scan.batches < total);
    assert!(breaching.scan.rows_read < u64::try_from(SMALL_BATCH).unwrap() * total);
    assert!(!breaching.alerts.is_empty());
    assert!(!control.stopped_on_latency);
    assert_eq!(control.scan.batches, total);
    assert_eq!(control.alerts.len(), SMALL_BATCH * BATCHES);
    assert!(
        outcome.reports.iter().any(|r| r.rule_id == "a_regexp"),
        "the breach is still reported for the post-hoc disable"
    );
}

#[tokio::test]
async fn the_same_rule_within_its_threshold_reads_every_batch() {
    let fx = stop_fixture();
    let rules = fx.engine.runnable_rules();

    let outcome = executor(&fx).evaluate(&rules, WIDE, &healthy()).await;

    let evaluation = by_id(&outcome, "a_regexp");
    assert!(!evaluation.stopped_on_latency);
    assert_eq!(evaluation.scan.batches, u64::try_from(BATCHES).unwrap());
    assert_eq!(evaluation.alerts.len(), SMALL_BATCH * BATCHES);
}

/// A rule that matches nothing, so `FilterExec` yields nothing between scan batches.
fn selective_fixture() -> Fixture {
    let mut fx = fixture(DetectionConfig {
        executor_batch_size: SMALL_BATCH,
        ..config()
    });
    put(
        &fx,
        rows_named("bash", u32::try_from(SMALL_BATCH * BATCHES).unwrap()),
    );
    load(
        &mut fx,
        "sel",
        "SELECT name FROM processes WHERE name REGEXP '^zzz$'",
    );
    fx
}

#[tokio::test]
async fn a_selective_breaching_rule_stops_scanning_instead_of_reading_every_batch() {
    let fx = selective_fixture();
    let issued = fx.engine.runnable_rules()[0].generation;
    let mut rules = fx.engine.runnable_rules();
    rules[0].pattern_latency_threshold = Duration::ZERO;

    let outcome = executor(&fx).evaluate(&rules, WIDE, &healthy()).await;

    let evaluation = only(&outcome);
    assert!(
        evaluation.scan.batches <= PRODUCER_LEAD,
        "the scan stopped near the breaching batch, not at the end"
    );
    assert!(evaluation.stopped_on_latency);
    assert!(
        evaluation.failure.is_none(),
        "a latency stop is not a failure"
    );
    assert!(evaluation.alerts.is_empty());
    assert!(evaluation.scan.rows_read <= u64::try_from(SMALL_BATCH).unwrap() * PRODUCER_LEAD);
    // The post-hoc path still sees the breach, under the issued generation.
    assert_eq!(outcome.reports.len(), 1);
    assert_eq!(outcome.reports[0].pattern, "^zzz$");
    assert_eq!(outcome.reports[0].generation, issued);
    assert!(outcome.reports[0].observed > Duration::ZERO);
}

#[tokio::test]
async fn a_selective_rule_within_its_threshold_still_reads_every_batch() {
    let fx = selective_fixture();

    let outcome = run(&fx).await;

    let evaluation = only(&outcome);
    assert!(!evaluation.stopped_on_latency);
    assert!(evaluation.failure.is_none());
    assert_eq!(evaluation.scan.batches, u64::try_from(BATCHES).unwrap());
    assert_eq!(
        evaluation.scan.rows_read,
        u64::try_from(SMALL_BATCH * BATCHES).unwrap()
    );
    assert_eq!(outcome.reports.len(), 1);
}

// --- failures and the scan counters ------------------------------------------------------------

#[tokio::test]
async fn a_rule_that_cannot_be_planned_records_a_failure_and_the_next_rule_still_runs() {
    let mut fx = fixture(config());
    put(&fx, rows_named("nc", 2));
    load(
        &mut fx,
        "a_bad",
        "SELECT pid FROM processes WHERE name = 'nc'",
    );
    load(
        &mut fx,
        "b_good",
        "SELECT pid FROM processes WHERE name = 'nc'",
    );
    let mut rules = fx.engine.runnable_rules();
    rules[0]
        .descriptor
        .columns
        .push(column("not_a_real_column", ColumnType::String));

    let outcome = executor(&fx).evaluate(&rules, WIDE, &healthy()).await;

    let bad = by_id(&outcome, "a_bad");
    assert!(matches!(bad.failure, Some(EvaluationFailure::Execution(_))));
    assert!(bad.alerts.is_empty());
    let good = by_id(&outcome, "b_good");
    assert!(good.failure.is_none());
    assert_eq!(good.alerts.len(), 2);
}

#[tokio::test]
async fn a_row_too_large_to_batch_is_counted_not_silently_dropped() {
    let mut fx = fixture(config());
    let mut rows = rows_named("nc", 3);
    rows[1].command_line = Some("y".repeat(5 * 1024 * 1024));
    put(&fx, rows);
    load(
        &mut fx,
        "r1",
        "SELECT command_line FROM processes WHERE name = 'nc'",
    );

    let outcome = run(&fx).await;

    let evaluation = only(&outcome);
    assert_eq!(evaluation.scan.oversized_rows, 1);
    assert_eq!(evaluation.scan.table, "processes");
    assert_eq!(evaluation.alerts.len(), 2);
}

// --- explain (R23) -------------------------------------------------------------------------------

/// Everything this crate's `prepare` plans with `name = 'nc'` pushed down and a `REGEXP` left over
/// as the residual; the residual is what must keep a `FilterExec` above the scan.
const RESIDUAL_RULE: &str =
    "SELECT pid FROM processes WHERE name = 'nc' AND command_line REGEXP 'x+y'";

async fn explained(window: CycleWindow) -> String {
    let mut fx = fixture(config());
    put(&fx, rows_named("nc", 3));
    load(&mut fx, "r1", RESIDUAL_RULE);
    let rules = fx.engine.runnable_rules();
    executor(&fx).explain(&rules[0], window).await.unwrap()
}

#[tokio::test]
async fn explain_a_rule_with_a_residual_names_the_scan_a_filter_and_the_fetch_bound() {
    let text = explained(WIDE).await;

    assert!(text.contains("BucketScanExec"));
    assert!(text.contains("FilterExec"));
    assert!(text.contains("fetch="));
}

#[tokio::test]
async fn explain_a_window_that_excludes_every_bucket_plans_an_empty_scan() {
    // Rows sit at hour 10; the window is hours 50..60, so every bucket is pruned.
    let text = explained(CycleWindow {
        after_ms: 50 * HOUR,
        through_ms: 60 * HOUR,
    })
    .await;

    assert!(text.contains("EmptyExec"));
    assert!(!text.contains("BucketScanExec"));
}

#[tokio::test]
async fn explain_describes_the_plan_that_evaluate_runs() {
    let mut fx = fixture(config());
    put(&fx, rows_named("nc", 3));
    load(&mut fx, "r1", RESIDUAL_RULE);
    let rules = fx.engine.runnable_rules();
    let exec = executor(&fx);

    let first = exec.explain(&rules[0], WIDE).await.unwrap();
    let second = exec.explain(&rules[0], WIDE).await.unwrap();
    let outcome = exec.evaluate(&rules, WIDE, &healthy()).await;

    assert_eq!(first, second, "explain is deterministic");
    assert!(only(&outcome).failure.is_none());
    assert!(
        first.contains("BucketScanExec"),
        "the scan explain names is the one evaluate reads"
    );
    assert!(only(&outcome).scan.batches > 0);
}
