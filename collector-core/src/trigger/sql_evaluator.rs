//! SQL-to-IPC integration for trigger condition evaluation.
//!
//! This submodule contains [`SqlTriggerEvaluator`], which compiles trigger
//! conditions, parses constrained SQL predicate expressions, and evaluates
//! them against process data to produce trigger requests.

#![allow(clippy::as_conversions)]
#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::pattern_type_mismatch)]
#![allow(clippy::integer_division)]
#![allow(clippy::unnecessary_wraps)]

use crate::event::TriggerRequest;
use serde::{Deserialize, Serialize};
use sqlparser::{dialect::GenericDialect, parser::Parser};
use std::collections::HashMap;
use std::time::SystemTime;
use tracing::warn;
use uuid::Uuid;

use super::types::{ConditionType, ProcessTriggerData, TriggerCondition, TriggerError};

/// SQL-to-IPC integration for trigger condition evaluation.
#[derive(Debug)]
pub struct SqlTriggerEvaluator {
    /// SQL dialect for parsing
    dialect: GenericDialect,

    /// Compiled trigger conditions mapped by collector
    compiled_conditions: HashMap<String, Vec<CompiledTriggerCondition>>,

    /// Performance statistics
    evaluation_stats: SqlEvaluationStats,
}

/// Compiled trigger condition for efficient evaluation.
#[derive(Debug, Clone)]
pub struct CompiledTriggerCondition {
    /// Original condition
    pub condition: TriggerCondition,

    /// Compiled SQL predicate (if applicable)
    pub compiled_predicate: Option<sqlparser::ast::Expr>,

    /// Evaluation statistics
    pub stats: ConditionEvaluationStats,
}

/// Statistics for SQL trigger evaluation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SqlEvaluationStats {
    /// Total evaluations performed
    pub total_evaluations: u64,

    /// Successful matches
    pub successful_matches: u64,

    /// Evaluation errors
    pub evaluation_errors: u64,

    /// Average evaluation time (microseconds)
    pub avg_evaluation_time_us: u64,

    /// SQL parsing errors
    pub sql_parsing_errors: u64,
}

/// Statistics for individual condition evaluation.
#[derive(Debug, Clone, Default)]
pub struct ConditionEvaluationStats {
    /// Number of times this condition was evaluated
    pub evaluation_count: u64,

    /// Number of times this condition matched
    pub match_count: u64,

    /// Number of times this condition did not match
    pub no_match_count: u64,

    /// Total evaluation time (microseconds)
    pub total_evaluation_time_us: u64,

    /// Last evaluation timestamp
    pub last_evaluation: Option<SystemTime>,

    /// Last time this condition matched
    pub last_match: Option<SystemTime>,

    /// Last time this condition did not match
    pub last_no_match: Option<SystemTime>,
}

impl SqlTriggerEvaluator {
    /// Creates a new SQL trigger evaluator.
    pub fn new() -> Self {
        Self {
            dialect: GenericDialect {},
            compiled_conditions: HashMap::new(),
            evaluation_stats: SqlEvaluationStats::default(),
        }
    }

    /// Registers trigger conditions for a collector.
    pub fn register_collector_conditions(
        &mut self,
        collector_id: String,
        conditions: Vec<TriggerCondition>,
    ) -> Result<(), TriggerError> {
        let mut compiled_conditions = Vec::new();

        for condition in conditions {
            let compiled = self.compile_condition(condition.clone())?;
            compiled_conditions.push(compiled);
        }

        self.compiled_conditions
            .insert(collector_id, compiled_conditions);
        Ok(())
    }

    /// Compiles a trigger condition for efficient evaluation.
    fn compile_condition(
        &self,
        condition: TriggerCondition,
    ) -> Result<CompiledTriggerCondition, TriggerError> {
        #[allow(clippy::wildcard_enum_match_arm)]
        let compiled_predicate = match &condition.condition_type {
            ConditionType::Custom(sql_expr) => {
                // Parse SQL expression for custom conditions
                match self.parse_sql_expression(sql_expr) {
                    Ok(expr) => Some(expr),
                    Err(e) => {
                        warn!(
                            condition_id = %condition.id,
                            error = %e,
                            "Failed to compile SQL expression for trigger condition"
                        );
                        None
                    }
                }
            }
            _ => None, // Built-in conditions don't need SQL compilation
        };

        Ok(CompiledTriggerCondition {
            condition,
            compiled_predicate,
            stats: ConditionEvaluationStats::default(),
        })
    }

