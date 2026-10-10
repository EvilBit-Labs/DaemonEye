//! The agent's detection cycle (T6 · U13): `run_detection_cycle` over a real event store, with the
//! engine lock counted and, where a test needs it, a reload interleaved between two acquisitions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{TimeZone, Utc};
use daemoneye_agent::HeartbeatStatus;
use daemoneye_agent::detection_cycle::{
    CycleResult, EngineCell, PROCMOND_COLLECTOR_ID, build_signals, ingest_cycle,
    load_persisted_rules, next_window, run_detection_cycle,
};
use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::verify_spawn_token;
use daemoneye_lib::detection::execution::completeness::{CollectorHealth, IngestSnapshot};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::RuleExecutor;
use daemoneye_lib::models::{AlertSeverity, CompletenessReason, DetectionRule, ProcessRecord};
use daemoneye_lib::proto::{ColumnDescriptor, ColumnType, SchemaDescriptor, TableDescriptor};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::{self, IngestConfig, IngestRecord, SequenceGap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};

const BASE_MS: u64 = 36_000_000;
const NC_RULE: &str = "SELECT name FROM processes WHERE name = 'nc'";
const WIDE: CycleWindow = CycleWindow {
    after_ms: 0,
    through_ms: BASE_MS * 100,
};

type Hook = Box<dyn FnOnce(&mut DetectionEngine) + Send>;

/// An engine behind the real mutex that counts acquisitions and can run one closure on the engine
/// just before the Nth of them is handed out, which is how a test interleaves a reload.
struct CountingCell {
    inner: Mutex<DetectionEngine>,
    locks: AtomicUsize,
    hook: std::sync::Mutex<Option<(usize, Hook)>>,
}

impl CountingCell {
    fn new(engine: DetectionEngine) -> Self {
        Self {
            inner: Mutex::new(engine),
            locks: AtomicUsize::new(0),
            hook: std::sync::Mutex::new(None),
        }
    }

    fn before_lock(&self, nth: usize, hook: impl FnOnce(&mut DetectionEngine) + Send + 'static) {
        *self.hook.lock().unwrap() = Some((nth, Box::new(hook)));
    }

    fn lock_count(&self) -> usize {
        self.locks.load(Ordering::SeqCst)
    }
}

impl CountingCell {
    async fn acquire(&self) -> MutexGuard<'_, DetectionEngine> {
        let nth = self.locks.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let due = {
            let mut slot = self.hook.lock().unwrap();
            match slot.take() {
                Some((at, hook)) if at == nth => Some(hook),
                other => {
                    *slot = other;
                    None
                }
            }
        };
        if let Some(hook) = due {
            hook(&mut *self.inner.lock().await);
        }
        self.inner.lock().await
    }
}

impl EngineCell for CountingCell {
    fn lock(&self) -> impl Future<Output = MutexGuard<'_, DetectionEngine>> + Send {
        self.acquire()
    }
}

struct Fx {
    _dir: TempDir,
    store: Arc<EventStore>,
    cell: CountingCell,
    executor: RuleExecutor,
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
        collector_id: PROCMOND_COLLECTOR_ID.to_owned(),
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

fn fx(config: &DetectionConfig) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("agent.redb")).unwrap());
    let mut engine = DetectionEngine::with_config(config);
    let token = "a".repeat(64);
    let verified = verify_spawn_token(PROCMOND_COLLECTOR_ID, Some(&token), Some(&token)).unwrap();
    engine.register_collector(&verified, schema()).unwrap();
    let executor = RuleExecutor::new(Arc::clone(&store), engine.regex_cache(), config).unwrap();
    Fx {
        _dir: dir,
        store,
        cell: CountingCell::new(engine),
        executor,
    }
}

fn default_fx() -> Fx {
    fx(&DetectionConfig::default())
}

fn rule(id: &str, sql: &str) -> DetectionRule {
    DetectionRule::new(
        id.to_owned(),
        format!("Rule {id}"),
        "cycle fixture".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::High,
    )
}

async fn load(fx: &Fx, id: &str, sql: &str) {
    fx.cell.inner.lock().await.load_rule(rule(id, sql)).unwrap();
}

