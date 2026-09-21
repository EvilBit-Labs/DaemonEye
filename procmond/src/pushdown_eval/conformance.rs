//! procmond's conformance self-test, run before it registers (R22).
//!
//! The agent never runs this. R22 puts the result on the registration exchange, so the collector
//! produces it for itself against the corpus both sides share
//! (`daemoneye_lib::detection::conformance`). Each advertised operation is exercised against every
//! corpus case that applies to its column's declared type, through
//! [`PushdownEvaluator::evaluate`] — the same code path a pushed task takes — and passes only when
//! every case that ran agreed with the agent's reference.
//!
//! A pass proves agreement with **that reference**, not with the executor that will evaluate the
//! residual half. ADR-0006 makes Apache `DataFusion` that executor, T6 builds it, and it does not
//! exist yet; when it lands, the reference has to be re-verified against it.
//!
//! Some cases cannot be run at all. `pid` is a 32-bit field that cannot hold `u64::MAX`, and
//! `command_line` models the empty string as SQL NULL, so a case naming either value is skipped
//! for that column rather than counted as a disagreement. An operation whose cases were *all*
//! skipped does not pass, so the skip cannot be used to earn one.

use super::PushdownEvaluator;
use daemoneye_eventbus::rpc::{
    ColumnDescriptor, ColumnType as WireColumnType, ConformanceResult,
    PredicateOp as WirePredicateOp, SchemaDescriptor,
};
use daemoneye_lib::detection::conformance::{
    ConformanceCase, ConformanceOutcome, verify_operation,
};
use daemoneye_lib::proto::{
    ColumnType, Literal, Predicate, PredicateOp, ProcessRecord, PushdownPlan, literal,
};

/// procmond's descriptor with its own conformance results attached (R22).
///
/// This is what registration advertises: the descriptor and the results ride the same
/// authenticated exchange, because the catalog binds a result to the `descriptor_version` it was
/// produced against and drops every result a collector held whenever its descriptor changes.
#[must_use]
pub fn descriptor_with_conformance(collector_id: &str) -> SchemaDescriptor {
    let evaluator = PushdownEvaluator::new(collector_id);
    let mut descriptor = evaluator.descriptor().clone();
    descriptor.conformance_results = self_test(&evaluator);
    descriptor
}

/// Run the corpus against this collector's own evaluation, one result per advertised operation.
#[must_use]
pub fn self_test(evaluator: &PushdownEvaluator) -> Vec<ConformanceResult> {
    let mut results = Vec::new();
    for table in &evaluator.descriptor().tables {
        for column in &table.columns {
            for &wire_op in &column.supported_ops {
                let Some(result) = verify_column_op(evaluator, &table.name, column, wire_op) else {
                    continue;
                };
                results.push(result);
            }
        }
    }
    results
}

/// One `(table, column, op)` triple's result, or `None` when this build cannot name the operation
/// or the column's declared type.
fn verify_column_op(
    evaluator: &PushdownEvaluator,
    table: &str,
    column: &ColumnDescriptor,
    wire_op: WirePredicateOp,
) -> Option<ConformanceResult> {
    let op = proto_op(wire_op)?;
    let column_type = proto_column_type(column.column_type)?;
    let passed = verify_operation(column_type, column.nullable, op, |case| {
        probe(evaluator, table, column, op, case)
    });
    Some(ConformanceResult {
        table: table.to_owned(),
        column: column.name.clone(),
        op: wire_op,
        passed,
    })
}

/// Evaluate one case with this collector's own machinery.
///
/// `None` means the case's row is not representable on this column, which is a skip rather than a
/// disagreement.
fn probe(
    evaluator: &PushdownEvaluator,
    table: &str,
    column: &ColumnDescriptor,
    op: PredicateOp,
    case: &ConformanceCase,
) -> Option<ConformanceOutcome> {
    let record = case_record(&column.name, column.nullable, case.observed.as_ref())?;
    let plan = PushdownPlan {
        table: table.to_owned(),
        predicates: vec![Predicate {
            column: column.name.clone(),
            op: i32::from(op),
            values: case
                .literals
                .iter()
                .map(|value| Literal {
                    value: Some(value.clone()),
                })
                .collect(),
        }],
        projection: vec![column.name.clone()],
        // `evaluate` is the lifetime-free primitive and never reads the TTL; `evaluate_task` is
        // the gated entry point, and gating a self-test on a task lifetime would test the clock.
        ttl_ms: 0,
    };
    match evaluator.evaluate(&plan, std::slice::from_ref(&record)) {
        Ok(rows) if rows.is_empty() => Some(ConformanceOutcome::Excludes),
        Ok(_admitted) => Some(ConformanceOutcome::Admits),
        Err(_refused) => Some(ConformanceOutcome::Refuses),
    }
}

