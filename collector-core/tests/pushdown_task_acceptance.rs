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
use daemoneye_lib::detection::RegexRejection;
use daemoneye_lib::detection_bounds::{MAX_IN_VALUES, MAX_PREDICATES_PER_PLAN};
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

/// Covers AE7.
///
/// What this proves: a task whose second predicate names a column the collector never
/// advertised is refused with `PushdownRejection::UnknownColumn`, and nothing from it is kept —
/// `status` stays `Unknown` and `active_count` is `0`. `PushdownTasks::accept` calls `record`
/// only after every predicate and the whole plan validate, so a task that fails partway can
/// never leave a partial plan behind for a later evaluator to run.
///
/// What it does NOT prove: it does not run any records through an evaluator to show no rows
/// came out. That is unreachable by construction here — with nothing recorded under this task
/// id, no evaluator has a plan to run — so no such assertion is needed.
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

/// A plan carrying more predicates than the fixed bound is refused whole.
#[test]
fn a_plan_beyond_the_predicate_bound_is_refused() {
    // Arrange
    let tasks = PushdownTasks::new(descriptor());
    let predicates = (0..=MAX_PREDICATES_PER_PLAN)
        .map(|_ordinal| predicate("pid", proto::PredicateOp::Eq, vec![literal_uint(1)]))
        .collect();
    let oversized = task("too-many", predicates, vec!["pid".to_owned()]);

    // Act
    let refusal = tasks.accept(&oversized, SystemTime::now());

    // Assert
    assert!(
        matches!(
            refusal,
            Err(PushdownRejection::PlanTooLarge {
                what: "predicates",
                ..
            })
        ),
        "an unbounded predicate count must be refused, got {refusal:?}"
    );
    assert_eq!(
        tasks.status("too-many", SystemTime::now()),
        TaskStatus::Unknown
    );
}

/// An `IN` list beyond the fixed bound is refused; its values are scanned per record.
#[test]
fn an_in_list_beyond_the_value_bound_is_refused() {
    // Arrange
    let tasks = PushdownTasks::new(in_capable_descriptor());
    let values = (0..=MAX_IN_VALUES)
        .map(|ordinal| literal_uint(u64::try_from(ordinal).expect("an ordinal below the bound")))
        .collect();
    let oversized = task(
        "wide-in",
        vec![predicate("pid", proto::PredicateOp::In, values)],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&oversized, SystemTime::now());

    // Assert
    assert!(
        matches!(
            refusal,
            Err(PushdownRejection::PlanTooLarge {
                what: "IN values",
                ..
            })
        ),
        "an unbounded IN list must be refused, got {refusal:?}"
    );
}

/// The descriptor above, with `pid` additionally advertising `IN`.
fn in_capable_descriptor() -> SchemaDescriptor {
    let mut schema = descriptor();
    for table in &mut schema.tables {
        for column in &mut table.columns {
            if column.name == "pid" {
                column.supported_ops.push(DescriptorOp::In);
            }
        }
    }
    schema
}

/// The descriptor above, with `name` additionally advertising `REGEXP`.
fn regexp_capable_descriptor() -> SchemaDescriptor {
    let mut schema = descriptor();
    for table in &mut schema.tables {
        for column in &mut table.columns {
            if column.name == "name" {
                column.supported_ops.push(DescriptorOp::Regexp);
            }
        }
    }
    schema
}

fn literal_string(value: &str) -> proto::Literal {
    proto::Literal {
        value: Some(proto::literal::Value::StringValue(value.to_owned())),
    }
}

const fn literal_null() -> proto::Literal {
    proto::Literal {
        value: Some(proto::literal::Value::NullValue(true)),
    }
}

/// The SDK path itself refuses a literal whose kind the column never declared, with no
/// collector-specific code involved.
#[test]
fn rejects_a_literal_whose_kind_the_column_did_not_declare() {
    // Arrange: `pid` is declared `UINT`.
    let tasks = PushdownTasks::new(descriptor());
    let mistyped = task(
        "mistyped",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_string("10")],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&mistyped, SystemTime::UNIX_EPOCH);

    // Assert
    assert_eq!(
        refusal,
        Err(PushdownRejection::LiteralTypeMismatch {
            column: "pid".to_owned(),
            column_type: ColumnType::Uint.as_wire_name().to_owned(),
            literal_kind: "string".to_owned(),
        })
    );
    assert_eq!(tasks.active_count(SystemTime::UNIX_EPOCH), 0);
}

/// A NULL literal can never match a column declared `NOT NULL`, so the plan is a static defect.
#[test]
fn rejects_a_null_literal_against_a_non_nullable_column() {
    // Arrange
    let tasks = PushdownTasks::new(descriptor());
    let impossible = task(
        "null-literal",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![literal_null()],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&impossible, SystemTime::UNIX_EPOCH);

    // Assert
    assert_eq!(
        refusal,
        Err(PushdownRejection::NullLiteralOnNonNullable {
            column: "pid".to_owned(),
        })
    );
}