fn record(ts_ms: u64, pid: u32, name: &str) -> ProcessRecord {
    let mut r = ProcessRecord::new(pid, name.to_owned());
    r.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
        .unwrap();
    r
}

fn put(fx: &Fx, rows: Vec<ProcessRecord>) {
    let batch: Vec<IngestRecord> = rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let n = u32::try_from(i).unwrap();
            IngestRecord::new("test", u64::from(n), n, r).unwrap()
        })
        .collect();
    fx.store.put_batch(&batch).unwrap();
}

fn healthy() -> daemoneye_lib::detection::execution::completeness::CycleSignals {
    build_signals(
        PROCMOND_COLLECTOR_ID,
        Ok(()),
        Some(CollectorHealth::Healthy),
        IngestSnapshot::default(),
    )
}

async fn cycle(fx: &Fx, window: CycleWindow) -> CycleResult {
    run_detection_cycle(&fx.cell, &fx.executor, window, &healthy()).await
}

fn alert_pids(result: &CycleResult) -> Vec<u32> {
    let mut pids: Vec<u32> = result
        .alerts
        .iter()
        .map(|a| a.process_record.pid.raw())
        .collect();
    pids.sort_unstable();
    pids
}

/// A time inside a bucket that is still open. The postings cache keeps only closed buckets (a
/// bucket below the wall clock's), so a test that writes into a "closed" 1970 bucket between two
/// cycles would read the first cycle's cached list; production rows carry the current time.
fn open_bucket_ms() -> u64 {
    let now = u64::try_from(Utc::now().timestamp_millis()).unwrap();
    now.saturating_add(24 * 3_600_000)
}

// --- AE6: the whole plan, end to end -------------------------------------------------------------

#[tokio::test]
async fn a_persisted_rule_alerts_once_per_row_across_cycles_and_again_for_a_new_row() {
    let fx = default_fx();
    let base = open_bucket_ms();
    fx.store.store_rule(&rule("persisted", NC_RULE)).unwrap();
    let loaded = load_persisted_rules(&fx.store, &mut *fx.cell.inner.lock().await).unwrap();
    assert_eq!(loaded, 1);
    assert!(fx.store.list_buckets().unwrap().is_empty());
    let pipeline = ingest::spawn(Arc::clone(&fx.store), IngestConfig::default());

    // Cycle 1: one matching process among two.
    let rows = vec![record(base, 10, "nc"), record(base + 1, 11, "bash")];
    let first = ingest_cycle(&pipeline, PROCMOND_COLLECTOR_ID, 0, &rows)
        .await
        .unwrap();
    let mark = first.high_water_ms;
    let one = cycle(&fx, next_window(base - 1, mark)).await;
    assert_eq!(alert_pids(&one), vec![10]);

    // Cycle 2: nothing collected. The cycle's high-water mark does not move.
    let two = cycle(&fx, next_window(mark, mark)).await;
    assert!(
        two.alerts.is_empty(),
        "the row that matched in cycle 1 is outside cycle 2's window"
    );

    // Cycle 3: a new matching process. Without this, "zero in cycle 2" would also pass for an
    // engine that stopped working after one cycle.
    let later = vec![record(base + 5_000, 12, "nc")];
    let third = ingest_cycle(&pipeline, PROCMOND_COLLECTOR_ID, 1, &later)
        .await
        .unwrap();
    let three = cycle(&fx, next_window(mark, third.high_water_ms)).await;
    assert_eq!(alert_pids(&three), vec![12]);
    pipeline.flush_and_stop().await;
}

// --- R8: the result gate ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rule_disabled_by_a_breach_mid_cycle_shows_matches_but_delivers_no_alerts() {
    let fx = default_fx();
    put(&fx, vec![record(BASE_MS, 10, "nc")]);
    load(&fx, "breaching", NC_RULE).await;
    fx.cell.before_lock(2, |engine| {
        let generation = engine.runnable_rules().first().unwrap().generation;
        assert!(engine.observe_pattern_latency("breaching", generation, Duration::from_millis(11)));
    });

    let result = cycle(&fx, WIDE).await;

    let evaluation = result.evaluations.first().unwrap();
    assert_eq!(evaluation.alerts.len(), 1, "the rule did find the row");
    assert!(
        result.alerts.is_empty(),
        "a breach observed mid-cycle drops the cycle's alerts"
    );
    assert_eq!(result.dropped_after_reeligibility, 1);
    assert!(fx.cell.inner.lock().await.runnable_rules().is_empty());
}

