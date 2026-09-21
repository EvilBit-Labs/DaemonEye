//! Collector-side acceptance and validation of pushed detection tasks (R19).
//!
//! A collector refuses any task naming a column or operation it did not advertise, and stops
//! evaluating a task once its TTL expires without renewal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use collector_core::{
    CollectionEvent, EventSource, PushdownRejection, PushdownTasks, SourceCaps, TaskStatus,
};
use daemoneye_eventbus::rpc::{
    ColumnDescriptor, ColumnType, PredicateOp as DescriptorOp, SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::proto;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

const TTL_MS: u64 = 60_000;

fn descriptor() -> SchemaDescriptor {
    SchemaDescriptor {
        collector_id: "test-collector".to_owned(),
        descriptor_version: "v1".to_owned(),
        tables: vec![TableDescriptor {
            name: "processes".to_owned(),
            columns: vec![
                ColumnDescriptor {
                    name: "pid".to_owned(),
                    column_type: ColumnType::Uint,
                    nullable: false,
                    supported_ops: vec![DescriptorOp::Eq, DescriptorOp::Gt],
                },
                ColumnDescriptor {
                    name: "name".to_owned(),
                    column_type: ColumnType::String,
                    nullable: false,
                    supported_ops: vec![DescriptorOp::Eq],
                },
            ],
        }],
        conformance_results: Vec::new(),
    }
}

const fn literal_uint(value: u64) -> proto::Literal {
    proto::Literal {
        value: Some(proto::literal::Value::UintValue(value)),
    }
}

fn task(
    task_id: &str,
    predicates: Vec<proto::Predicate>,
    projection: Vec<String>,
) -> proto::DetectionTask {
    proto::DetectionTask {
        task_id: task_id.to_owned(),
        pushdown_plan: Some(proto::PushdownPlan {
            table: "processes".to_owned(),
            predicates,
            projection,
            ttl_ms: TTL_MS,
        }),
        ..Default::default()
    }
}

fn predicate(
    column: &str,
    op: proto::PredicateOp,
    values: Vec<proto::Literal>,
) -> proto::Predicate {
    proto::Predicate {
        column: column.to_owned(),
        op: i32::from(op),
        values,
    }
}

/// A source that advertises the descriptor above and routes acceptance through it.
struct AdvertisingSource {
    tasks: PushdownTasks,
}

#[async_trait::async_trait]
impl EventSource for AdvertisingSource {
    fn name(&self) -> &'static str {
        "advertising"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS
    }

    async fn start(
        &self,
        _tx: mpsc::Sender<CollectionEvent>,
        _shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn accept_pushdown_task(
        &self,
        task: &proto::DetectionTask,
        now: SystemTime,
    ) -> Result<(), PushdownRejection> {
        self.tasks.accept(task, now)
    }
}

/// A source that implements only the four required methods and never mentions pushdown.
struct BareSource;

#[async_trait::async_trait]
impl EventSource for BareSource {
    fn name(&self) -> &'static str {
        "bare"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS
    }

    async fn start(
        &self,
        _tx: mpsc::Sender<CollectionEvent>,
        _shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn advertising() -> AdvertisingSource {
    AdvertisingSource {
        tasks: PushdownTasks::new(descriptor()),
    }
}

#[test]
fn accepts_a_task_naming_an_advertised_column_and_operation() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-accept",
        vec![predicate(
            "pid",
            proto::PredicateOp::Gt,
            vec![literal_uint(1)],
        )],
        vec!["pid".to_owned(), "name".to_owned()],
    );

    assert_eq!(source.accept_pushdown_task(&task, now), Ok(()));
    assert_eq!(source.tasks.status("task-accept", now), TaskStatus::Active);
}

#[test]
fn rejects_a_task_naming_an_unadvertised_column_without_partial_evaluation() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    // The first predicate is valid; the second names a column the collector never advertised.
    let task = task(
        "task-unknown-column",
        vec![
            predicate("pid", proto::PredicateOp::Eq, vec![literal_uint(1)]),
            predicate("ppid", proto::PredicateOp::Eq, vec![literal_uint(2)]),
        ],
        vec!["pid".to_owned()],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnknownColumn {
            table: "processes".to_owned(),
            column: "ppid".to_owned(),
        })
    );
    // Nothing from the task is retained: the valid half was not kept for evaluation.
    assert_eq!(
        source.tasks.status("task-unknown-column", now),
        TaskStatus::Unknown
    );
    assert_eq!(source.tasks.active_count(now), 0);
}

#[test]
fn rejects_a_task_naming_an_unadvertised_projection_column() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-projection",
        vec![],
        vec!["executable_path".to_owned()],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnknownColumn {
            table: "processes".to_owned(),
            column: "executable_path".to_owned(),
        })
    );
    assert_eq!(source.tasks.active_count(now), 0);
}

