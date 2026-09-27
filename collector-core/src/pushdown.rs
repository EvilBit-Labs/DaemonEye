//! Collector-side acceptance of pushed detection tasks (R19).
//!
//! A [`daemoneye_lib::proto::PushdownPlan`] arrives from the agent naming a table,
//! a conjunction of predicates and a projection. This module validates that plan against the
//! [`SchemaDescriptor`] the collector registered and tracks the lifetime of the tasks it accepts.
//!
//! Three properties are load-bearing:
//!
//! - **A task is accepted whole or refused whole.** Validation completes before anything is
//!   recorded, so a plan whose first predicate is valid and whose second is not leaves no trace.
//! - **Validation runs in three passes, in this order.** Structural bounds (task id, identifier
//!   lengths, plan counts, TTL) come first, then the descriptor and typed checks, then pattern
//!   compilation. A plan that will be refused anyway must not first compile a pattern into the
//!   bounded cache, where it would evict a resident entry and force a later recompile.
//! - **Expiry is observable.** An accepted task moves from [`TaskStatus::Active`] to
//!   [`TaskStatus::Expired`] at its deadline, read through an explicit `now` rather than wall
//!   time, so a missed renewal ends the work.
//!
//! Every check here is generic: a third-party collector that delegates to
//! [`PushdownTasks::accept`] gets literal-kind, nullability and bounded-pattern validation without
//! writing any of it, and under the same `detection_bounds` ceilings the agent applies at rule
//! load. Evaluating the predicates against real data is the owning collector's job; this module
//! only decides whether a task may be evaluated at all.