#[tokio::test]
async fn a_rule_reloaded_mid_cycle_has_its_evaluation_dropped_and_its_reports_discarded() {
    let fx = heavy_fx().await;
    fx.cell.before_lock(2, |engine| {
        engine
            .load_rule(rule("heavy", HEAVY_RULE))
            .expect("reload succeeds");
    });

    let result = cycle(&fx, WIDE).await;

    let evaluation = result.evaluations.first().unwrap();
    assert!(
        evaluation.stopped_on_latency,
        "the old load breached its latency budget"
    );
    assert!(!evaluation.alerts.is_empty());
    assert!(result.alerts.is_empty());
    assert_eq!(result.dropped_after_reeligibility, 1);
    let (runnable, recorded) = {
        let engine = fx.cell.inner.lock().await;
        (
            engine.runnable_rules().len(),
            engine.last_evaluation("heavy").is_some(),
        )
    };
    assert_eq!(
        runnable, 1,
        "the breach belonged to the previous load and was not applied to the reloaded rule"
    );
    assert!(!recorded, "the dropped evaluation was not recorded");
}

const HEAVY_RULE: &str = "SELECT pid FROM processes WHERE command_line REGEXP '(ab|ba)+z'";

/// A rule whose regexp over long command lines certainly outlasts the minimum 1 ms budget.
async fn heavy_fx() -> Fx {
    let fx = fx(&DetectionConfig {
        pattern_latency_threshold_ms: DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MIN,
        ..DetectionConfig::default()
    });
    let line = format!("{}z", "ab".repeat(1_500));
    let rows = (1..=600_u32)
        .map(|pid| {
            let mut r = record(BASE_MS + u64::from(pid), pid, "bash");
            r.command_line = Some(line.clone());
            r
        })
        .collect();
    put(&fx, rows);
    load(&fx, "heavy", HEAVY_RULE).await;
    fx
}

#[tokio::test]
async fn a_pattern_that_breaches_its_budget_is_disabled_and_its_cycle_dropped() {
    let fx = heavy_fx().await;

    let result = cycle(&fx, WIDE).await;

    assert!(result.evaluations.first().unwrap().stopped_on_latency);
    assert!(!result.evaluations.first().unwrap().alerts.is_empty());
    assert!(result.alerts.is_empty());
    assert_eq!(result.dropped_after_reeligibility, 1);
    assert!(
        fx.cell.inner.lock().await.runnable_rules().is_empty(),
        "the report was applied under the second scope"
    );
}

// --- R20: lock scopes ------------------------------------------------------------------------------

