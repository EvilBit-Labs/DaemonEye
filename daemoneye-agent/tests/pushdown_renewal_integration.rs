//! The agent's renewal pass: what lands extends a task, what never lands expires it (R16).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::Mutex as AsyncMutex;

use daemoneye_agent::CollectorAdmission;
use daemoneye_agent::pushdown_renewal::{
    TaskDispatch, reissue_registered_collectors, run_renewal_cycle,
};
use daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore;
use daemoneye_eventbus::rpc::RegistrationRequest;
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::catalog::{VerifiedRegistration, verify_spawn_token};
use daemoneye_lib::detection::rule_health::RuleHealth;
use daemoneye_lib::detection::task_renewal::task_id;
use daemoneye_lib::detection_bounds::{PUSHDOWN_TASK_RENEWAL_INTERVAL, PUSHDOWN_TASK_TTL};
use daemoneye_lib::models::{AlertSeverity, DetectionRule};
use daemoneye_lib::proto::{
    ColumnDescriptor, ColumnType, DetectionTask, PredicateOp, SchemaDescriptor, TableDescriptor,
};

/// A dispatcher that records every task it was handed and answers as configured.
struct RecordingDispatch {
    accepts: bool,
    sent: Mutex<Vec<(String, String)>>,
}

impl RecordingDispatch {
    const fn new(accepts: bool) -> Self {
        Self {
            accepts,
            sent: Mutex::new(Vec::new()),
        }
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl TaskDispatch for RecordingDispatch {
    async fn send_task(&self, collector_id: &str, task: DetectionTask) -> Result<(), String> {
        self.sent
            .lock()
            .unwrap()
            .push((collector_id.to_owned(), task.task_id.clone()));
        assert!(
            task.pushdown_plan.is_some(),
            "a renewal carries the plan and the TTL it was issued under"
        );
        if self.accepts {
            return Ok(());
        }
        Err("collector unreachable".to_owned())
    }
}

/// A registration carrying no descriptor: the collector is already in the catalog, and what this
/// exercises is the re-issue its re-registration owes, not a schema change.
fn registration(collector_id: &str, token: String) -> RegistrationRequest {
    RegistrationRequest {
        collector_id: collector_id.to_owned(),
        collector_type: "procmond".to_owned(),
        hostname: "localhost".to_owned(),
        version: Some("1.0.0".to_owned()),
        pid: Some(1001),
        capabilities: vec![],
        attributes: HashMap::new(),
        heartbeat_interval_ms: Some(10_000),
        descriptor: None,
        spawn_token: Some(token),
    }
}

fn verified(collector_id: &str) -> VerifiedRegistration {
    let token = "a".repeat(64);
    verify_spawn_token(collector_id, Some(&token), Some(&token)).unwrap()
}

fn descriptor() -> SchemaDescriptor {
    let ops = [PredicateOp::Eq, PredicateOp::Gt, PredicateOp::Lt];
    SchemaDescriptor {
        collector_id: "procmond".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![ColumnDescriptor {
                name: "cpu_usage".to_owned(),
                column_type: i32::from(ColumnType::Int),
                nullable: false,
                supported_ops: ops.iter().copied().map(i32::from).collect(),
            }],
        }],
        conformance_results: Vec::new(),
    }
}

