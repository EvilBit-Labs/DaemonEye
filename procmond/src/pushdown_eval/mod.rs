//! Typed evaluation of pushed detection predicates against real process records (R20, R21).
//!
//! The agent lowers a rule into a pushed half — a conjunction of `{column, op, literal}`
//! predicates plus a projection — and keeps the residual. This module evaluates that typed payload
//! directly: **no SQL AST crosses the IPC boundary**, and `collector_core`'s `SqlTriggerEvaluator`
//! is untouched, so the detection dialect never reaches the collector.
//!
//! Three properties are load-bearing:
//!
//! - **A malformed plan is refused before it becomes active.** A literal whose kind does not match
//!   the column's declared type, a NULL literal against a `NOT NULL` column, and a `REGEXP`
//!   pattern beyond the fixed compile bounds are all static defects of the plan, checkable with no
//!   row in hand. All three are refused at acceptance, ahead of
//!   [`collector_core::PushdownTasks::accept`], so a refused task is never recorded.
//! - **The collector bounds its own patterns (R21).** A task arrives over IPC and may carry any
//!   pattern; the agent's load-time compile protects the agent only. Patterns are compiled here
//!   through this collector's own [`RegexCache`], under the same `detection_bounds` ceilings.
//! - **NULL is UNKNOWN, not false.** A column with no value short-circuits *before* the operation
//!   is dispatched, so `column != literal` over a NULL yields UNKNOWN like every other comparison.
//!   A conjunction admits a row only when every predicate is `Some(true)`.

pub mod conformance;
mod predicate;
pub mod schema;

use collector_core::{PushdownRejection, PushdownTasks, TaskStatus};
use daemoneye_eventbus::rpc::{ColumnType, PredicateOp as DescriptorOp};
use daemoneye_lib::detection::{RegexCache, RegexCacheStats, RegexRejection};
use daemoneye_lib::proto::{DetectionTask, Predicate, PredicateOp, ProcessRecord, PushdownPlan};
use predicate::{
    check_literal, compare_first, descriptor_op, in_holds, pattern_rejected, pattern_source,
    project,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::SystemTime;
use thiserror::Error;

pub use conformance::descriptor_with_conformance;
pub use schema::{DESCRIPTOR_VERSION, FieldValue, PROCESS_TABLE, process_schema_descriptor};

/// The collector id procmond advertises unless its registration config names another.
pub const DEFAULT_COLLECTOR_ID: &str = "procmond";

/// The row a collector returns: exactly the columns the plan's projection names, each carrying its
/// value or SQL NULL.
pub type ProjectedRow = BTreeMap<String, Option<FieldValue>>;

/// Why procmond refused or could not evaluate a pushed task.
///
/// [`PushdownRejection`] has no variant for a mistyped literal or an uncompilable pattern, so the
/// precise reason lives here and the trait boundary carries the coarser one. Callers that need the
/// specific defect — the conformance vectors, and an operator reading a log line — read this type.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PushdownError {
    /// The task failed the identity validation `collector_core` performs.
    #[error(transparent)]
    Identity(#[from] PushdownRejection),

    /// A literal's kind did not match the column's declared type.
    #[error(
        "column `{column}` is declared {column_type} and cannot be compared to a {literal_kind} literal"
    )]
    LiteralTypeMismatch {
        /// Column the offending predicate reads.
        column: String,
        /// Wire name of the column's declared type.
        column_type: &'static str,
        /// Wire name of the literal's kind.
        literal_kind: &'static str,
    },

    /// A NULL literal was pushed against a column declared `NOT NULL`.
    #[error("column `{column}` is declared NOT NULL, so a NULL literal can never match")]
    NullLiteralOnNonNullable {
        /// Column the offending predicate reads.
        column: String,
    },

    /// The task is not accepted, or its TTL elapsed without renewal.
    #[error("task `{task_id}` is not active, so its plan must not be evaluated")]
    TaskNotActive {
        /// The task that may not be evaluated.
        task_id: String,
    },

    /// A pattern exceeded this collector's own compile bounds, or did not compile at all (R21).
    #[error("pattern on column `{column}` cannot be compiled by this collector: {rejection}")]
    PatternRejected {
        /// Column the offending predicate reads.
        column: String,
        /// The bounded compiler's own reason.
        rejection: RegexRejection,
    },
}

