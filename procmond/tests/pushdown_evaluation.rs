//! procmond evaluates the pushed typed predicate payload against real process records (R20, R21).
//!
//! No SQL crosses the IPC boundary here: every scenario builds the typed
//! [`daemoneye_lib::proto::PushdownPlan`] the agent lowers a rule into and checks the rows and
//! columns the collector hands back.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // Every wildcard below panics: a variant this test cannot name is a test failure, never a pass.
    clippy::wildcard_enum_match_arm
)]

use daemoneye_eventbus::rpc::{ColumnType, PredicateOp as DescriptorOp};
use daemoneye_lib::detection::RegexRejection;
use daemoneye_lib::proto::{
    DetectionTask, Literal, Predicate, PredicateOp, ProcessRecord, PushdownPlan, literal,
};
use procmond::pushdown_eval::{FieldValue, ProjectedRow, PushdownError, PushdownEvaluator};
use std::time::{Duration, SystemTime};

const TTL_MS: u64 = 60_000;
const COLLECTOR_ID: &str = "procmond";

fn evaluator() -> PushdownEvaluator {
    PushdownEvaluator::new(COLLECTOR_ID)
}

fn string_literal(value: &str) -> Literal {
    Literal {
        value: Some(literal::Value::StringValue(value.to_owned())),
    }
}

const fn uint_literal(value: u64) -> Literal {
    Literal {
        value: Some(literal::Value::UintValue(value)),
    }
}

const fn int_literal(value: i64) -> Literal {
    Literal {
        value: Some(literal::Value::IntValue(value)),
    }
}

const fn null_literal() -> Literal {
    Literal {
        value: Some(literal::Value::NullValue(true)),
    }
}

fn predicate(column: &str, op: PredicateOp, values: Vec<Literal>) -> Predicate {
    Predicate {
        column: column.to_owned(),
        op: i32::from(op),
        values,
    }
}

fn plan(predicates: Vec<Predicate>, projection: &[&str]) -> PushdownPlan {
    PushdownPlan {
        table: "processes".to_owned(),
        predicates,
        projection: projection.iter().map(|name| (*name).to_owned()).collect(),
        ttl_ms: TTL_MS,
    }
}

fn task(task_id: &str, plan: PushdownPlan) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        pushdown_plan: Some(plan),
        ..Default::default()
    }
}

/// A record with every column populated, so a test opts into NULL rather than inheriting it.
fn record(pid: u32, name: &str) -> ProcessRecord {
    ProcessRecord {
        pid,
        ppid: Some(1),
        name: name.to_owned(),
        executable_path: Some(format!("/usr/bin/{name}")),
        command_line: vec![name.to_owned(), "--serve".to_owned()],
        start_time: Some(1_700_000_000),
        cpu_usage: Some(2.5),
        memory_usage: Some(4096),
        executable_hash: Some("a".repeat(64)),
        user_id: Some("0".to_owned()),
        accessible: true,
        file_exists: true,
        collection_time: 1_700_000_000_000,
        ..Default::default()
    }
}

/// Accept `plan` and evaluate it against `records`, failing loudly on a refusal.
fn matching_rows(
    evaluator: &PushdownEvaluator,
    plan: &PushdownPlan,
    records: &[ProcessRecord],
) -> Vec<ProjectedRow> {
    let now = SystemTime::now();
    evaluator
        .accept(&task("t1", plan.clone()), now)
        .expect("plan must be accepted");
    evaluator.evaluate(plan, records).expect("evaluation")
}

/// The pids a plan admits, in record order.
fn matching_pids(
    evaluator: &PushdownEvaluator,
    plan: &PushdownPlan,
    records: &[ProcessRecord],
) -> Vec<u64> {
    matching_rows(evaluator, plan, records)
        .iter()
        .map(|row| match row.get("pid") {
            Some(&Some(FieldValue::Uint(pid))) => pid,
            other => panic!("expected a pid column, got {other:?}"),
        })
        .collect()
}

#[test]
fn equality_and_inequality_select_the_rows_their_column_type_expects() {
    // Arrange
    let evaluator = evaluator();
    let records = vec![record(10, "nginx"), record(20, "bash"), record(30, "nginx")];

    // Act
    let equal = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "name",
                PredicateOp::Eq,
                vec![string_literal("nginx")],
            )],
            &["pid"],
        ),
        &records,
    );
    let unequal = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("pid", PredicateOp::Ne, vec![uint_literal(20)])],
            &["pid"],
        ),
        &records,
    );

    // Assert
    assert_eq!(equal, vec![10, 30]);
    assert_eq!(unequal, vec![10, 30]);
}