use daemoneye_eventbus::rpc::{
    ColumnDescriptor, ColumnType as DescriptorColumnType, PredicateOp as DescriptorOp,
    SchemaDescriptor, TableDescriptor,
};
use daemoneye_lib::detection::{RegexCache, RegexRejection};
use daemoneye_lib::detection_bounds::{
    MAX_IDENTIFIER_LENGTH, MAX_IN_VALUES, MAX_PREDICATES_PER_PLAN, MAX_PROJECTION_COLUMNS,
    PUSHDOWN_TASK_TTL,
};
use daemoneye_lib::proto::{
    ColumnType, DetectionTask, Literal, Predicate, PredicateOp, PushdownPlan, literal,
};
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

    /// The plan named more predicates, projected columns or `IN` values than the fixed bound.
    ///
    /// Identifier *lengths* are bounded by [`Self::IdentifierTooLong`]; this bounds the *counts*,
    /// which is the other half of what a plan can inflate. Each is evaluated per record across a
    /// scrape of 10,000+ processes, so an unbounded count is unbounded work.
    #[error("plan carries {count} {what}, beyond the bound of {limit}")]
    PlanTooLarge {
        /// What was over the bound, named for the operator: `predicates`, `projected columns` or
        /// `IN values`.
        what: &'static str,
        /// How many the plan carried.
        count: usize,
        /// The fixed ceiling.
        limit: usize,
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

/// An accepted task: the plan it was validated against, and when it stops being evaluable.
///
/// The plan is kept, not discarded. Holding only the deadline made the task id the whole binding,
/// so an evaluator had to be handed a plan by its caller and could be handed a *different* one —
/// evaluating something this module never validated, which is exactly what validation is for.
#[derive(Debug, Clone)]
struct AcceptedTask {
    plan: PushdownPlan,
    deadline: SystemTime,
}

/// The pushdown tasks a single collector has accepted, and the descriptor it validates against.
///
/// The descriptor held here is the same value the collector advertises in its registration
/// exchange; validating against anything else would let a collector accept work it never claimed.
#[derive(Debug)]
pub struct PushdownTasks {
    descriptor: SchemaDescriptor,
    accepted: Mutex<HashMap<String, AcceptedTask>>,
    patterns: RegexCache,
}

impl PushdownTasks {
    /// Creates a task set that validates against `descriptor`.
    #[must_use]
    pub fn new(descriptor: SchemaDescriptor) -> Self {
        Self {
            descriptor,
            accepted: Mutex::new(HashMap::new()),
            patterns: RegexCache::new(),
        }
    }

    /// The bounded pattern cache this collector compiles every pushed pattern through (R21).
    ///
    /// One cache per collector, owned here rather than by the collector, so acceptance and
    /// evaluation share both the compiled program and the fixed entry budget. A collector that
    /// kept its own would compile every pattern twice and hold two caches against one stated
    /// memory bound.
    #[must_use]
    pub const fn patterns(&self) -> &RegexCache {
        &self.patterns
    }

    /// Checks only what a plan's *shape* must satisfy, recording nothing and compiling nothing.
    ///
    /// This is the first of [`Self::accept`]'s three passes, and is public so a collector that
    /// wants its own checks before acceptance can still run these first: the task id is validated
    /// before any caller logs it, and a plan beyond the fixed counts is refused before any pattern
    /// can reach the bounded cache.
    ///
    /// # Errors
    ///
    /// Returns the [`PushdownRejection`] naming the missing plan or task id, the over-long
    /// identifier, or the count that exceeded its bound.
    pub fn validate_bounds(task: &DetectionTask) -> Result<(), PushdownRejection> {
        let plan = task
            .pushdown_plan
            .as_ref()
            .ok_or(PushdownRejection::MissingPlan)?;
        if task.task_id.is_empty() {
            return Err(PushdownRejection::MissingTaskId);
        }
        check_identifier(&task.task_id)?;
        check_count("predicates", plan.predicates.len(), MAX_PREDICATES_PER_PLAN)?;
        check_count(
            "projected columns",
            plan.projection.len(),
            MAX_PROJECTION_COLUMNS,
        )?;
        check_identifier(&plan.table)?;
        for predicate in &plan.predicates {
            check_identifier(&predicate.column)?;
        }
        for column in &plan.projection {
            check_identifier(column)?;
        }
        validate_ttl(plan.ttl_ms).map(|_bounded| ())
    }

    /// Validates `task` against the registered descriptor and records it when it passes.
    ///
    /// Nothing is recorded unless the whole plan validates, and nothing is compiled until the
    /// whole plan has passed every check that needs no compiler.
    ///
    /// # Errors
    ///
    /// Returns the specific [`PushdownRejection`] naming the offending table, column, operation,
    /// literal, pattern or TTL.
    pub fn accept(&self, task: &DetectionTask, now: SystemTime) -> Result<(), PushdownRejection> {
        Self::validate_bounds(task)?;
        let plan = task
            .pushdown_plan
            .as_ref()
            .ok_or(PushdownRejection::MissingPlan)?;

        self.validate_plan(plan)?;
        self.compile_patterns(plan)?;
        let deadline =
            now.checked_add(validate_ttl(plan.ttl_ms)?)
                .ok_or(PushdownRejection::InvalidTtl {
                    ttl_ms: plan.ttl_ms,
                })?;

        self.record(task.task_id.clone(), plan.clone(), deadline, now);
        Ok(())
    }

    /// Records a validated task's plan and deadline, dropping entries that already expired.
    ///
    /// Re-accepting a task id that is already known refreshes both its deadline *and* its plan;
    /// that is the seam renewal drives, so renewal needs no separate mechanism — and a renewal
    /// that narrows a rule takes effect rather than being shadowed by the plan first accepted.
    fn record(&self, task_id: String, plan: PushdownPlan, deadline: SystemTime, now: SystemTime) {
        let mut accepted = self.accepted.lock();
        accepted.retain(|_task_id, task| task.deadline > now);
        accepted.insert(task_id, AcceptedTask { plan, deadline });
    }

    /// Lifecycle state of `task_id` as of `now`.
    #[must_use]
    pub fn status(&self, task_id: &str, now: SystemTime) -> TaskStatus {
        match self.accepted.lock().get(task_id) {
            None => TaskStatus::Unknown,
            Some(task) if task.deadline > now => TaskStatus::Active,
            Some(_expired) => TaskStatus::Expired,
        }
    }

    /// The validated plan `task_id` was accepted with, if it is still within its TTL.
    ///
    /// This is what an evaluator must evaluate. Taking a plan from the caller instead would make
    /// the task id a name for nothing: validation ran over *this* plan, and no other.
    #[must_use]
    pub fn active_plan(&self, task_id: &str, now: SystemTime) -> Option<PushdownPlan> {
        self.accepted
            .lock()
            .get(task_id)
            .filter(|task| task.deadline > now)
            .map(|task| task.plan.clone())
    }

    /// Number of accepted tasks still within their TTL as of `now`.
    #[must_use]
    pub fn active_count(&self, now: SystemTime) -> usize {
        self.accepted
            .lock()
            .values()
            .filter(|task| task.deadline > now)
            .count()
    }

    /// Validates the plan's table, predicates and projection against the descriptor.
    ///
    /// Bounds are already checked ([`Self::validate_bounds`]) and nothing here compiles a pattern:
    /// every defect this pass can find is found before the compiler is reached.
    fn validate_plan(&self, plan: &PushdownPlan) -> Result<(), PushdownRejection> {
        let table = self.table(plan)?;
        for predicate in &plan.predicates {
            validate_predicate(table, predicate)?;
        }
        for column in &plan.projection {
            find_column(table, column)?;
        }
        Ok(())
    }

    /// Compiles every pattern the plan pushes, under this collector's own bounds (R21).
    ///
    /// The last pass, run only once the whole plan is otherwise valid. Compiling earlier would let
    /// a plan that is refused for an unrelated defect evict a resident entry from the fixed-size
    /// cache, forcing a live task's pattern to be recompiled on its next evaluation.
    fn compile_patterns(&self, plan: &PushdownPlan) -> Result<(), PushdownRejection> {
        for predicate in &plan.predicates {
            // `matches!` rather than a match arm: an operation this build cannot name carries no
            // pattern, and `validate_predicate` has already refused it.
            if !matches!(predicate.op(), PredicateOp::Like | PredicateOp::Regexp) {
                continue;
            }
            let source = pattern_source(predicate)?;
            self.patterns
                .get_or_compile(&source)
                .map(|_compiled| ())
                .map_err(|rejection| PushdownRejection::PatternRejected {
                    column: predicate.column.clone(),
                    rejection,
                })?;
        }
        Ok(())
    }

    /// The descriptor's table the plan names, or a refusal naming the unserved table.
    fn table(&self, plan: &PushdownPlan) -> Result<&TableDescriptor, PushdownRejection> {
        self.descriptor
            .tables
            .iter()
            .find(|candidate| candidate.name == plan.table)
            .ok_or_else(|| PushdownRejection::UnknownTable {
                table: plan.table.clone(),
            })
    }
}

/// Rejects a plan carrying more of something than the fixed bound allows.
const fn check_count(
    what: &'static str,
    count: usize,
    limit: usize,
) -> Result<(), PushdownRejection> {
    if count > limit {
        return Err(PushdownRejection::PlanTooLarge { what, count, limit });
    }
    Ok(())
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

/// Validates one predicate's column, operation, value count and literals.
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
        // `IN` is the one operation whose arity is unbounded by shape, and its list is scanned per
        // record, so the ceiling is checked here rather than left to the arity test below.
        check_count("IN values", values, MAX_IN_VALUES)?;
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

    for value in &predicate.values {
        check_literal(column, value)?;
    }
    Ok(())
}

/// Refuses a literal whose kind does not match the column's declared type, a NULL literal against
/// a column declared `NOT NULL`, and an empty literal oneof.
///
/// All three are static defects of the plan: none needs a row to detect, and none can ever match.
fn check_literal(column: &ColumnDescriptor, value: &Literal) -> Result<(), PushdownRejection> {
    let Some(ref inner) = value.value else {
        return Err(PushdownRejection::LiteralTypeMismatch {
            column: column.name.clone(),
            column_type: column.column_type.as_wire_name().to_owned(),
            literal_kind: "unset".to_owned(),
        });
    };
    if matches!(*inner, literal::Value::NullValue(_marker)) {
        if column.nullable {
            return Ok(());
        }
        return Err(PushdownRejection::NullLiteralOnNonNullable {
            column: column.name.clone(),
        });
    }
    if literal_matches_type(inner, column.column_type) {
        return Ok(());
    }
    Err(PushdownRejection::LiteralTypeMismatch {
        column: column.name.clone(),
        column_type: column.column_type.as_wire_name().to_owned(),
        literal_kind: literal_kind(inner).to_owned(),
    })
}

/// Whether a literal's kind is the one a column of `column_type` compares against.
///
/// `Unspecified`, and any type newer than this build, match nothing: a literal whose column type
/// cannot be named is never comparable.
const fn literal_matches_type(value: &literal::Value, column_type: DescriptorColumnType) -> bool {
    match column_type {
        DescriptorColumnType::String => matches!(*value, literal::Value::StringValue(_)),
        DescriptorColumnType::Int => matches!(*value, literal::Value::IntValue(_)),
        DescriptorColumnType::Uint => matches!(*value, literal::Value::UintValue(_)),
        DescriptorColumnType::Float => matches!(*value, literal::Value::FloatValue(_)),
        DescriptorColumnType::Bool => matches!(*value, literal::Value::BoolValue(_)),
        DescriptorColumnType::Unspecified => false,
        _unrecognized => false,
    }
}

/// Wire name of a literal's kind, for a refusal message.
///
/// Description only. A literal kind this build cannot name is refused by
/// `literal_matches_type`, which never returns true for one.
#[must_use]
pub const fn literal_kind(value: &literal::Value) -> &'static str {
    match *value {
        literal::Value::StringValue(_) => "string",
        literal::Value::IntValue(_) => "int",
        literal::Value::UintValue(_) => "uint",
        literal::Value::FloatValue(_) => "float",
        literal::Value::BoolValue(_) => "bool",
        literal::Value::NullValue(_) => "null",
        ref _unrecognized => "unrecognized",
    }
}