#[test]
fn rejects_an_operation_the_column_did_not_advertise() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-bad-op",
        vec![predicate(
            "name",
            proto::PredicateOp::Regexp,
            vec![proto::Literal {
                value: Some(proto::literal::Value::StringValue("^a".to_owned())),
            }],
        )],
        vec![],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnsupportedOperation {
            column: "name".to_owned(),
            op: "PREDICATE_OP_REGEXP".to_owned(),
        })
    );
    assert_eq!(source.tasks.active_count(now), 0);
}

#[test]
fn rejects_an_unspecified_operation() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-unspecified-op",
        vec![predicate(
            "pid",
            proto::PredicateOp::Unspecified,
            vec![literal_uint(1)],
        )],
        vec![],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnspecifiedOperation {
            column: "pid".to_owned(),
        })
    );
}

#[test]
fn rejects_an_operation_value_newer_than_this_build() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let mut task = task(
        "task-future-op",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_uint(1)],
        )],
        vec![],
    );
    if let Some(predicate) = task
        .pushdown_plan
        .as_mut()
        .and_then(|plan| plan.predicates.first_mut())
    {
        predicate.op = 9999;
    }

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnspecifiedOperation {
            column: "pid".to_owned(),
        })
    );
}

#[test]
fn rejects_a_table_the_collector_does_not_serve() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let mut task = task("task-bad-table", vec![], vec![]);
    if let Some(plan) = task.pushdown_plan.as_mut() {
        plan.table = "sockets".to_owned();
    }

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::UnknownTable {
            table: "sockets".to_owned(),
        })
    );
}

#[test]
fn rejects_a_predicate_with_the_wrong_value_count() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-arity",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_uint(1), literal_uint(2)],
        )],
        vec![],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::InvalidArity {
            column: "pid".to_owned(),
            op: "PREDICATE_OP_EQ".to_owned(),
            values: 2,
        })
    );
}

#[test]
fn rejects_a_task_carrying_no_plan() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = proto::DetectionTask {
        task_id: "task-no-plan".to_owned(),
        ..Default::default()
    };

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::MissingPlan)
    );
}

#[test]
fn rejects_a_task_carrying_no_id() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let task = task("", vec![], vec!["pid".to_owned()]);

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::MissingTaskId)
    );
    assert_eq!(source.tasks.active_count(now), 0);
}

#[test]
fn rejects_a_zero_ttl_rather_than_reading_it_as_no_expiry() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let mut task = task("task-zero-ttl", vec![], vec![]);
    if let Some(plan) = task.pushdown_plan.as_mut() {
        plan.ttl_ms = 0;
    }

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::InvalidTtl { ttl_ms: 0 })
    );
    assert_eq!(source.tasks.active_count(now), 0);
}

#[test]
fn rejects_a_ttl_beyond_the_fixed_ceiling() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let over =
        u64::try_from(daemoneye_lib::detection_bounds::PUSHDOWN_TASK_TTL.as_millis()).unwrap() + 1;
    let mut task = task("task-long-ttl", vec![], vec![]);
    if let Some(plan) = task.pushdown_plan.as_mut() {
        plan.ttl_ms = over;
    }

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::InvalidTtl { ttl_ms: over })
    );
}

#[test]
fn rejects_an_over_long_identifier() {
    let source = advertising();
    let now = SystemTime::UNIX_EPOCH;
    let long = "x".repeat(daemoneye_lib::detection_bounds::MAX_IDENTIFIER_LENGTH + 1);
    let task = task("task-long-ident", vec![], vec![long.clone()]);

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::IdentifierTooLong { length: long.len() })
    );
}

#[test]
fn an_accepted_task_transitions_to_expired_once_its_ttl_elapses() {
    let source = advertising();
    let accepted_at = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-ttl",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_uint(7)],
        )],
        vec!["pid".to_owned()],
    );

    assert_eq!(source.accept_pushdown_task(&task, accepted_at), Ok(()));
    assert_eq!(
        source.tasks.status("task-ttl", accepted_at),
        TaskStatus::Active
    );
    assert_eq!(source.tasks.active_count(accepted_at), 1);

    let just_before = accepted_at + Duration::from_millis(TTL_MS - 1);
    assert_eq!(
        source.tasks.status("task-ttl", just_before),
        TaskStatus::Active
    );

    let after = accepted_at + Duration::from_millis(TTL_MS + 1);
    assert_eq!(source.tasks.status("task-ttl", after), TaskStatus::Expired);
    assert_eq!(source.tasks.active_count(after), 0);
}

#[test]
fn a_source_that_does_not_override_the_defaulted_method_rejects_every_task() {
    let source = BareSource;
    let now = SystemTime::UNIX_EPOCH;
    let task = task(
        "task-bare",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_uint(1)],
        )],
        vec!["pid".to_owned()],
    );

    assert_eq!(
        source.accept_pushdown_task(&task, now),
        Err(PushdownRejection::PushdownUnsupported {
            event_source: "bare"
        })
    );
}