#[test]
fn ordering_operations_select_the_rows_their_column_type_expects() {
    let evaluator = evaluator();
    let records = vec![record(10, "a"), record(20, "b"), record(30, "c")];

    let greater = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("pid", PredicateOp::Gt, vec![uint_literal(10)])],
            &["pid"],
        ),
        &records,
    );
    let at_most = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("pid", PredicateOp::Le, vec![uint_literal(20)])],
            &["pid"],
        ),
        &records,
    );

    assert_eq!(greater, vec![20, 30]);
    assert_eq!(at_most, vec![10, 20]);
}

#[test]
fn in_matches_any_listed_literal_and_rejects_the_rest() {
    let evaluator = evaluator();
    let records = vec![record(10, "a"), record(20, "b"), record(30, "c")];

    let selected = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "pid",
                PredicateOp::In,
                vec![uint_literal(10), uint_literal(30)],
            )],
            &["pid"],
        ),
        &records,
    );

    assert_eq!(selected, vec![10, 30]);
}

#[test]
fn like_matches_its_wildcards_and_treats_the_rest_of_the_pattern_literally() {
    let evaluator = evaluator();
    let records = vec![
        record(10, "nginx"),
        record(20, "bash"),
        record(30, "nginx-worker"),
    ];

    let prefixed = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "name",
                PredicateOp::Like,
                vec![string_literal("nginx%")],
            )],
            &["pid"],
        ),
        &records,
    );
    let single = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "name",
                PredicateOp::Like,
                vec![string_literal("bas_")],
            )],
            &["pid"],
        ),
        &records,
    );

    assert_eq!(prefixed, vec![10, 30]);
    assert_eq!(single, vec![20]);
}

#[test]
fn regexp_matches_the_rows_its_pattern_describes() {
    let evaluator = evaluator();
    let records = vec![record(10, "nginx"), record(20, "bash"), record(30, "sshd")];

    let selected = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "name",
                PredicateOp::Regexp,
                vec![string_literal("^(nginx|sshd)$")],
            )],
            &["pid"],
        ),
        &records,
    );

    assert_eq!(selected, vec![10, 30]);
}

#[test]
fn an_over_bounds_regexp_pattern_refuses_the_task_and_never_enters_the_pattern_cache() {
    // Arrange: a pattern whose compiled program cannot fit the fixed byte ceiling.
    let evaluator = evaluator();
    let pattern = format!("(?:{}){{4096}}", "[0-9a-f]".repeat(64));
    let offending = plan(
        vec![predicate(
            "name",
            PredicateOp::Regexp,
            vec![string_literal(&pattern)],
        )],
        &["pid"],
    );

    // Act
    let rejection = evaluator
        .accept(&task("oversized", offending), SystemTime::now())
        .expect_err("an over-bounds pattern must refuse the task");

    // Assert: the task is refused, and the pattern is not resident in the bounded cache.
    assert!(
        matches!(
            rejection,
            PushdownError::PatternRejected {
                rejection: RegexRejection::CompiledTooBig { .. },
                ..
            }
        ),
        "expected an over-bounds pattern rejection, got {rejection:?}"
    );
    assert!(
        !evaluator.is_pattern_cached(&pattern),
        "an over-bounds pattern must never become resident"
    );
    assert_eq!(evaluator.pattern_stats().compiles, 1);
    assert_eq!(
        evaluator.cached_pattern_count(),
        0,
        "the cache must hold nothing after a refusal"
    );
}

#[test]
fn a_projection_naming_three_of_five_columns_returns_exactly_those_three() {
    // Arrange
    let evaluator = evaluator();
    let records = vec![record(10, "nginx")];
    let five_available = ["pid", "ppid", "name", "executable_path", "memory_usage"];
    let projected = ["pid", "name", "memory_usage"];

    // Act
    let rows = matching_rows(
        &evaluator,
        &plan(
            vec![predicate("pid", PredicateOp::Eq, vec![uint_literal(10)])],
            &projected,
        ),
        &records,
    );

    // Assert
    let row = rows.first().expect("one matching row");
    let mut names: Vec<&str> = row.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["memory_usage", "name", "pid"]);
    for dropped in five_available
        .iter()
        .filter(|name| !projected.contains(name))
    {
        assert!(
            !row.contains_key(*dropped),
            "projection leaked column `{dropped}`"
        );
    }
}

