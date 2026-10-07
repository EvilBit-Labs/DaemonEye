//! Completeness of executor evaluations (U10; R14, R15, R16 reason half; AE1, AE8, AE9 model half).
//!
//! Every assertion names the discriminating field of a reason (collector, table, sequence), never
//! only its variant: `matches!(reason, CollectorUnavailable { .. })` is true for the wrong
//! collector and the wrong table.
#![cfg(feature = "detection-engine")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use daemoneye_lib::config::DetectionConfig;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::verify_spawn_token;
use daemoneye_lib::detection::execution::completeness::{
    CollectorHealth, CycleSignals, EvaluationSummary, IngestSnapshot, SequenceGap,
};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::{CycleOutcome, RuleEvaluation, RuleExecutor};
use daemoneye_lib::models::{
    AlertSeverity, CompletenessReason, CompletenessStatus, DetectionRule, ProcessRecord,
};
use daemoneye_lib::proto::{ColumnDescriptor, ColumnType, SchemaDescriptor, TableDescriptor};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::IngestRecord;
use tempfile::TempDir;

const HOUR: u64 = 3_600_000;
const WIDE: CycleWindow = CycleWindow {
    after_ms: 0,
    through_ms: 100 * HOUR,
};

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
    let store = Arc::new(EventStore::new(dir.path().join("completeness.redb")).unwrap());
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
        pattern_latency_threshold_ms: DetectionConfig::PATTERN_LATENCY_THRESHOLD_MS_MAX,
        ..DetectionConfig::default()
    }
}

/// `count` rows named `name`, pids `1..=count`.
fn put_named(fx: &Fixture, name: &str, count: u32) {
    let batch: Vec<IngestRecord> = (1..=count)
        .map(|pid| {
            let mut record = ProcessRecord::new(pid, name.to_owned());
            let ts_ms = 10 * HOUR + u64::from(pid);
            record.collection_time = Utc
                .timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
                .unwrap();
            IngestRecord::new("test", u64::from(pid), pid, record).unwrap()
        })
        .collect();
    fx.store.put_batch(&batch).unwrap();
}

fn load(fx: &mut Fixture, id: &str, sql: &str) {
    let rule = DetectionRule::new(
        id.to_owned(),
        format!("Rule {id}"),
        "completeness fixture".to_owned(),
        sql.to_owned(),
        "test".to_owned(),
        AlertSeverity::High,
    );
    fx.engine.load_rule(rule).unwrap();
}

async fn run_with(fx: &Fixture, signals: &CycleSignals) -> CycleOutcome {
    RuleExecutor::new(Arc::clone(&fx.store), fx.engine.regex_cache(), &fx.config)
        .unwrap()
        .evaluate(&fx.engine.runnable_rules(), WIDE, signals)
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

fn healthy() -> CycleSignals {
    CycleSignals {
        collection: [("procmond".to_owned(), Ok(()))].into(),
        heartbeat: [("procmond".to_owned(), CollectorHealth::Healthy)].into(),
        ingest: IngestSnapshot::default(),
    }
}

fn unavailable() -> CompletenessReason {
    CompletenessReason::CollectorUnavailable {
        collector_id: "procmond".to_owned(),
        table: "processes".to_owned(),
    }
}

#[tokio::test]
async fn completeness_ae1_healthy_control_is_complete_and_failed_collection_is_not() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 3);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");

    let control_outcome = run_with(&fx, &healthy()).await;
    let control = only(&control_outcome);
    assert!(control.alerts.is_empty());
    assert_eq!(control.completeness.status(), CompletenessStatus::Complete);
    assert!(control.completeness.reasons().is_empty());

    let mut failed = healthy();
    failed.collection.insert(
        "procmond".to_owned(),
        Err("task rpc returned success=false".to_owned()),
    );
    let degraded_outcome = run_with(&fx, &failed).await;
    let degraded = only(&degraded_outcome);
    assert!(degraded.alerts.is_empty());
    assert_eq!(degraded.completeness.status(), CompletenessStatus::Degraded);
    let unavailable_count = degraded
        .completeness
        .reasons()
        .iter()
        .filter(|reason| **reason == unavailable())
        .count();
    assert_eq!(unavailable_count, 1);
    assert!(!control.completeness.reasons().contains(&unavailable()));
}

