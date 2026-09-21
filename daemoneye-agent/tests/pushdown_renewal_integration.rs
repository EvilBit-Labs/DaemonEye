//! The agent's renewal pass: what lands extends a task, what never lands expires it (R16).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Mutex;
use std::time::SystemTime;

use daemoneye_agent::pushdown_renewal::{TaskDispatch, run_renewal_cycle};
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
    let mut engine = engine();
    let dispatch = RecordingDispatch::new(true);

    let first = run_renewal_cycle(&mut engine, &dispatch, start).await;
    assert_eq!(first.landed.len(), 1, "the first pass issues the task");
    assert!(first.expired_rules.is_empty(), "nothing has lapsed yet");

    for round in 1_u32..=4 {
        let now = start + PUSHDOWN_TASK_RENEWAL_INTERVAL * round;
        let outcome = run_renewal_cycle(&mut engine, &dispatch, now).await;
        assert!(
            outcome.expired_rules.is_empty(),
            "round {round} let the task lapse"
        );
        assert_eq!(outcome.landed.len(), 1, "round {round} renewed nothing");
    }

    assert_eq!(
        engine.renewal_count(&task_id("rule-1", "procmond")),
        Some(5),
        "the issue and all four renewals landed and were counted"
    );
    assert_eq!(engine.active_task_count(), 1, "no duplicate task accrued");
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
    let mut engine = engine();
    let dispatch = RecordingDispatch::new(false);

    let first = run_renewal_cycle(&mut engine, &dispatch, start).await;
    assert_eq!(first.failed.len(), 1, "the issue did not reach a collector");

    let retry = run_renewal_cycle(
        &mut engine,
        &dispatch,
        start + PUSHDOWN_TASK_RENEWAL_INTERVAL,
    )
    .await;
    assert_eq!(retry.failed.len(), 1, "the interval offers a retry");
    assert!(retry.expired_rules.is_empty(), "the TTL has not elapsed");

    let lapsed = run_renewal_cycle(&mut engine, &dispatch, start + PUSHDOWN_TASK_TTL).await;
    assert_eq!(lapsed.expired_rules, ["rule-1"]);
    assert!(
        matches!(
            engine.rule_health("rule-1"),
            Some(&RuleHealth::Unhealthy { .. })
        ),
        "a rule whose pushed half stopped running is unhealthy, not silently covered"
    );
    assert_eq!(engine.active_task_count(), 0, "the lapsed task is retired");
}