/// A process with no readable argument vector has a NULL `command_line`, not an empty string.
///
/// The distinction is load-bearing and easy to lose: modelling an absent argument vector as `""`
/// would make `command_line = ''` match every process the collector cannot read, which is the
/// opposite of what an operator writing that rule means. Nothing else in the suite constructs a
/// record with an empty vector, so this is the only place the semantic is exercised rather than
/// merely inspected.
#[test]
fn an_empty_argument_vector_reads_as_null_rather_than_as_the_empty_string() {
    let evaluator = evaluator();
    let mut unreadable = record(30, "unreadable");
    unreadable.command_line = vec![];
    let records = vec![record(10, "rooted"), unreadable];

    // `= ''` must not match the unreadable process: NULL is not the empty string.
    let empty_match = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "command_line",
                PredicateOp::Eq,
                vec![string_literal("")],
            )],
            &["pid"],
        ),
        &records,
    );
    assert!(
        empty_match.is_empty(),
        "an absent argument vector must not match the empty string, got {empty_match:?}"
    );

    // `!= 'x'` must not match it either: a comparison against NULL is UNKNOWN, not true.
    let unequal = matching_pids(
        &evaluator,
        &plan(
            vec![predicate(
                "command_line",
                PredicateOp::Ne,
                vec![string_literal("x")],
            )],
            &["pid"],
        ),
        &records,
    );
    assert_eq!(
        unequal,
        vec![10],
        "only the readable process may match; NULL != 'x' is UNKNOWN"
    );
}

#[test]
fn a_predicate_over_a_null_column_is_unknown_so_the_row_never_matches() {
    // Arrange: `ppid` is NULL on the second record.
    let evaluator = evaluator();
    let mut orphan = record(20, "orphan");
    orphan.ppid = None;
    let records = vec![record(10, "rooted"), orphan];

    // Act: both a positive and a negative comparison, because `!= NULL` is the arm that
    // silently reads as `true` when NULL is handled per-operation instead of up front.
    let equal = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("ppid", PredicateOp::Eq, vec![uint_literal(1)])],
            &["pid"],
        ),
        &records,
    );
    let unequal = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("ppid", PredicateOp::Ne, vec![uint_literal(1)])],
            &["pid"],
        ),
        &records,
    );
    let listed = matching_pids(
        &evaluator,
        &plan(
            vec![predicate("ppid", PredicateOp::In, vec![uint_literal(1)])],
            &["pid"],
        ),
        &records,
    );

    // Assert: the NULL row is absent from every one of them.
    assert_eq!(equal, vec![10]);
    assert_eq!(unequal, Vec::<u64>::new());
    assert_eq!(listed, vec![10]);
}

#[test]
fn a_conjunction_of_three_predicates_matches_only_rows_satisfying_all_three() {
    // Arrange
    let evaluator = evaluator();
    let mut low_memory = record(20, "nginx");
    low_memory.memory_usage = Some(8);
    let mut other_name = record(30, "bash");
    other_name.memory_usage = Some(8192);
    let records = vec![record(10, "nginx"), low_memory, other_name];

    // Act
    let selected = matching_pids(
        &evaluator,
        &plan(
            vec![
                predicate("name", PredicateOp::Eq, vec![string_literal("nginx")]),
                predicate("memory_usage", PredicateOp::Ge, vec![uint_literal(1024)]),
                predicate("pid", PredicateOp::Lt, vec![uint_literal(15)]),
            ],
            &["pid"],
        ),
        &records,
    );

    // Assert
    assert_eq!(selected, vec![10]);
}

#[test]
fn a_string_literal_against_an_integer_column_is_refused_rather_than_coerced() {
    // Arrange
    let evaluator = evaluator();
    let mistyped = plan(
        vec![predicate(
            "pid",
            PredicateOp::Eq,
            vec![string_literal("10")],
        )],
        &["pid"],
    );

    // Act
    let rejection = evaluator
        .accept(&task("mistyped", mistyped), SystemTime::now())
        .expect_err("a mistyped literal must refuse the task");

    // Assert
    assert!(
        matches!(rejection, PushdownError::LiteralTypeMismatch { .. }),
        "expected a literal type mismatch, got {rejection:?}"
    );
}