#[tokio::test]
async fn completeness_failed_collection_still_evaluates_what_is_stored_and_alerts_carry_it() {
    let mut fx = fixture(config());
    put_named(&fx, "nc", 2);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut failed = healthy();
    failed
        .collection
        .insert("procmond".to_owned(), Err("boom".to_owned()));

    let outcome = run_with(&fx, &failed).await;

    let evaluation = only(&outcome);
    assert_eq!(evaluation.alerts.len(), 2);
    assert!(
        evaluation
            .alerts
            .iter()
            .all(|alert| alert.completeness == evaluation.completeness)
    );
    assert_eq!(
        evaluation.completeness.reasons(),
        [
            unavailable(),
            CompletenessReason::CollectionFailed {
                collector_id: "procmond".to_owned(),
                error: "boom".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn completeness_failed_heartbeat_for_another_collector_does_not_degrade_this_table() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut signals = healthy();
    signals.heartbeat.insert(
        "other".to_owned(),
        CollectorHealth::Failed { missed_count: 3 },
    );

    let outcome = run_with(&fx, &signals).await;

    assert_eq!(
        only(&outcome).completeness.status(),
        CompletenessStatus::Complete
    );
    assert!(only(&outcome).completeness.reasons().is_empty());
}

#[tokio::test]
async fn completeness_failed_heartbeat_for_the_owner_degrades_without_a_collection_error() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut signals = healthy();
    signals.heartbeat.insert(
        "procmond".to_owned(),
        CollectorHealth::Failed { missed_count: 3 },
    );

    let outcome = run_with(&fx, &signals).await;

    assert_eq!(only(&outcome).completeness.reasons(), [unavailable()]);
}

#[tokio::test]
async fn completeness_sequence_gap_degrades_only_the_gapped_collectors_table() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let gap = |collector: &str| SequenceGap {
        collector_id: collector.to_owned(),
        expected_seq: 11,
        observed_seq: 13,
    };
    let mut other = healthy();
    other.ingest.sequence_gaps = vec![gap("other")];
    let mut ours = healthy();
    ours.ingest.sequence_gaps = vec![gap("procmond")];

    let unaffected = run_with(&fx, &other).await;
    let affected = run_with(&fx, &ours).await;

    assert!(only(&unaffected).completeness.reasons().is_empty());
    assert_eq!(
        only(&affected).completeness.reasons(),
        [CompletenessReason::SequenceGapDetected {
            collector_id: "procmond".to_owned(),
            expected_seq: 11,
            observed_seq: 13,
        }]
    );
}

#[tokio::test]
async fn completeness_ingest_backpressure_sheds_with_the_discarded_count() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut signals = healthy();
    signals.ingest.saturation_delta = 2;

    let outcome = run_with(&fx, &signals).await;

    assert_eq!(
        only(&outcome).completeness.reasons(),
        [CompletenessReason::Shed { discarded: 2 }]
    );
}

#[tokio::test]
async fn completeness_execution_error_follows_the_collector_reasons_in_a_stable_order() {
    let mut fx = fixture(config());
    put_named(&fx, "nc", 2);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut rules = fx.engine.runnable_rules();
    rules[0]
        .descriptor
        .columns
        .push(column("not_a_real_column", ColumnType::String));
    let mut failed = healthy();
    failed
        .collection
        .insert("procmond".to_owned(), Err("boom".to_owned()));

    let outcome = RuleExecutor::new(Arc::clone(&fx.store), fx.engine.regex_cache(), &fx.config)
        .unwrap()
        .evaluate(&rules, WIDE, &failed)
        .await;

    let reasons = only(&outcome).completeness.reasons();
    assert_eq!(reasons.len(), 3);
    assert_eq!(reasons[0], unavailable());
    assert!(matches!(
        &reasons[1],
        CompletenessReason::CollectionFailed { collector_id, error }
            if collector_id == "procmond" && error == "boom"
    ));
    assert!(matches!(
        &reasons[2],
        CompletenessReason::ExecutionError { detail } if detail.contains("not_a_real_column")
    ));
}