/// The text a literal carries, if it is a text literal at all.
///
/// The wildcard arm yields `None`, which every caller reads as a refusal. `literal::Value` is
/// `#[non_exhaustive]`, and a kind this build cannot name is never text.
#[must_use]
#[allow(clippy::wildcard_enum_match_arm)]
pub const fn literal_text(value: &literal::Value) -> Option<&str> {
    match *value {
        literal::Value::StringValue(ref text) => Some(text.as_str()),
        ref _non_text => None,
    }
}

/// The regular-expression source a `LIKE` or `REGEXP` predicate compiles to.
///
/// `LIKE` is translated rather than given a second matcher: one bounded compiler, one cache. Every
/// collector must derive the source the same way, or a pattern validated at acceptance would be
/// cached under a key its own evaluation never looks up.
///
/// # Errors
///
/// Returns [`PushdownRejection::LiteralTypeMismatch`] when the predicate's literal is not a string
/// — including a NULL literal, which can never be a pattern — and
/// [`PushdownRejection::UnspecifiedOperation`] when the operation is not a pattern operation.
// The wildcard arm refuses. `PredicateOp` is `#[non_exhaustive]`; an operation newer than this
// build must not be silently treated as `REGEXP`.
#[allow(clippy::wildcard_enum_match_arm)]
pub fn pattern_source(predicate: &Predicate) -> Result<String, PushdownRejection> {
    let source = pattern_text(predicate)?;
    match predicate.op() {
        PredicateOp::Like => Ok(like_to_regex(source)),
        PredicateOp::Regexp => Ok(source.to_owned()),
        _unsupported => Err(PushdownRejection::UnspecifiedOperation {
            column: predicate.column.clone(),
        }),
    }
}