#[tokio::test]
async fn a_cycle_with_no_rules_takes_one_lock_and_a_cycle_with_one_takes_two() {
    let fx = default_fx();
    let empty = cycle(&fx, WIDE).await;
    assert!(empty.alerts.is_empty() && empty.evaluations.is_empty());
    assert_eq!(fx.cell.lock_count(), 1);

    load(&fx, "r", NC_RULE).await;
    let before = fx.cell.lock_count();
    cycle(&fx, WIDE).await;
    assert_eq!(fx.cell.lock_count().saturating_sub(before), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_is_lockable_while_the_executor_runs() {
    let fx = heavy_fx().await;
    let free_while_executing = async {
        let mut seen = false;
        while fx.cell.lock_count() < 2 {
            if fx.cell.lock_count() == 1 && fx.cell.inner.try_lock().is_ok() {
                seen = true;
            }
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
        seen
    };

    let (_result, seen) = tokio::join!(cycle(&fx, WIDE), free_while_executing);

    assert!(
        seen,
        "no guard may be held between the first and second lock scopes"
    );
}

// --- R15 / R16: degraded conditions ---------------------------------------------------------------

async fn degraded(
    collection: Result<(), String>,
    heartbeat: HeartbeatStatus,
    ingest: IngestSnapshot,
) -> CycleResult {
    let fx = default_fx();
    put(&fx, vec![record(BASE_MS, 10, "nc")]);
    load(&fx, "r", NC_RULE).await;
    let signals = build_signals(
        PROCMOND_COLLECTOR_ID,
        collection,
        Some(heartbeat.collector_health()),
        ingest,
    );
    run_detection_cycle(&fx.cell, &fx.executor, WIDE, &signals).await
}

fn reasons(result: &CycleResult) -> Vec<CompletenessReason> {
    result
        .evaluations
        .first()
        .unwrap()
        .completeness
        .reasons()
        .to_vec()
}

fn unavailable() -> CompletenessReason {
    CompletenessReason::CollectorUnavailable {
        collector_id: PROCMOND_COLLECTOR_ID.to_owned(),
        table: "processes".to_owned(),
    }
}

#[tokio::test]
async fn a_failed_collection_marks_the_evaluation_and_the_rule_still_alerts() {
    let result = degraded(
        Err("rpc down".to_owned()),
        HeartbeatStatus::Healthy,
        IngestSnapshot::default(),
    )
    .await;
    assert!(reasons(&result).contains(&unavailable()));
    assert_eq!(alert_pids(&result), vec![10]);
    assert!(
        result
            .alerts
            .first()
            .unwrap()
            .completeness
            .reasons()
            .contains(&unavailable())
    );
}

#[tokio::test]
async fn a_failed_heartbeat_with_a_successful_collection_still_marks_the_evaluation() {
    let result = degraded(
        Ok(()),
        HeartbeatStatus::Failed {
            missed_count: 5,
            time_since_last: Duration::from_secs(90),
        },
        IngestSnapshot::default(),
    )
    .await;
    assert!(reasons(&result).contains(&unavailable()));
    assert_eq!(alert_pids(&result), vec![10]);
}

#[tokio::test]
async fn a_healthy_collector_leaves_the_evaluation_complete() {
    let result = degraded(Ok(()), HeartbeatStatus::Healthy, IngestSnapshot::default()).await;
    assert!(reasons(&result).is_empty());
}

#[tokio::test]
async fn a_degraded_but_not_failed_heartbeat_does_not_mark_the_evaluation() {
    let result = degraded(
        Ok(()),
        HeartbeatStatus::Degraded { missed_count: 1 },
        IngestSnapshot::default(),
    )
    .await;
    assert!(reasons(&result).is_empty());
}

#[tokio::test]
async fn an_ingest_sequence_gap_reaches_the_evaluation() {
    let gap = SequenceGap {
        collector_id: PROCMOND_COLLECTOR_ID.to_owned(),
        expected_seq: 3,
        observed_seq: 9,
    };
    let result = degraded(
        Ok(()),
        HeartbeatStatus::Healthy,
        IngestSnapshot {
            saturation_delta: 0,
            sequence_gaps: vec![gap],
            failure: None,
        },
    )
    .await;
    assert!(
        reasons(&result).contains(&CompletenessReason::SequenceGapDetected {
            collector_id: PROCMOND_COLLECTOR_ID.to_owned(),
            expected_seq: 3,
            observed_seq: 9,
        })
    );
}

#[test]
fn heartbeat_statuses_map_onto_collector_health() {
    assert_eq!(
        HeartbeatStatus::Healthy.collector_health(),
        CollectorHealth::Healthy
    );
    assert_eq!(
        HeartbeatStatus::Degraded { missed_count: 2 }.collector_health(),
        CollectorHealth::Degraded { missed_count: 2 }
    );
    assert_eq!(
        HeartbeatStatus::Failed {
            missed_count: 7,
            time_since_last: Duration::from_secs(1),
        }
        .collector_health(),
        CollectorHealth::Failed { missed_count: 7 }
    );
}

#[test]
fn a_collector_with_no_heartbeat_record_contributes_no_health() {
    let signals = build_signals(
        PROCMOND_COLLECTOR_ID,
        Ok(()),
        None,
        IngestSnapshot::default(),
    );
    assert!(signals.heartbeat.is_empty());
    assert_eq!(signals.collection.len(), 1);
}

// --- R22: cycle metrics -------------------------------------------------------------------------

const METRIC_FIELDS: [&str; 7] = [
    "rules_evaluated",
    "matches",
    "degraded_rules",
    "rows_scanned",
    "batches",
    "evaluation_ms",
    "max_pattern_latency_ms",
];

fn count_lines(lines: &[&str], level: &str, needle: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.contains(level) && line.contains(needle))
        .count()
}

#[tokio::test]
#[tracing_test::traced_test]
async fn metrics_one_cycle_over_two_rules_emits_exactly_one_info_event_with_all_seven_fields() {
    let fx = default_fx();
    put(
        &fx,
        vec![
            record(BASE_MS, 10, "nc"),
            record(BASE_MS + 1, 11, "nc"),
            record(BASE_MS + 2, 12, "bash"),
        ],
    );
    load(&fx, "r1", NC_RULE).await;
    load(&fx, "r2", "SELECT name FROM processes WHERE name = 'bash'").await;

    let result = cycle(&fx, WIDE).await;

    assert_eq!(result.evaluations.len(), 2);
    logs_assert(|lines: &[&str]| {
        let mut events = lines
            .iter()
            .filter(|line| line.contains("INFO") && line.contains("rules_evaluated"));
        let (Some(event), None) = (events.next(), events.next()) else {
            return Err("expected exactly one cycle-level info event".to_owned());
        };
        if let Some(missing) = METRIC_FIELDS.iter().find(|field| !event.contains(*field)) {
            return Err(format!("cycle event is missing field {missing}"));
        }
        for expected in ["rules_evaluated=2", "matches=3", "degraded_rules=0"] {
            if !event.contains(expected) {
                return Err(format!("cycle event lacks {expected}"));
            }
        }
        Ok(())
    });
}

#[tokio::test]
#[tracing_test::traced_test]
async fn metrics_each_rule_gets_a_debug_event_and_no_cycle_level_field_leaks_into_it() {
    let fx = default_fx();
    put(&fx, vec![record(BASE_MS, 10, "nc")]);
    load(&fx, "r1", NC_RULE).await;
    load(&fx, "r2", NC_RULE).await;

    cycle(&fx, WIDE).await;

    logs_assert(|lines: &[&str]| {
        let per_rule = count_lines(lines, "DEBUG", "degraded=false");
        if per_rule != 2 {
            return Err("expected one debug event per rule".to_owned());
        }
        let leaked = count_lines(lines, "DEBUG", "rules_evaluated");
        if leaked != 0 {
            return Err("a per-rule event carries a cycle-level field".to_owned());
        }
        Ok(())
    });
}

#[tokio::test]
#[tracing_test::traced_test]
async fn metrics_a_degraded_rule_warns_once_naming_the_rule_and_its_reason() {
    let fx = default_fx();
    put(&fx, vec![record(BASE_MS, 10, "nc")]);
    load(&fx, "r1", NC_RULE).await;
    let signals = build_signals(
        PROCMOND_COLLECTOR_ID,
        Err("rpc down".to_owned()),
        Some(CollectorHealth::Healthy),
        IngestSnapshot::default(),
    );

    run_detection_cycle(&fx.cell, &fx.executor, WIDE, &signals).await;

    logs_assert(|lines: &[&str]| {
        let warned = count_lines(lines, "WARN", "CollectorUnavailable");
        let naming_rule = count_lines(lines, "WARN", "r1");
        let cycle_line = count_lines(lines, "INFO", "degraded_rules=1");
        if warned != 1 || naming_rule != 1 || cycle_line != 1 {
            return Err(
                "expected one warn naming r1 and CollectorUnavailable, degraded_rules=1".to_owned(),
            );
        }
        Ok(())
    });
}

#[tokio::test]
#[tracing_test::traced_test]
async fn metrics_a_cycle_with_no_rules_emits_no_cycle_event() {
    let fx = default_fx();

    cycle(&fx, WIDE).await;

    logs_assert(|lines: &[&str]| {
        if count_lines(lines, "INFO", "rules_evaluated") == 0 {
            Ok(())
        } else {
            Err("an empty cycle must not report metrics".to_owned())
        }
    });
}