#[test]
fn a_null_literal_against_a_non_nullable_column_is_refused() {
    // Arrange: `pid` is declared NOT NULL.
    let evaluator = evaluator();
    let impossible = plan(
        vec![predicate("pid", PredicateOp::Eq, vec![null_literal()])],
        &["pid"],
    );

    // Act
    let rejection = evaluator
        .accept(&task("null-literal", impossible), SystemTime::now())
        .expect_err("a NULL literal against a NOT NULL column must refuse the task");

    // Assert
    assert!(
        matches!(rejection, PushdownError::NullLiteralOnNonNullable { .. }),
        "expected a NULL-literal rejection, got {rejection:?}"
    );
}

#[test]
fn every_column_and_operation_the_descriptor_advertises_is_evaluable() {
    // Arrange: the descriptor is the contract; anything advertised must be evaluable.
    let evaluator = evaluator();
    let records = vec![record(10, "nginx")];
    let descriptor = evaluator.descriptor().clone();
    let table = descriptor.tables.first().expect("one table");

    // Act + Assert
    for column in &table.columns {
        for &op in &column.supported_ops {
            let (wire_op, value) = advertised_probe(op, column.column_type);
            let probe = plan(
                vec![predicate(&column.name, wire_op, vec![value])],
                &["pid"],
            );
            let accepted = evaluator.accept(
                &task(
                    &format!("{}-{}", column.name, op.as_wire_name()),
                    probe.clone(),
                ),
                SystemTime::now(),
            );
            assert!(
                accepted.is_ok(),
                "advertised {}/{} was refused: {accepted:?}",
                column.name,
                op.as_wire_name()
            );
            let evaluated = evaluator.evaluate(&probe, &records);
            assert!(
                evaluated.is_ok(),
                "advertised {}/{} is not evaluable: {evaluated:?}",
                column.name,
                op.as_wire_name()
            );
        }
    }
}

/// A well-typed probe literal for one advertised `(op, column type)` pair.
fn advertised_probe(op: DescriptorOp, column_type: ColumnType) -> (PredicateOp, Literal) {
    let wire_op = match op {
        DescriptorOp::Eq => PredicateOp::Eq,
        DescriptorOp::Ne => PredicateOp::Ne,
        DescriptorOp::Lt => PredicateOp::Lt,
        DescriptorOp::Le => PredicateOp::Le,
        DescriptorOp::Gt => PredicateOp::Gt,
        DescriptorOp::Ge => PredicateOp::Ge,
        DescriptorOp::In => PredicateOp::In,
        DescriptorOp::Like => PredicateOp::Like,
        DescriptorOp::Regexp => PredicateOp::Regexp,
        other => panic!("descriptor advertises an unusable operation: {other:?}"),
    };
    let value = match column_type {
        ColumnType::String => string_literal("nginx"),
        ColumnType::Int => int_literal(1),
        ColumnType::Uint => uint_literal(1),
        ColumnType::Float => Literal {
            value: Some(literal::Value::FloatValue(1.0)),
        },
        ColumnType::Bool => Literal {
            value: Some(literal::Value::BoolValue(true)),
        },
        other => panic!("descriptor advertises an unusable column type: {other:?}"),
    };
    (wire_op, value)
}

#[test]
fn scalar_comparisons_agree_with_the_reference_evaluation_the_agent_uses() {
    // Arrange: the six comparisons the agent's reference evaluator covers, over an INT column.
    // The reference model lives in `daemoneye-lib/tests/support`, which cannot be imported across
    // crates, so the oracle below reproduces `predicate_holds` for those six operations exactly.
    let evaluator = evaluator();
    let records: Vec<ProcessRecord> = (0_i64..4)
        .map(|offset| {
            let mut candidate = record(u32::try_from(offset).unwrap_or(0) + 1, "probe");
            candidate.start_time = Some(offset);
            candidate
        })
        .collect();
    let ops = [
        PredicateOp::Eq,
        PredicateOp::Ne,
        PredicateOp::Lt,
        PredicateOp::Le,
        PredicateOp::Gt,
        PredicateOp::Ge,
    ];

    // Act + Assert
    for op in ops {
        for expected in 0_i64..4 {
            let probe = plan(
                vec![predicate("start_time", op, vec![int_literal(expected)])],
                &["pid", "start_time"],
            );
            let admitted = matching_rows(&evaluator, &probe, &records);
            let reference: Vec<i64> = records
                .iter()
                .filter_map(|candidate| candidate.start_time)
                .filter(|&observed| reference_holds(op, observed, expected))
                .collect();
            let observed: Vec<i64> = admitted
                .iter()
                .map(|row| match row.get("start_time") {
                    Some(&Some(FieldValue::Int(value))) => value,
                    other => panic!("expected an int column, got {other:?}"),
                })
                .collect();
            assert_eq!(
                observed, reference,
                "collector and reference disagree for {op:?} against {expected}"
            );
        }
    }
}