#[tokio::test]
async fn completeness_result_cap_degrades_and_names_the_cap() {
    let mut fx = fixture(DetectionConfig {
        max_matches_per_rule: 3,
        ..config()
    });
    put_named(&fx, "nc", 5);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");

    let outcome = run_with(&fx, &healthy()).await;

    let evaluation = only(&outcome);
    assert_eq!(evaluation.alerts.len(), 3);
    assert_eq!(
        evaluation.completeness.reasons(),
        [CompletenessReason::ResultCapped { cap: 3 }]
    );
}

#[tokio::test]
async fn completeness_oversized_row_degrades_as_a_resource_limit_naming_the_table() {
    let mut fx = fixture(config());
    let mut big = ProcessRecord::new(2, "nc".to_owned());
    big.command_line = Some("y".repeat(5 * 1024 * 1024));
    big.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(10 * HOUR + 2).unwrap())
        .unwrap();
    let batch = [IngestRecord::new("test", 1, 0, big).unwrap()];
    fx.store.put_batch(&batch).unwrap();
    load(
        &mut fx,
        "r1",
        "SELECT command_line FROM processes WHERE name = 'nc'",
    );

    let outcome = run_with(&fx, &healthy()).await;

    let reasons = only(&outcome).completeness.reasons();
    assert_eq!(reasons.len(), 1);
    assert!(matches!(
        &reasons[0],
        CompletenessReason::ResourceLimit { detail } if detail.contains("processes")
    ));
}

#[tokio::test]
async fn completeness_alert_names_the_pid_even_when_the_rule_did_not_select_it() {
    let mut fx = fixture(config());
    put_named(&fx, "nc", 3);
    load(
        &mut fx,
        "r1",
        "SELECT name FROM processes WHERE name = 'nc'",
    );

    let outcome = run_with(&fx, &healthy()).await;

    let mut pids: Vec<u32> = only(&outcome)
        .alerts
        .iter()
        .map(|alert| alert.process_record.pid.raw())
        .collect();
    pids.sort_unstable();
    assert_eq!(pids, vec![1, 2, 3]);
}

#[tokio::test]
async fn completeness_survives_a_store_round_trip_with_variant_and_fields() {
    let mut fx = fixture(config());
    put_named(&fx, "nc", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    let mut signals = healthy();
    signals
        .collection
        .insert("procmond".to_owned(), Err("boom".to_owned()));
    let outcome = run_with(&fx, &signals).await;
    let alert = only(&outcome).alerts[0].clone();
    assert_eq!(alert.completeness.reasons().len(), 2);

    fx.store.store_alert(&alert).unwrap();
    let fetched = fx
        .store
        .get_alert(&alert.id.to_string())
        .unwrap()
        .expect("alert present");

    assert_eq!(fetched.completeness, alert.completeness);
    assert_eq!(fetched, alert);
}

#[tokio::test]
async fn completeness_last_evaluation_is_kept_per_rule_and_dropped_with_the_rule() {
    let mut fx = fixture(config());
    put_named(&fx, "bash", 1);
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    assert!(fx.engine.last_evaluation("r1").is_none());
    let mut failed = healthy();
    failed
        .collection
        .insert("procmond".to_owned(), Err("boom".to_owned()));
    let outcome = run_with(&fx, &failed).await;
    let evaluation = only(&outcome);

    fx.engine
        .record_evaluation(EvaluationSummary::of(evaluation));

    let summary = fx.engine.last_evaluation("r1").expect("recorded");
    assert_eq!(summary.completeness, evaluation.completeness);
    assert_eq!(summary.generation, evaluation.generation);
    assert_eq!(summary.alert_count, 0);

    let stale = EvaluationSummary {
        alert_count: 99,
        ..EvaluationSummary::of(evaluation)
    };
    load(&mut fx, "r1", "SELECT pid FROM processes WHERE name = 'nc'");
    fx.engine.record_evaluation(stale);
    assert_eq!(
        fx.engine.last_evaluation("r1").map(|s| s.alert_count),
        Some(0),
        "a result for a superseded generation does not replace the kept one"
    );

    fx.engine.remove_rule("r1");
    assert!(fx.engine.last_evaluation("r1").is_none());
    fx.engine
        .record_evaluation(EvaluationSummary::of(evaluation));
    assert!(fx.engine.last_evaluation("r1").is_none());
}