impl PushdownError {
    /// The column a refusal names, when it names one.
    ///
    /// The trait boundary returns [`PushdownRejection`], which has no variant for a mistyped
    /// literal or an uncompilable pattern; the caller uses this to name the offender in the
    /// coarser rejection it must hand back.
    #[must_use]
    pub const fn column(&self) -> Option<&str> {
        match *self {
            Self::Identity(_) | Self::TaskNotActive { .. } => None,
            Self::LiteralTypeMismatch { ref column, .. }
            | Self::NullLiteralOnNonNullable { ref column }
            | Self::PatternRejected { ref column, .. } => Some(column.as_str()),
        }
    }

    /// A refusal naming a column this collector does not serve.
    pub(super) fn unknown_column(column: &str) -> Self {
        Self::Identity(PushdownRejection::UnknownColumn {
            table: PROCESS_TABLE.to_owned(),
            column: column.to_owned(),
        })
    }

    /// A refusal for an operation this build cannot name.
    pub(super) fn unusable_operation(column: &str) -> Self {
        Self::Identity(PushdownRejection::UnspecifiedOperation {
            column: column.to_owned(),
        })
    }

    /// A refusal for a predicate carrying the wrong number of values.
    pub(super) fn bad_arity(predicate: &Predicate) -> Self {
        Self::Identity(PushdownRejection::InvalidArity {
            column: predicate.column.clone(),
            op: descriptor_op(predicate.op())
                .map_or("PREDICATE_OP_UNSPECIFIED", DescriptorOp::as_wire_name)
                .to_owned(),
            values: predicate.values.len(),
        })
    }
}

/// procmond's evaluator for pushed typed predicates.
///
/// It owns three things that must agree: the descriptor it advertises, the task set that validates
/// against that descriptor, and the bounded pattern cache R21 requires the collector to keep for
/// itself.
#[derive(Debug)]
pub struct PushdownEvaluator {
    descriptor: daemoneye_eventbus::rpc::SchemaDescriptor,
    tasks: PushdownTasks,
    patterns: RegexCache,
}

impl PushdownEvaluator {
    /// Creates an evaluator advertising `collector_id`'s process schema.
    #[must_use]
    pub fn new(collector_id: &str) -> Self {
        let descriptor = process_schema_descriptor(collector_id);
        Self {
            tasks: PushdownTasks::new(descriptor.clone()),
            descriptor,
            patterns: RegexCache::new(),
        }
    }

    /// The schema this collector advertises and validates against.
    #[must_use]
    pub const fn descriptor(&self) -> &daemoneye_eventbus::rpc::SchemaDescriptor {
        &self.descriptor
    }

    /// Validates `task` and records it when it passes.
    ///
    /// The typed checks and the pattern compile run *before* `collector_core` records anything, so
    /// a task refused for a mistyped literal or an over-bounds pattern never becomes active.
    ///
    /// # Errors
    ///
    /// Returns the specific [`PushdownError`] naming the offending column, literal or pattern.
    pub fn accept(&self, task: &DetectionTask, now: SystemTime) -> Result<(), PushdownError> {
        if let Some(plan) = task.pushdown_plan.as_ref() {
            self.validate_types(plan)?;
        }
        self.tasks
            .accept(task, now)
            .map_err(PushdownError::Identity)
    }

    /// Whether `task_id` is accepted and still within its TTL.
    #[must_use]
    pub fn is_active(&self, task_id: &str, now: SystemTime) -> bool {
        self.tasks.status(task_id, now) == TaskStatus::Active
    }