/// `predicate_holds` from the agent's property-test support module, for the six scalar ops.
fn reference_holds(op: PredicateOp, observed: i64, expected: i64) -> bool {
    match op {
        PredicateOp::Eq => observed == expected,
        PredicateOp::Ne => observed != expected,
        PredicateOp::Lt => observed < expected,
        PredicateOp::Le => observed <= expected,
        PredicateOp::Gt => observed > expected,
        PredicateOp::Ge => observed >= expected,
        other => panic!("reference model reached an unexpected operation: {other:?}"),
    }
}

#[test]
fn an_expired_task_stops_producing_rows() {
    // Arrange
    let evaluator = evaluator();
    let now = SystemTime::now();
    let accepted = plan(
        vec![predicate("pid", PredicateOp::Eq, vec![uint_literal(10)])],
        &["pid"],
    );
    let records = vec![record(10, "nginx")];
    evaluator
        .accept(&task("expiring", accepted), now)
        .expect("accepted");
    let after_ttl = now
        .checked_add(Duration::from_millis(TTL_MS + 1))
        .expect("a representable deadline");

    // Act
    let while_active = evaluator.evaluate_task("expiring", &records, now);
    let once_expired = evaluator.evaluate_task("expiring", &records, after_ttl);

    // Assert
    assert_eq!(while_active.expect("active task evaluates").len(), 1);
    assert!(
        matches!(
            once_expired,
            Err(PushdownError::TaskNotActive { ref task_id }) if task_id == "expiring"
        ),
        "an expired task must refuse evaluation, got {once_expired:?}"
    );
}

#[test]
fn an_accepted_task_is_active_until_its_ttl_elapses() {
    // Arrange
    let evaluator = evaluator();
    let now = SystemTime::now();
    let accepted = plan(
        vec![predicate("pid", PredicateOp::Eq, vec![uint_literal(10)])],
        &["pid"],
    );

    // Act
    evaluator
        .accept(&task("lifetime", accepted), now)
        .expect("accepted");

    // Assert
    assert!(evaluator.is_active("lifetime", now));
    let after_ttl = now
        .checked_add(Duration::from_millis(TTL_MS + 1))
        .expect("a representable deadline");
    assert!(!evaluator.is_active("lifetime", after_ttl));
}

/// Evaluating by task id uses the plan the task was accepted with, and renewal replaces it.
///
/// The binding cannot be tested by substituting a plan — `evaluate_task` no longer takes one, which
/// is the fix. What it can show is that the stored plan is the one evaluated: accept a plan
/// matching one pid, evaluate by id, then re-accept the same id with a plan matching a different
/// pid and evaluate by id again.
#[test]
fn evaluating_by_task_id_uses_the_stored_plan() {
    // Arrange
    let evaluator = evaluator();
    let now = SystemTime::now();
    let records = vec![record(10, "nginx"), record(20, "bash")];
    let first = plan(
        vec![predicate("pid", PredicateOp::Eq, vec![uint_literal(10)])],
        &["pid"],
    );
    let renewed = plan(
        vec![predicate("pid", PredicateOp::Eq, vec![uint_literal(20)])],
        &["pid"],
    );

    // Act
    evaluator
        .accept(&task("bound", first), now)
        .expect("the first plan is accepted");
    let before = evaluator
        .evaluate_task("bound", &records, now)
        .expect("an active task evaluates");
    evaluator
        .accept(&task("bound", renewed), now)
        .expect("renewal is accepted");
    let after = evaluator
        .evaluate_task("bound", &records, now)
        .expect("the renewed task evaluates");

    // Assert
    assert_eq!(pids_of(&before), vec![10]);
    assert_eq!(
        pids_of(&after),
        vec![20],
        "renewal must replace the stored plan, not be shadowed by the one first accepted"
    );
}

/// The pids in a set of projected rows, in row order.
fn pids_of(rows: &[ProjectedRow]) -> Vec<u64> {
    rows.iter()
        .map(|row| match row.get("pid") {
            Some(&Some(FieldValue::Uint(pid))) => pid,
            other => panic!("expected a uint pid, got {other:?}"),
        })
        .collect()
}