    /// Parses a SQL WHERE clause expression from a string into an AST.
    ///
    /// This function is designed to parse simple SQL predicate expressions suitable for
    /// filtering process data. It has specific limitations:
    ///
    /// - **Predicate-only**: The input string must represent only the WHERE clause content,
    ///   not a full `SELECT` statement.
    /// - **No table references**: Expressions should not contain table-qualified identifiers
    ///   (e.g., `processes.name`). Only unqualified column names (e.g., `name`) are supported.
    /// - **No subqueries**: Subqueries (e.g., `(SELECT ...)`) are not allowed.
    /// - **Unqualified column names**: All column names are assumed to refer to fields
    ///   within the `ProcessInfo` structure.
    ///
    /// # Arguments
    /// * `sql_expr` - The SQL WHERE clause expression string.
    ///
    /// # Returns
    /// A `Result` containing the parsed `Expr` AST node on success, or a `TriggerError` on failure.
    ///
    /// # Errors
    /// Returns `TriggerError::SqlParsingError` if the expression is invalid or contains
    /// disallowed patterns.
    ///
    /// # Examples
    ///
    /// **Valid expressions:**
    /// ```ignore
    /// let expr = parse_sql_expression("pid > 1000 AND name LIKE 'chrome%'")?;
    /// ```
    ///
    /// **Invalid expressions (will return an error):**
    /// ```ignore
    /// // Contains a disallowed "SELECT" keyword
    /// parse_sql_expression("pid IN (SELECT id FROM other_table)"); // Error
    ///
    /// // Contains a table-qualified identifier
    /// parse_sql_expression("processes.pid > 100"); // Error
    /// ```
    fn parse_sql_expression(&self, sql_expr: &str) -> Result<sqlparser::ast::Expr, TriggerError> {
        // Lightweight validation to reject disallowed patterns before parsing
        let expr_upper = sql_expr.to_uppercase();
        if expr_upper.contains("SELECT")
            || expr_upper.contains("FROM")
            || sql_expr.contains('.')
            || (sql_expr.contains('(') && expr_upper.contains("SELECT"))
        {
            return Err(TriggerError::SqlParsingError(format!(
                "Expression contains disallowed patterns (e.g., SELECT, FROM, table-qualified identifiers, subqueries): {sql_expr}"
            )));
        }

        // Wrap the expression in a SELECT statement for parsing
        let sql = format!("SELECT * FROM dummy WHERE {sql_expr}");

        let statements = Parser::parse_sql(&self.dialect, &sql).map_err(|e| {
            TriggerError::SqlParsingError(format!("Failed to parse SQL expression: {e}"))
        })?;

        if let Some(sqlparser::ast::Statement::Query(query)) = statements.first()
            && let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref()
            && let Some(selection) = &select.selection
        {
            return Ok(selection.clone());
        }

        Err(TriggerError::SqlParsingError(
            "Could not extract WHERE clause expression from SQL statement.".to_owned(),
        ))
    }

    /// Evaluates trigger conditions against process data using SQL-like logic.
    pub fn evaluate_sql_triggers(
        &mut self,
        collector_id: &str,
        process_data: &ProcessTriggerData,
    ) -> Result<Vec<TriggerRequest>, TriggerError> {
        let start_time = SystemTime::now();
        self.evaluation_stats.total_evaluations += 1;

        // Get conditions reference to avoid expensive clone
        let Some(conditions) = self.compiled_conditions.get(collector_id) else {
            return Ok(Vec::new()); // No conditions registered for this collector
        };

        let mut triggers = Vec::new();
        let mut match_results = Vec::new();

        for (index, compiled_condition) in conditions.iter().enumerate() {
            let condition_start = SystemTime::now();

            let matches = match Self::evaluate_compiled_condition(compiled_condition, process_data)
            {
                Ok(matches) => matches,
                Err(e) => {
                    self.evaluation_stats.evaluation_errors += 1;
                    warn!(
                        condition_id = %compiled_condition.condition.id,
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to evaluate trigger condition"
                    );
                    match_results.push((index, false, condition_start));
                    continue;
                }
            };

            match_results.push((index, matches, condition_start));

            if matches {
                self.evaluation_stats.successful_matches += 1;

                // Create trigger request
                let trigger =
                    Self::create_sql_trigger_request(&compiled_condition.condition, process_data)?;
                triggers.push(trigger);
            }
        }

        // Update condition statistics after the loop to avoid borrow conflicts
        for (index, matches, _condition_start) in match_results {
            if let Some(conditions_mut) = self.compiled_conditions.get_mut(collector_id)
                && let Some(compiled_condition_mut) = conditions_mut.get_mut(index)
            {
                compiled_condition_mut.stats.evaluation_count += 1;
                compiled_condition_mut.stats.last_evaluation = Some(SystemTime::now());

                if matches {
                    compiled_condition_mut.stats.match_count += 1;
                    compiled_condition_mut.stats.last_match = Some(SystemTime::now());
                } else {
                    compiled_condition_mut.stats.no_match_count += 1;
                    compiled_condition_mut.stats.last_no_match = Some(SystemTime::now());
                }
            }
        }

        // Update overall evaluation time
        if let Ok(elapsed) = start_time.elapsed() {
            let elapsed_us = elapsed.as_micros() as u64;
            let total_evals = self.evaluation_stats.total_evaluations;
            self.evaluation_stats.avg_evaluation_time_us =
                (self.evaluation_stats.avg_evaluation_time_us * (total_evals - 1) + elapsed_us)
                    / total_evals;
        }

        Ok(triggers)
    }

