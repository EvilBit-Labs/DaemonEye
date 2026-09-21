//! Collector-side acceptance of pushed detection tasks (R19).
//!
//! A [`daemoneye_lib::proto::PushdownPlan`] arrives from the agent naming a table,
//! a conjunction of predicates and a projection. This module validates that plan against the
//! [`SchemaDescriptor`] the collector registered and tracks the lifetime of the tasks it accepts.
//!
//! Two properties are load-bearing:
//!
//! - **A task is accepted whole or refused whole.** Validation completes before anything is
//!   recorded, so a plan whose first predicate is valid and whose second is not leaves no trace.
//! - **Expiry is observable.** An accepted task moves from [`TaskStatus::Active`] to
//!   [`TaskStatus::Expired`] at its deadline, read through an explicit `now` rather than wall
//!   time, so a missed renewal ends the work.
//!
//! Evaluating the predicates against real data is the owning collector's job; this module only
//! decides whether a task may be evaluated at all.

use daemoneye_eventbus::rpc::{
    ColumnDescriptor, PredicateOp as DescriptorOp, SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::detection::RegexRejection;
use daemoneye_lib::detection_bounds::{MAX_IDENTIFIER_LENGTH, PUSHDOWN_TASK_TTL};
use daemoneye_lib::proto::{DetectionTask, Predicate, PredicateOp, PushdownPlan};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use thiserror::Error;

/// Why a collector refused a pushed detection task.
///
/// Every variant names the specific offender, so an operator learns which column or operation the
/// collector never advertised rather than that "validation failed".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PushdownRejection {
    /// The event source does not handle pushdown tasks at all.
    #[error("event source `{event_source}` does not handle pushdown tasks")]
    PushdownUnsupported {
        /// Name of the refusing event source.
        event_source: &'static str,
    },

    /// The task carried no pushdown plan.
    #[error("detection task carries no pushdown plan")]
    MissingPlan,

    /// The task carried no identifier, so its lifetime could not be tracked.
    #[error("detection task carries no task id")]
    MissingTaskId,

    /// An identifier in the plan exceeded the fixed length bound.
    #[error("identifier of {length} bytes exceeds the {MAX_IDENTIFIER_LENGTH}-byte bound")]
    IdentifierTooLong {
        /// Length of the offending identifier in bytes.
        length: usize,
    },

    /// The plan named a table this collector does not serve.
    #[error("table `{table}` is not advertised by this collector")]
    UnknownTable {
        /// The unadvertised table.
        table: String,
    },

    /// The plan named a column the table does not declare.
    #[error("column `{column}` is not advertised by table `{table}`")]
    UnknownColumn {
        /// Table the column was expected in.
        table: String,
        /// The unadvertised column.
        column: String,
    },

    /// The predicate's operation was unset, or newer than this build can name.
    #[error("predicate on column `{column}` carries an unrecognized operation")]
    UnspecifiedOperation {
        /// Column the offending predicate reads.
        column: String,
    },

    /// The column did not advertise the predicate's operation.
    #[error("column `{column}` does not advertise operation `{op}`")]
    UnsupportedOperation {
        /// Column the offending predicate reads.
        column: String,
        /// Wire name of the unadvertised operation.
        op: String,
    },

    /// The predicate carried the wrong number of values for its operation.
    #[error("operation `{op}` on column `{column}` cannot take {values} values")]
    InvalidArity {
        /// Column the offending predicate reads.
        column: String,
        /// Wire name of the operation.
        op: String,
        /// Number of values the predicate carried.
        values: usize,
    },

    /// A literal's kind did not match the column's declared type.
    ///
    /// Distinct from [`Self::UnsupportedOperation`]: the operation is advertised and the column
    /// exists, and only the literal is wrong. Collapsing the two told an operator to look at an
    /// operation that was never the problem.
    #[error(
        "column `{column}` is declared {column_type} and cannot be compared to a {literal_kind} literal"
    )]
    LiteralTypeMismatch {
        /// Column the offending predicate reads.
        column: String,
        /// Wire name of the column's declared type.
        column_type: String,
        /// Wire name of the literal's kind.
        literal_kind: String,
    },

    /// A NULL literal was pushed against a column declared `NOT NULL`.
    #[error("column `{column}` is declared NOT NULL, so a NULL literal can never match")]
    NullLiteralOnNonNullable {
        /// Column the offending predicate reads.
        column: String,
    },

    /// A `LIKE` or `REGEXP` pattern exceeded the collector's own compile bounds, or did not
    /// compile at all (R21).
    #[error("pattern on column `{column}` cannot be compiled by this collector: {rejection}")]
    PatternRejected {
        /// Column the offending predicate reads.
        column: String,
        /// The bounded compiler's own reason.
        rejection: RegexRejection,
    },

    /// The plan's TTL was zero or beyond the fixed ceiling.
    #[error("ttl of {ttl_ms}ms is outside the permitted range")]
    InvalidTtl {
        /// The offending TTL in milliseconds.
        ttl_ms: u64,
    },
}