    /// A snapshot of the pattern cache's counters.
    #[must_use]
    pub fn pattern_stats(&self) -> RegexCacheStats {
        self.patterns.stats()
    }

    /// Whether `pattern` is resident in this collector's bounded cache.
    #[must_use]
    pub fn is_pattern_cached(&self, pattern: &str) -> bool {
        self.patterns.is_cached(pattern)
    }

    /// How many compiled patterns the bounded cache currently holds.
    #[must_use]
    pub fn cached_pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Evaluates an accepted task's plan, refusing once its TTL has elapsed.
    ///
    /// This is the entry point real collection uses: a task whose renewal was missed stops
    /// producing rows, which is what makes the TTL mean anything. [`Self::evaluate`] is the
    /// lifetime-free primitive beneath it, for callers that have already checked.
    ///
    /// # Errors
    ///
    /// Returns [`PushdownError::TaskNotActive`] when `task_id` is unknown or expired, or whatever
    /// [`Self::evaluate`] returns.
    pub fn evaluate_task(
        &self,
        task_id: &str,
        plan: &PushdownPlan,
        records: &[ProcessRecord],
        now: SystemTime,
    ) -> Result<Vec<ProjectedRow>, PushdownError> {
        if !self.is_active(task_id, now) {
            return Err(PushdownError::TaskNotActive {
                task_id: task_id.to_owned(),
            });
        }
        self.evaluate(plan, records)
    }

    /// Evaluates `plan` against `records`, returning the projected columns of every admitted row.
    ///
    /// This does not consult the task's TTL; [`Self::evaluate_task`] is the gated entry point.
    ///
    /// # Errors
    ///
    /// Returns a [`PushdownError`] when the plan names something this collector does not serve, or
    /// carries a literal or pattern it cannot evaluate.
    pub fn evaluate(
        &self,
        plan: &PushdownPlan,
        records: &[ProcessRecord],
    ) -> Result<Vec<ProjectedRow>, PushdownError> {
        if plan.table != PROCESS_TABLE {
            return Err(PushdownError::Identity(PushdownRejection::UnknownTable {
                table: plan.table.clone(),
            }));
        }
        let projection = self.projected_columns(plan)?;
        let mut rows = Vec::new();
        for record in records {
            if self.admits(plan, record)? {
                rows.push(project(record, &projection)?);
            }
        }
        Ok(rows)
    }