    /// Evaluates a compiled trigger condition against process data.
    fn evaluate_compiled_condition(
        compiled_condition: &CompiledTriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<bool, TriggerError> {
        match &compiled_condition.condition.condition_type {
            ConditionType::ProcessNamePattern(pattern) => Ok(data.name.contains(pattern)),
            ConditionType::ExecutablePathPattern(pattern) => Ok(data
                .executable_path
                .as_ref()
                .is_some_and(|path| path.contains(pattern))),
            ConditionType::MissingExecutable => Ok(!data.file_exists),
            ConditionType::HashMismatch => {
                // This would integrate with actual hash verification logic
                Ok(false)
            }
            ConditionType::SuspiciousParentChild => {
                // This would integrate with parent-child relationship analysis
                Ok(false)
            }
            ConditionType::ResourceAnomaly {
                cpu_threshold,
                memory_threshold,
            } => {
                let cpu_anomaly = data.cpu_usage.is_some_and(|cpu| cpu > *cpu_threshold);
                let memory_anomaly = data.memory_usage.is_some_and(|mem| mem > *memory_threshold);
                Ok(cpu_anomaly || memory_anomaly)
            }
            ConditionType::Custom(_sql_expr) => {
                // TODO: Implement SQL evaluation for custom conditions.
                // For now, custom conditions are not supported and always evaluate to false.
                // See issue #000 for tracking.
                warn!("Custom SQL conditions are not yet supported and always evaluate to false.");
                Ok(false)
            }
        }
    }

    /// Creates a trigger request from a matched condition and process data.
    fn create_sql_trigger_request(
        condition: &TriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<TriggerRequest, TriggerError> {
        let trigger_id = Uuid::new_v4().to_string();
        let correlation_id = Uuid::new_v4().to_string();
        let timestamp = SystemTime::now();

        let mut metadata = HashMap::new();
        metadata.insert("condition_id".to_owned(), condition.id.clone());
        metadata.insert("source_pid".to_owned(), data.pid.to_string());
        metadata.insert("evaluation_method".to_owned(), "sql_trigger".to_owned());

        if let Some(ref path) = data.executable_path {
            metadata.insert("executable_path".to_owned(), path.clone());
        }

        Ok(TriggerRequest {
            trigger_id,
            target_collector: condition.target_collector.clone(),
            analysis_type: condition.analysis_type.clone(),
            priority: condition.priority.clone(),
            target_pid: Some(data.pid),
            target_path: data.executable_path.clone(),
            correlation_id,
            metadata,
            timestamp,
        })
    }

    /// Returns SQL evaluation statistics.
    pub fn get_evaluation_stats(&self) -> SqlEvaluationStats {
        self.evaluation_stats.clone()
    }

    /// Returns condition-specific statistics for a collector.
    pub fn get_condition_stats(
        &self,
        collector_id: &str,
    ) -> Vec<(String, ConditionEvaluationStats)> {
        self.compiled_conditions
            .get(collector_id)
            .map(|conditions| {
                conditions
                    .iter()
                    .map(|c| (c.condition.id.clone(), c.stats.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for SqlTriggerEvaluator {
    fn default() -> Self {
        Self::new()
    }
}