/// Lifecycle state of a task the collector was asked to evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TaskStatus {
    /// No such task was ever accepted, or it was refused.
    Unknown,
    /// Accepted and within its TTL.
    Active,
    /// Accepted, but its TTL elapsed without renewal.
    Expired,
}

/// The pushdown tasks a single collector has accepted, and the descriptor it validates against.
///
/// The descriptor held here is the same value the collector advertises in its registration
/// exchange; validating against anything else would let a collector accept work it never claimed.
#[derive(Debug)]
pub struct PushdownTasks {
    descriptor: SchemaDescriptor,
    accepted: Mutex<HashMap<String, SystemTime>>,
}

impl PushdownTasks {
    /// Creates a task set that validates against `descriptor`.
    #[must_use]
    pub fn new(descriptor: SchemaDescriptor) -> Self {
        Self {
            descriptor,
            accepted: Mutex::new(HashMap::new()),
        }
    }

    /// Validates `task` against the registered descriptor and records it when it passes.
    ///
    /// Nothing is recorded unless the whole plan validates.
    ///
    /// # Errors
    ///
    /// Returns the specific [`PushdownRejection`] naming the offending table, column, operation
    /// or TTL.
    pub fn accept(&self, task: &DetectionTask, now: SystemTime) -> Result<(), PushdownRejection> {
        let plan = task
            .pushdown_plan
            .as_ref()
            .ok_or(PushdownRejection::MissingPlan)?;
        if task.task_id.is_empty() {
            return Err(PushdownRejection::MissingTaskId);
        }
        check_identifier(&task.task_id)?;

        self.validate_plan(plan)?;
        let deadline =
            now.checked_add(validate_ttl(plan.ttl_ms)?)
                .ok_or(PushdownRejection::InvalidTtl {
                    ttl_ms: plan.ttl_ms,
                })?;

        self.record(task.task_id.clone(), deadline, now);
        Ok(())
    }

    /// Records a validated task's deadline, dropping entries that already expired.
    ///
    /// Re-accepting a task id that is already known refreshes its deadline; that is the seam a
    /// later renewal unit drives, so renewal needs no separate mechanism.
    fn record(&self, task_id: String, deadline: SystemTime, now: SystemTime) {
        let mut accepted = self.accepted.lock();
        accepted.retain(|_task_id, expires_at| *expires_at > now);
        accepted.insert(task_id, deadline);
    }

    /// Lifecycle state of `task_id` as of `now`.
    #[must_use]
    pub fn status(&self, task_id: &str, now: SystemTime) -> TaskStatus {
        match self.accepted.lock().get(task_id) {
            None => TaskStatus::Unknown,
            Some(expires_at) if *expires_at > now => TaskStatus::Active,
            Some(_expired) => TaskStatus::Expired,
        }
    }

    /// Number of accepted tasks still within their TTL as of `now`.
    #[must_use]
    pub fn active_count(&self, now: SystemTime) -> usize {
        self.accepted
            .lock()
            .values()
            .filter(|expires_at| **expires_at > now)
            .count()
    }