    /// Whether every predicate in the plan holds for `record`.
    ///
    /// UNKNOWN does not admit: SQL's three-valued `AND` requires every conjunct to be true.
    fn admits(&self, plan: &PushdownPlan, record: &ProcessRecord) -> Result<bool, PushdownError> {
        for predicate in &plan.predicates {
            if self.predicate_holds(predicate, record)? != Some(true) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Evaluates one predicate. `None` is SQL UNKNOWN.
    ///
    /// The NULL check runs before the operation is dispatched. Handling NULL per operation is how
    /// `!=` silently becomes true over a NULL and the pushed and residual halves disagree.
    fn predicate_holds(
        &self,
        predicate: &Predicate,
        record: &ProcessRecord,
    ) -> Result<Option<bool>, PushdownError> {
        let Some(observed) = schema::field_value(record, &predicate.column)? else {
            return Ok(None);
        };
        // KTD8: the wildcard arm refuses. `PredicateOp` is `#[non_exhaustive]`, and an operation
        // this build cannot name must never be read as a comparison it is not.
        match predicate.op() {
            PredicateOp::Eq => Ok(compare_first(&observed, predicate)?.map(Ordering::is_eq)),
            PredicateOp::Ne => Ok(compare_first(&observed, predicate)?.map(Ordering::is_ne)),
            PredicateOp::Lt => Ok(compare_first(&observed, predicate)?.map(Ordering::is_lt)),
            PredicateOp::Le => Ok(compare_first(&observed, predicate)?.map(Ordering::is_le)),
            PredicateOp::Gt => Ok(compare_first(&observed, predicate)?.map(Ordering::is_gt)),
            PredicateOp::Ge => Ok(compare_first(&observed, predicate)?.map(Ordering::is_ge)),
            PredicateOp::In => in_holds(&observed, predicate),
            PredicateOp::Like | PredicateOp::Regexp => self.pattern_holds(&observed, predicate),
            PredicateOp::Unspecified => Err(PushdownError::unusable_operation(&predicate.column)),
            _unrecognized => Err(PushdownError::unusable_operation(&predicate.column)),
        }
    }

    /// Evaluates a `LIKE` or `REGEXP` predicate through this collector's bounded cache.
    fn pattern_holds(
        &self,
        observed: &FieldValue,
        predicate: &Predicate,
    ) -> Result<Option<bool>, PushdownError> {
        let FieldValue::Str(ref text) = *observed else {
            return Err(PushdownError::LiteralTypeMismatch {
                column: predicate.column.clone(),
                column_type: ColumnType::String.as_wire_name(),
                literal_kind: "pattern",
            });
        };
        self.pattern_matches(predicate, text).map(Some)
    }

    /// Compiles the predicate's pattern under this collector's own bounds (R21) and reports
    /// whether it matches `text`.
    fn pattern_matches(&self, predicate: &Predicate, text: &str) -> Result<bool, PushdownError> {
        let source = pattern_source(predicate)?;
        let compiled = self
            .patterns
            .get_or_compile(&source)
            .map_err(|rejection| pattern_rejected(predicate, rejection))?;
        Ok(compiled.is_match(text))
    }

    /// Refuses a plan whose literals do not match the declared column types, and compiles every
    /// pattern it pushes, before any of it is recorded.
    ///
    /// A predicate this collector cannot resolve — unknown table, unknown column, unadvertised
    /// operation — is left alone here so that `collector_core` names it, rather than being
    /// reported twice with two different words.
    fn validate_types(&self, plan: &PushdownPlan) -> Result<(), PushdownError> {
        let Some(table) = self
            .descriptor
            .tables
            .iter()
            .find(|candidate| candidate.name == plan.table)
        else {
            return Ok(());
        };
        for predicate in &plan.predicates {
            let Some(column) = table
                .columns
                .iter()
                .find(|candidate| candidate.name == predicate.column)
            else {
                continue;
            };
            let Some(op) = descriptor_op(predicate.op()) else {
                continue;
            };
            if !column.supported_ops.contains(&op) {
                continue;
            }
            for value in &predicate.values {
                check_literal(column, value)?;
            }
            if matches!(op, DescriptorOp::Like | DescriptorOp::Regexp) {
                // Compiled here so an over-bounds pattern refuses the task ahead of the record
                // below. A pattern that does compile stays resident in the bounded cache even if
                // the task is then refused for another reason; the cache is fixed-size, so that
                // costs a slot and nothing else.
                let source = pattern_source(predicate)?;
                let _compiled = self
                    .patterns
                    .get_or_compile(&source)
                    .map_err(|rejection| pattern_rejected(predicate, rejection))?;
            }
        }
        Ok(())
    }

    /// The column names a plan's projection asks for, in descriptor order for an empty projection.
    fn projected_columns(&self, plan: &PushdownPlan) -> Result<Vec<String>, PushdownError> {
        let Some(table) = self
            .descriptor
            .tables
            .iter()
            .find(|candidate| candidate.name == plan.table)
        else {
            return Err(PushdownError::Identity(PushdownRejection::UnknownTable {
                table: plan.table.clone(),
            }));
        };
        if plan.projection.is_empty() {
            return Ok(table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect());
        }
        for name in &plan.projection {
            if !table.columns.iter().any(|column| column.name == *name) {
                return Err(PushdownError::unknown_column(name));
            }
        }
        Ok(plan.projection.clone())
    }
}