/// The predicate's single literal as the text a pattern operation requires.
fn pattern_text(predicate: &Predicate) -> Result<&str, PushdownRejection> {
    let mismatch = |literal_kind: &str| PushdownRejection::LiteralTypeMismatch {
        column: predicate.column.clone(),
        column_type: DescriptorColumnType::String.as_wire_name().to_owned(),
        literal_kind: literal_kind.to_owned(),
    };
    let value = predicate
        .values
        .first()
        .and_then(|literal| literal.value.as_ref())
        .ok_or_else(|| mismatch("unset"))?;
    literal_text(value).ok_or_else(|| mismatch(literal_kind(value)))
}

/// Translates a SQL `LIKE` pattern into an anchored regular expression.
///
/// `%` and `_` are the only wildcards; everything else is literal, and ASCII punctuation is
/// escaped so a pattern cannot smuggle regex syntax through `LIKE`. There is no `ESCAPE` clause,
/// so a backslash in a `LIKE` pattern matches a backslash.
#[must_use]
pub fn like_to_regex(pattern: &str) -> String {
    let mut translated = String::from("(?s)^");
    for character in pattern.chars() {
        match character {
            '%' => translated.push_str(".*"),
            '_' => translated.push('.'),
            literal_character => {
                if literal_character.is_ascii_punctuation() {
                    translated.push('\\');
                }
                translated.push(literal_character);
            }
        }
    }
    translated.push('$');
    translated
}