/// A one-row record carrying `observed` in `column` and nothing else of interest.
///
/// `None` when this column cannot carry that value faithfully, which is the skip the module docs
/// describe. Building a fresh record per case rather than mutating one keeps a case from
/// inheriting the previous case's row.
fn case_record(
    column: &str,
    nullable: bool,
    observed: Option<&literal::Value>,
) -> Option<ProcessRecord> {
    let base = ProcessRecord::default();
    let Some(value) = observed else {
        // SQL NULL. Every nullable column is already absent on a fresh record; a column declared
        // `NOT NULL` has no way to carry it, so the case is not representable there.
        return nullable.then_some(base);
    };
    let record = match column {
        "pid" => ProcessRecord {
            pid: u32::try_from(uint_of(value)?).ok()?,
            ..base
        },
        "ppid" => ProcessRecord {
            ppid: Some(u32::try_from(uint_of(value)?).ok()?),
            ..base
        },
        "name" => ProcessRecord {
            name: string_of(value)?.to_owned(),
            ..base
        },
        "executable_path" => ProcessRecord {
            executable_path: Some(string_of(value)?.to_owned()),
            ..base
        },
        "executable_hash" => ProcessRecord {
            executable_hash: Some(string_of(value)?.to_owned()),
            ..base
        },
        "user_id" => ProcessRecord {
            user_id: Some(string_of(value)?.to_owned()),
            ..base
        },
        // The empty string is this collector's spelling of SQL NULL here, so it cannot be
        // represented as a value and the case is skipped.
        "command_line" => {
            let text = string_of(value)?;
            if text.is_empty() {
                return None;
            }
            ProcessRecord {
                command_line: vec![text.to_owned()],
                ..base
            }
        }
        "start_time" => ProcessRecord {
            start_time: Some(int_of(value)?),
            ..base
        },
        "cpu_usage" => ProcessRecord {
            cpu_usage: Some(float_of(value)?),
            ..base
        },
        "memory_usage" => ProcessRecord {
            memory_usage: Some(uint_of(value)?),
            ..base
        },
        "accessible" => ProcessRecord {
            accessible: bool_of(value)?,
            ..base
        },
        "file_exists" => ProcessRecord {
            file_exists: bool_of(value)?,
            ..base
        },
        "collection_time" => ProcessRecord {
            collection_time: int_of(value)?,
            ..base
        },
        _unadvertised => return None,
    };
    Some(record)
}

/// The unsigned value a literal carries.
// KTD8: the wildcard arm yields `None`, which the caller reads as "not representable".
// `literal::Value` is `#[non_exhaustive]`, and a kind this build cannot name is not an integer.
#[allow(clippy::wildcard_enum_match_arm)]
const fn uint_of(value: &literal::Value) -> Option<u64> {
    match *value {
        literal::Value::UintValue(inner) => Some(inner),
        ref _other => None,
    }
}

/// The signed value a literal carries.
#[allow(clippy::wildcard_enum_match_arm)]
const fn int_of(value: &literal::Value) -> Option<i64> {
    match *value {
        literal::Value::IntValue(inner) => Some(inner),
        ref _other => None,
    }
}

/// The float a literal carries.
#[allow(clippy::wildcard_enum_match_arm)]
const fn float_of(value: &literal::Value) -> Option<f64> {
    match *value {
        literal::Value::FloatValue(inner) => Some(inner),
        ref _other => None,
    }
}

/// The boolean a literal carries.
#[allow(clippy::wildcard_enum_match_arm)]
const fn bool_of(value: &literal::Value) -> Option<bool> {
    match *value {
        literal::Value::BoolValue(inner) => Some(inner),
        ref _other => None,
    }
}

/// The text a literal carries.
#[allow(clippy::wildcard_enum_match_arm)]
const fn string_of(value: &literal::Value) -> Option<&str> {
    match *value {
        literal::Value::StringValue(ref inner) => Some(inner.as_str()),
        ref _other => None,
    }
}

/// Maps the descriptor's operation vocabulary onto the protobuf one the corpus is keyed by.
///
/// `None` for an operation this build cannot name; the wildcard arm refuses, because
/// `PredicateOp` is `#[non_exhaustive]` and an unnameable operation must never be verified as one
/// it is not.
const fn proto_op(op: WirePredicateOp) -> Option<PredicateOp> {
    match op {
        WirePredicateOp::Eq => Some(PredicateOp::Eq),
        WirePredicateOp::Ne => Some(PredicateOp::Ne),
        WirePredicateOp::Lt => Some(PredicateOp::Lt),
        WirePredicateOp::Le => Some(PredicateOp::Le),
        WirePredicateOp::Gt => Some(PredicateOp::Gt),
        WirePredicateOp::Ge => Some(PredicateOp::Ge),
        WirePredicateOp::In => Some(PredicateOp::In),
        WirePredicateOp::Like => Some(PredicateOp::Like),
        WirePredicateOp::Regexp => Some(PredicateOp::Regexp),
        WirePredicateOp::Unspecified => None,
        _unrecognized => None,
    }
}

/// Maps the descriptor's column types onto the protobuf ones, refusing anything unnameable.
const fn proto_column_type(column_type: WireColumnType) -> Option<ColumnType> {
    match column_type {
        WireColumnType::String => Some(ColumnType::String),
        WireColumnType::Int => Some(ColumnType::Int),
        WireColumnType::Uint => Some(ColumnType::Uint),
        WireColumnType::Float => Some(ColumnType::Float),
        WireColumnType::Bool => Some(ColumnType::Bool),
        WireColumnType::Unspecified => None,
        _unrecognized => None,
    }
}