    /// Validates the plan's table, predicates and projection against the descriptor.
    fn validate_plan(&self, plan: &PushdownPlan) -> Result<(), PushdownRejection> {
        check_identifier(&plan.table)?;
        let table = self
            .descriptor
            .tables
            .iter()
            .find(|candidate| candidate.name == plan.table)
            .ok_or_else(|| PushdownRejection::UnknownTable {
                table: plan.table.clone(),
            })?;

        for predicate in &plan.predicates {
            validate_predicate(table, predicate)?;
        }
        for column in &plan.projection {
            check_identifier(column)?;
            find_column(table, column)?;
        }
        Ok(())
    }
}

/// Rejects an identifier longer than the fixed bound before it is used in any lookup.
const fn check_identifier(identifier: &str) -> Result<(), PushdownRejection> {
    if identifier.len() > MAX_IDENTIFIER_LENGTH {
        return Err(PushdownRejection::IdentifierTooLong {
            length: identifier.len(),
        });
    }
    Ok(())
}

/// Looks up a column the table declares, or names it as unadvertised.
fn find_column<'table>(
    table: &'table TableDescriptor,
    column: &str,
) -> Result<&'table ColumnDescriptor, PushdownRejection> {
    table
        .columns
        .iter()
        .find(|candidate| candidate.name == column)
        .ok_or_else(|| PushdownRejection::UnknownColumn {
            table: table.name.clone(),
            column: column.to_owned(),
        })
}

/// Validates one predicate's column, operation and value count.
fn validate_predicate(
    table: &TableDescriptor,
    predicate: &Predicate,
) -> Result<(), PushdownRejection> {
    check_identifier(&predicate.column)?;
    let column = find_column(table, &predicate.column)?;

    // `Predicate::op()` folds an operation newer than this build into `Unspecified`, which
    // `descriptor_op` then refuses; an op the collector cannot name is never pushable.
    let op =
        descriptor_op(predicate.op()).ok_or_else(|| PushdownRejection::UnspecifiedOperation {
            column: predicate.column.clone(),
        })?;

    if !column.supported_ops.contains(&op) {
        return Err(PushdownRejection::UnsupportedOperation {
            column: predicate.column.clone(),
            op: op.as_wire_name().to_owned(),
        });
    }

    let values = predicate.values.len();
    let arity_ok = if op == DescriptorOp::In {
        values >= 1
    } else {
        values == 1
    };
    if !arity_ok {
        return Err(PushdownRejection::InvalidArity {
            column: predicate.column.clone(),
            op: op.as_wire_name().to_owned(),
            values,
        });
    }
    Ok(())
}

/// Maps a wire operation onto the descriptor's operation vocabulary.
///
/// `None` means the operation is unset or newer than this build, which is always a refusal. The
/// wildcard arm exists because the protobuf enum is `#[non_exhaustive]` and may grow; it rejects,
/// so a variant this build predates can never be read as a comparison it is not.
const fn descriptor_op(op: PredicateOp) -> Option<DescriptorOp> {
    match op {
        PredicateOp::Eq => Some(DescriptorOp::Eq),
        PredicateOp::Ne => Some(DescriptorOp::Ne),
        PredicateOp::Lt => Some(DescriptorOp::Lt),
        PredicateOp::Le => Some(DescriptorOp::Le),
        PredicateOp::Gt => Some(DescriptorOp::Gt),
        PredicateOp::Ge => Some(DescriptorOp::Ge),
        PredicateOp::In => Some(DescriptorOp::In),
        PredicateOp::Like => Some(DescriptorOp::Like),
        PredicateOp::Regexp => Some(DescriptorOp::Regexp),
        PredicateOp::Unspecified => None,
        _unrecognized => None,
    }
}

/// Bounds a plan's TTL. Zero is a refusal, never "no expiry".
fn validate_ttl(ttl_ms: u64) -> Result<Duration, PushdownRejection> {
    let ttl = Duration::from_millis(ttl_ms);
    if ttl_ms == 0 || ttl > PUSHDOWN_TASK_TTL {
        return Err(PushdownRejection::InvalidTtl { ttl_ms });
    }
    Ok(ttl)
}