/// Maps a protobuf operation onto the descriptor's operation vocabulary.
///
/// `None` means the operation is unset or newer than this build, which is always a refusal. The
/// wildcard arm exists because the protobuf enum is `#[non_exhaustive]` and may grow; it rejects,
/// so a variant this build predates can never be read as a comparison it is not.
///
/// This crate is the only one that depends on both sides of the mirror, so the three conversions
/// between them live here and every caller shares one table per direction rather than keeping its
/// own copy.
#[must_use]
pub const fn descriptor_op(op: PredicateOp) -> Option<DescriptorOp> {
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

/// Maps a descriptor operation onto the protobuf vocabulary, the reverse of [`descriptor_op`].
///
/// `None` for an operation that is unset or newer than this build. The wildcard arm refuses for
/// the same reason [`descriptor_op`]'s does: `PredicateOp` is `#[non_exhaustive]`, and an
/// operation this build cannot name must never be read as one it is not.
#[must_use]
pub const fn proto_op(op: DescriptorOp) -> Option<PredicateOp> {
    match op {
        DescriptorOp::Eq => Some(PredicateOp::Eq),
        DescriptorOp::Ne => Some(PredicateOp::Ne),
        DescriptorOp::Lt => Some(PredicateOp::Lt),
        DescriptorOp::Le => Some(PredicateOp::Le),
        DescriptorOp::Gt => Some(PredicateOp::Gt),
        DescriptorOp::Ge => Some(PredicateOp::Ge),
        DescriptorOp::In => Some(PredicateOp::In),
        DescriptorOp::Like => Some(PredicateOp::Like),
        DescriptorOp::Regexp => Some(PredicateOp::Regexp),
        DescriptorOp::Unspecified => None,
        _unrecognized => None,
    }
}

/// Maps a descriptor column type onto the protobuf vocabulary.
///
/// `None` for a type that is unset or newer than this build; `ColumnType` is `#[non_exhaustive]`,
/// and a type this build cannot name carries no claim about what a literal may be compared to.
#[must_use]
pub const fn proto_column_type(column_type: DescriptorColumnType) -> Option<ColumnType> {
    match column_type {
        DescriptorColumnType::String => Some(ColumnType::String),
        DescriptorColumnType::Int => Some(ColumnType::Int),
        DescriptorColumnType::Uint => Some(ColumnType::Uint),
        DescriptorColumnType::Float => Some(ColumnType::Float),
        DescriptorColumnType::Bool => Some(ColumnType::Bool),
        DescriptorColumnType::Unspecified => None,
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