/// An empty literal oneof carries no value at all, and is refused rather than read as some default.
#[test]
fn rejects_a_literal_carrying_an_empty_oneof() {
    // Arrange
    let tasks = PushdownTasks::new(descriptor());
    let empty = task(
        "empty-oneof",
        vec![predicate(
            "pid",
            proto::PredicateOp::Eq,
            vec![proto::Literal { value: None }],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&empty, SystemTime::UNIX_EPOCH);

    // Assert
    assert_eq!(
        refusal,
        Err(PushdownRejection::LiteralTypeMismatch {
            column: "pid".to_owned(),
            column_type: ColumnType::Uint.as_wire_name().to_owned(),
            literal_kind: "unset".to_owned(),
        })
    );
}

/// A pattern beyond the shared compile ceiling is refused by the SDK, and never becomes resident.
#[test]
fn rejects_a_pattern_beyond_the_shared_compile_bounds() {
    // Arrange: a pattern whose compiled program cannot fit the fixed byte ceiling.
    let tasks = PushdownTasks::new(regexp_capable_descriptor());
    let pattern = format!("(?:{}){{4096}}", "[0-9a-f]".repeat(64));
    let oversized = task(
        "oversized-pattern",
        vec![predicate(
            "name",
            proto::PredicateOp::Regexp,
            vec![literal_string(&pattern)],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&oversized, SystemTime::UNIX_EPOCH);

    // Assert
    assert!(
        matches!(
            refusal,
            Err(PushdownRejection::PatternRejected {
                ref column,
                rejection: RegexRejection::CompiledTooBig { .. },
            }) if column == "name"
        ),
        "expected an over-bounds pattern rejection, got {refusal:?}"
    );
    assert!(
        !tasks.patterns().is_cached(&pattern),
        "an over-bounds pattern must never become resident"
    );
    assert_eq!(tasks.patterns().len(), 0);
}

/// A NULL literal is never a pattern, whatever the column's nullability allows.
#[test]
fn rejects_a_null_literal_pushed_as_a_pattern() {
    // Arrange: `name` is `STRING` and advertises `REGEXP`.
    let mut schema = regexp_capable_descriptor();
    for table in &mut schema.tables {
        for column in &mut table.columns {
            if column.name == "name" {
                column.nullable = true;
            }
        }
    }
    let tasks = PushdownTasks::new(schema);
    let nulled = task(
        "null-pattern",
        vec![predicate(
            "name",
            proto::PredicateOp::Regexp,
            vec![literal_null()],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&nulled, SystemTime::UNIX_EPOCH);

    // Assert
    assert_eq!(
        refusal,
        Err(PushdownRejection::LiteralTypeMismatch {
            column: "name".to_owned(),
            column_type: ColumnType::String.as_wire_name().to_owned(),
            literal_kind: "null".to_owned(),
        })
    );
    assert_eq!(tasks.patterns().stats().compiles, 0);
}

/// A plan beyond the predicate bound is refused before any of its patterns reaches the cache.
///
/// The cache is fixed-size, so a compile on behalf of a plan that is refused anyway can evict a
/// live task's resident pattern and force it to be recompiled on the next evaluation.
#[test]
fn a_plan_beyond_the_predicate_bound_compiles_no_patterns() {
    // Arrange: the first predicate carries a perfectly compilable pattern.
    let tasks = PushdownTasks::new(regexp_capable_descriptor());
    let pattern = "^nginx$";
    let mut predicates = vec![predicate(
        "name",
        proto::PredicateOp::Regexp,
        vec![literal_string(pattern)],
    )];
    predicates.extend(
        (0..=MAX_PREDICATES_PER_PLAN)
            .map(|_ordinal| predicate("pid", proto::PredicateOp::Eq, vec![literal_uint(1)])),
    );
    let oversized = task("too-many-with-pattern", predicates, vec!["pid".to_owned()]);

    // Act
    let refusal = tasks.accept(&oversized, SystemTime::UNIX_EPOCH);

    // Assert
    assert!(
        matches!(
            refusal,
            Err(PushdownRejection::PlanTooLarge {
                what: "predicates",
                ..
            })
        ),
        "an unbounded predicate count must be refused, got {refusal:?}"
    );
    assert_eq!(
        tasks.patterns().stats().compiles,
        0,
        "a plan refused on its bounds must attempt no compilation at all"
    );
    assert!(!tasks.patterns().is_cached(pattern));
}

/// The same ordering holds for a typed defect: nothing compiles until the whole plan is valid.
#[test]
fn a_plan_with_a_later_unknown_column_compiles_no_patterns() {
    // Arrange: a valid pattern predicate followed by a column the collector never advertised.
    let tasks = PushdownTasks::new(regexp_capable_descriptor());
    let pattern = "^sshd$";
    let mixed = task(
        "pattern-then-unknown",
        vec![
            predicate(
                "name",
                proto::PredicateOp::Regexp,
                vec![literal_string(pattern)],
            ),
            predicate("ppid", proto::PredicateOp::Eq, vec![literal_uint(2)]),
        ],
        vec!["pid".to_owned()],
    );

    // Act
    let refusal = tasks.accept(&mixed, SystemTime::UNIX_EPOCH);

    // Assert
    assert_eq!(
        refusal,
        Err(PushdownRejection::UnknownColumn {
            table: "processes".to_owned(),
            column: "ppid".to_owned(),
        })
    );
    assert_eq!(tasks.patterns().stats().compiles, 0);
    assert!(!tasks.patterns().is_cached(pattern));
}

/// An accepted pattern *is* resident, so the evaluator that follows never recompiles it.
#[test]
fn an_accepted_pattern_is_resident_in_the_shared_cache() {
    // Arrange
    let tasks = PushdownTasks::new(regexp_capable_descriptor());
    let pattern = "^bash$";
    let accepted = task(
        "resident",
        vec![predicate(
            "name",
            proto::PredicateOp::Regexp,
            vec![literal_string(pattern)],
        )],
        vec!["pid".to_owned()],
    );

    // Act
    assert_eq!(tasks.accept(&accepted, SystemTime::UNIX_EPOCH), Ok(()));

    // Assert
    assert!(tasks.patterns().is_cached(pattern));
    assert_eq!(tasks.patterns().stats().compiles, 1);
}