fn engine() -> DetectionEngine {
    let mut engine = DetectionEngine::new();
    engine
        .register_collector(&verified("procmond"), descriptor())
        .unwrap();
    engine
        .load_rule(DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Renewal test rule".to_owned(),
            "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        ))
        .unwrap();
    engine
}

#[tokio::test]
async fn renewals_that_land_keep_the_task_alive_across_cycles() {
    let start = SystemTime::UNIX_EPOCH;
    let engine = AsyncMutex::new(engine());
    let dispatch = RecordingDispatch::new(true);

    let first = run_renewal_cycle(&engine, &dispatch, start).await;
    assert_eq!(first.landed.len(), 1, "the first pass issues the task");
    assert!(first.expired_rules.is_empty(), "nothing has lapsed yet");

    for round in 1_u32..=4 {
        let now = start + PUSHDOWN_TASK_RENEWAL_INTERVAL * round;
        let outcome = run_renewal_cycle(&engine, &dispatch, now).await;
        assert!(
            outcome.expired_rules.is_empty(),
            "round {round} let the task lapse"
        );
        assert_eq!(outcome.landed.len(), 1, "round {round} renewed nothing");
    }

    assert_eq!(
        engine
            .lock()
            .await
            .renewal_count(&task_id("rule-1", "procmond")),
        Some(5),
        "the issue and all four renewals landed and were counted"
    );
    assert_eq!(
        engine.lock().await.active_task_count(),
        1,
        "no duplicate task accrued"
    );
    let sent = dispatch.sent();
    assert_eq!(sent.len(), 5, "one issue plus four renewals were sent");
    assert!(
        sent.iter()
            .all(|entry| entry.0 == "procmond" && entry.1 == task_id("rule-1", "procmond")),
        "every send carries the same derived task id"
    );
}

#[tokio::test]
async fn a_renewal_that_never_reaches_its_collector_expires_the_task_and_marks_the_rule() {
    let start = SystemTime::UNIX_EPOCH;
    let engine = AsyncMutex::new(engine());
    let dispatch = RecordingDispatch::new(false);

    let first = run_renewal_cycle(&engine, &dispatch, start).await;
    assert_eq!(first.failed.len(), 1, "the issue did not reach a collector");

    let retry = run_renewal_cycle(&engine, &dispatch, start + PUSHDOWN_TASK_RENEWAL_INTERVAL).await;
    assert_eq!(retry.failed.len(), 1, "the interval offers a retry");
    assert!(retry.expired_rules.is_empty(), "the TTL has not elapsed");

    let lapsed = run_renewal_cycle(&engine, &dispatch, start + PUSHDOWN_TASK_TTL).await;
    assert_eq!(lapsed.expired_rules, ["rule-1"]);
    assert!(
        matches!(
            engine.lock().await.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy { .. })
        ),
        "a rule whose pushed half stopped running is unhealthy, not silently covered"
    );
    assert_eq!(
        engine.lock().await.active_task_count(),
        0,
        "the lapsed task is retired"
    );
}

#[tokio::test]
async fn a_collector_registration_re_issues_its_active_task_set_without_duplicating_it() {
    // R16: a collector that registers again has lost its accepted-task map, so the agent re-sends
    // every task addressed to it. Identifiers are derived from the rule and the collector, so the
    // re-issue overwrites in place instead of accumulating a second task.
    let start = SystemTime::UNIX_EPOCH;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SpawnTokenStore::new(dir.path()).unwrap());
    let engine = Arc::new(AsyncMutex::new(engine()));
    let admission = CollectorAdmission::new(Arc::clone(&store), Arc::clone(&engine));
    let dispatch = RecordingDispatch::new(true);

    // The first renewal pass issues the rule's task once.
    let first = run_renewal_cycle(&engine, &dispatch, start).await;
    assert_eq!(first.landed, [task_id("rule-1", "procmond")]);

    // Act: the collector registers through the real admission path, which records it as owed a
    // re-issue; the renewal loop drains that set on its next tick.
    let _issued = store.issue("procmond").unwrap();
    let token = store.expected_token("procmond").unwrap();
    admission
        .admit(&registration("procmond", token))
        .await
        .unwrap();
    let reissued = reissue_registered_collectors(&engine, &admission, &dispatch, start).await;

    // Assert
    assert_eq!(
        reissued.landed,
        [task_id("rule-1", "procmond")],
        "the collector's whole active set is re-issued"
    );
    assert_eq!(
        engine.lock().await.active_task_count(),
        1,
        "a re-issue overwrites the task in place rather than accumulating a duplicate"
    );
    assert_eq!(
        dispatch.sent().len(),
        2,
        "the task was sent once on issue and once on re-issue"
    );
    assert!(
        admission.drain_pending_reissue().await.is_empty(),
        "a drained re-issue is not owed twice"
    );
}
