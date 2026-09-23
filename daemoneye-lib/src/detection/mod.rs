//! SQL-based detection engine with security validation.
//!
//! This module provides the core detection engine that executes SQL-based rules
//! against process data with comprehensive security validation to prevent SQL injection.

pub mod allowlist;
pub mod catalog;
pub mod conformance;
pub mod planner;
pub mod regex_cache;
pub mod rejection;
pub mod rule_health;
pub mod sql_validation;
pub mod task_renewal;

use crate::config::DetectionConfig;
use crate::detection::catalog::{CatalogChange, CatalogError, SchemaCatalog, VerifiedRegistration};
use crate::detection::rule_health::{RuleHealth, RuleHealthRegistry};
use crate::models::{Alert, DetectionRule, ProcessRecord, RuleError};
use crate::proto::SchemaDescriptor;
use crate::rejection_log::{RejectionLog, RejectionReason};
use std::collections::HashMap;
use thiserror::Error;

pub use allowlist::{ALLOWED_SQL_FUNCTIONS, is_allowed_sql_function};
pub use conformance::{
    ConformanceAxis, ConformanceCase, ConformanceOutcome, cases_for, corpus, reference_outcome,
    verify_operation,
};
pub use planner::{CompiledRule, PlanError, plan_rule};
pub use regex_cache::{CompiledPattern, RegexCache, RegexCacheStats, compile_rule_patterns};
pub use rejection::{RegexConstruct, RegexRejection, SqlPosition, SqlRejection};
pub use sql_validation::validate_detection_sql;
pub use task_renewal::{PendingRenewal, RenewalCycle, TaskRenewalLedger};

/// Detection engine errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DetectionEngineError {
    #[error("SQL validation failed: {0}")]
    SqlValidationError(String),

    #[error("Rule execution failed: {0}")]
    ExecutionError(String),

    #[error("Timeout during rule execution")]
    Timeout,

    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    #[error("Rule could not be lowered into a pushdown plan: {0}")]
    PlanRejected(String),
}

/// Detection engine for executing SQL-based rules.
#[derive(Debug)]
pub struct DetectionEngine {
    rules: HashMap<String, DetectionRule>,
    /// Compiled plans, one per rule that has been lowered. This is the enabled set: a rule with no
    /// entry here has no task to issue.
    compiled: HashMap<String, CompiledRule>,
    /// Rules loaded before any collector registered, waiting for the first one (R18).
    deferred: Vec<String>,
    catalog: SchemaCatalog,
    health: RuleHealthRegistry,
    patterns: RegexCache,
    /// Live pushdown tasks and when each was last confirmed on its collector (R16).
    tasks: TaskRenewalLedger,
    max_subquery_depth: u32,
    rejections: RejectionLog,
    #[allow(dead_code)]
    max_execution_time_ms: u64,
    #[allow(dead_code)]
    max_memory_mb: u64,
}

impl DetectionEngine {
    /// Create a new detection engine.
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            compiled: HashMap::new(),
            deferred: Vec::new(),
            catalog: SchemaCatalog::new(),
            health: RuleHealthRegistry::new(),
            patterns: RegexCache::new(),
            tasks: TaskRenewalLedger::new(),
            max_subquery_depth: DetectionConfig::default().max_subquery_depth,
            rejections: RejectionLog::new(),
            max_execution_time_ms: 30000, // 30 seconds
            max_memory_mb: 100,           // 100 MB
        }
    }

    /// The catalog rules are planned against.
    #[must_use]
    pub const fn catalog(&self) -> &SchemaCatalog {
        &self.catalog
    }

    /// The compiled plan for a rule, if the rule is in the enabled set.
    #[must_use]
    pub fn compiled_rule(&self, id: &str) -> Option<&CompiledRule> {
        self.compiled.get(id)
    }

    /// Rules waiting for the first collector registration (R18), in load order.
    #[must_use]
    pub fn deferred_rule_ids(&self) -> Vec<String> {
        self.deferred.clone()
    }

    /// Current health of a rule, once it has been judged against a catalog (R12).
    #[must_use]
    pub fn rule_health(&self, id: &str) -> Option<&RuleHealth> {
        self.health.health(id)
    }

    /// Register a collector's schema descriptor, then plan everything it unblocked (R10, R12, R18).
    ///
    /// A rule deferred under R18 has never been planned, so the drain here *is* its first load:
    /// a lowering failure rejects it, exactly as a non-deferred first load would. Marking a rule
    /// unhealthy is reserved for re-planning an already-loaded rule.
    ///
    /// # Errors
    ///
    /// Returns the [`CatalogError`] that refused the descriptor. Rules are untouched in that case.
    pub fn register_collector(
        &mut self,
        verified: &VerifiedRegistration,
        descriptor: SchemaDescriptor,
    ) -> Result<CatalogChange, CatalogError> {
        let change = self.catalog.register(verified, descriptor)?;

        for rule_id in std::mem::take(&mut self.deferred) {
            if let Err(error) = self.plan_and_record(&rule_id) {
                self.reject_rule(&rule_id, &error.to_string());
            }
        }

        let outcome = self.health.revalidate(&self.catalog, &change);
        for rule_id in outcome.to_replan().to_vec() {
            if let Err(error) = self.plan_and_record(&rule_id) {
                // The rule's references still resolve — `revalidate` just checked — so this is a
                // shape the planner can no longer lower. Drop it from the enabled set so no task
                // is re-issued, and record why.
                let _dropped = self.compiled.remove(&rule_id);
                self.rejections
                    .record(RejectionReason::rule_other(&rule_id, &error.to_string()));
            }
        }

        Ok(change)
    }

    /// Lower one loaded rule and record its plan and its references.
    fn plan_and_record(&mut self, rule_id: &str) -> Result<(), PlanError> {
        let Some(rule) = self.rules.get(rule_id) else {
            return Ok(());
        };
        let compiled = plan_rule(&self.catalog, &self.patterns, rule, self.max_subquery_depth)?;
        self.health
            .track(rule_id, compiled.references().iter().cloned());
        let _previous = self.compiled.insert(rule_id.to_owned(), compiled);
        Ok(())
    }

    /// Record a first-load rejection and take the rule back out of the engine (R17).
    fn reject_rule(&mut self, rule_id: &str, message: &str) {
        self.rejections
            .record(RejectionReason::rule_other(rule_id, message));
        let _removed_rule = self.rules.remove(rule_id);
        let _removed_plan = self.compiled.remove(rule_id);
        self.health.forget(rule_id);
    }

    /// Loads a detection rule into the engine.
    ///
    /// Validates the rule's SQL using `rule.validate_sql()` and, on success,
    /// inserts the rule into the engine's rule map keyed by `rule.id.raw().to_string()`.
    /// If a rule with the same ID already exists it will be overwritten.
    ///
    /// On validation failure this returns `DetectionEngineError::SqlValidationError`.
    ///
    /// # Examples
    ///
    /// ```text
    /// // Create an engine and load a validated rule (see DetectionRule::new docs)
    /// // let mut engine = daemoneye_lib::detection::DetectionEngine::new();
    /// // engine.load_rule(rule).unwrap();
    /// ```
    pub fn load_rule(&mut self, rule: DetectionRule) -> Result<(), DetectionEngineError> {
        // Validate the rule before loading. A rejection is recorded with its structured cause
        // intact (R4) before it is flattened into the engine's error type.
        if let Err(error) = rule.validate_sql() {
            let rule_id = rule.id.raw();
            let message = error.to_string();
            let reason = match error {
                RuleError::SqlRejected(rejection) => RejectionReason::rule_sql(rule_id, rejection),
                RuleError::RegexRejected(rejection) => {
                    RejectionReason::rule_regex(rule_id, rejection)
                }
                RuleError::MissingField(_)
                | RuleError::ValidationFailed(_)
                | RuleError::RuleNotFound(_)
                | RuleError::ExecutionFailed(_) => RejectionReason::rule_other(rule_id, &message),
            };
            self.rejections.record(reason);
            return Err(DetectionEngineError::SqlValidationError(message));
        }

        let rule_id = rule.id.raw().to_owned();
        let _previous = self.rules.insert(rule_id.clone(), rule);

        // R18: no rule is judged against an empty catalog. It waits, it is not dropped.
        if self.catalog.is_empty() {
            self.deferred.push(rule_id);
            return Ok(());
        }

        if let Err(error) = self.plan_and_record(&rule_id) {
            let message = error.to_string();
            self.reject_rule(&rule_id, &message);
            return Err(DetectionEngineError::PlanRejected(message));
        }
        Ok(())
    }

    /// The rejections this engine has recorded, oldest first.
    ///
    /// The chain is in-memory and agent-side: the `audit_ledger` table is procmond's to write.
    pub const fn rejection_log(&self) -> &RejectionLog {
        &self.rejections
    }

    /// Execute all enabled rules against process data.
    pub fn execute_rules(&self, processes: &[ProcessRecord]) -> Vec<Alert> {
        let mut alerts = Vec::new();

        for rule in self.rules.values() {
            if !rule.enabled {
                continue;
            }

            let mut rule_alerts = Self::execute_rule(rule, processes);
            alerts.append(&mut rule_alerts);
        }

        alerts
    }

    /// Execute a single detection rule against a slice of process records.
    ///
    /// This is a placeholder implementation that interprets the rule by its
    /// metadata.category and generates Alerts for matching processes:
    /// - "`suspicious_process"`: produces an alert for any process whose name
    ///   contains the substring "suspicious".
    /// - "`high_cpu"`: produces an alert for any process with `cpu_usage` > 80.0.
    /// - other/unknown categories produce no alerts.
    ///
    /// Returns a vector of generated Alert objects or a `DetectionEngineError` on failure.
    /// (Current implementation does not return errors; the Result wrapper is preserved
    /// for future, real SQL-based execution.)
    ///
    /// # Examples
    ///
    /// ```text
    /// // Illustrative async example; construct a rule and processes then call execute_rule().
    /// ```
    fn execute_rule(rule: &DetectionRule, processes: &[ProcessRecord]) -> Vec<Alert> {
        // In a real implementation, this would:
        // 1. Parse the SQL query using sqlparser
        // 2. Validate it against a whitelist of allowed operations
        // 3. Execute it against the process data
        // 4. Generate alerts based on results

        // For now, we'll create a simple placeholder implementation
        // Pre-allocate with conservative capacity since most processes won't generate alerts
        let mut alerts = Vec::with_capacity(4);
        let rule_id = rule.id.raw().to_owned();

        // Simple pattern matching based on rule category
        match rule.metadata.category.as_deref().unwrap_or("unknown") {
            "suspicious_process" => {
                for process in processes {
                    if process.name.contains("suspicious") {
                        let alert = Alert::new(
                            rule.severity,
                            format!("Suspicious process detected: {}", process.name),
                            format!("Process {} matches suspicious pattern", process.name),
                            rule_id.clone(),
                            process.clone(),
                        );

                        alerts.push(alert);
                    }
                }
            }
            "high_cpu" => {
                for process in processes {
                    if let Some(cpu_usage) = process.cpu_usage
                        && cpu_usage > 80.0
                    {
                        let alert = Alert::new(
                            rule.severity,
                            format!("High CPU usage detected: {cpu_usage}%"),
                            format!("Process {} is using {}% CPU", process.name, cpu_usage),
                            rule_id.clone(),
                            process.clone(),
                        );

                        alerts.push(alert);
                    }
                }
            }
            _ => {
                // Default behavior for unknown categories
            }
        }

        alerts
    }

    /// Get all loaded rules.
    pub fn get_rules(&self) -> Vec<&DetectionRule> {
        self.rules.values().collect()
    }

    /// Get a specific rule by ID.
    pub fn get_rule(&self, id: &str) -> Option<&DetectionRule> {
        self.rules.get(id)
    }

    /// Remove a rule by ID.
    pub fn remove_rule(&mut self, id: &str) -> Option<DetectionRule> {
        self.rules.remove(id)
    }

    /// Enable or disable a rule.
    pub fn set_rule_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<(), DetectionEngineError> {
        if let Some(rule) = self.rules.get_mut(id) {
            rule.enabled = enabled;
            Ok(())
        } else {
            Err(DetectionEngineError::ExecutionError(format!(
                "Rule not found: {id}"
            )))
        }
    }
}

impl Default for DetectionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::models::AlertSeverity;

    #[tokio::test]
    async fn test_detection_engine_creation() {
        let engine = DetectionEngine::new();
        assert_eq!(engine.get_rules().len(), 0);
    }

    #[tokio::test]
    async fn test_rule_loading() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        assert!(engine.load_rule(rule).is_ok());
        assert_eq!(engine.get_rules().len(), 1);
    }

    #[tokio::test]
    async fn test_rule_loading_duplicate_id() {
        let mut engine = DetectionEngine::new();
        let rule1 = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule 1".to_owned(),
            "Test detection rule 1".to_owned(),
            "SELECT * FROM processes WHERE name = 'test1'".to_owned(),
            "test1".to_owned(),
            AlertSeverity::Medium,
        );
        let rule2 = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule 2".to_owned(),
            "Test detection rule 2".to_owned(),
            "SELECT * FROM processes WHERE name = 'test2'".to_owned(),
            "test2".to_owned(),
            AlertSeverity::High,
        );

        assert!(engine.load_rule(rule1).is_ok());
        assert!(engine.load_rule(rule2).is_ok()); // This will overwrite the first rule
        assert_eq!(engine.get_rules().len(), 1);

        // Verify the second rule overwrote the first
        let loaded_rule = engine.get_rule("rule-1").expect("Failed to get rule");
        assert_eq!(loaded_rule.name, "Test Rule 2");
        assert_eq!(loaded_rule.severity, AlertSeverity::High);
    }

    #[tokio::test]
    async fn test_rule_loading_invalid_sql() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "INVALID SQL QUERY".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        assert!(engine.load_rule(rule).is_err());
        assert_eq!(engine.get_rules().len(), 0);
    }

    #[tokio::test]
    async fn test_rule_removal() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        engine.load_rule(rule).expect("Failed to load rule");
        assert_eq!(engine.get_rules().len(), 1);

        assert!(engine.remove_rule("rule-1").is_some());
        assert_eq!(engine.get_rules().len(), 0);
    }

    #[tokio::test]
    async fn test_rule_removal_nonexistent() {
        let mut engine = DetectionEngine::new();
        assert!(engine.remove_rule("nonexistent-rule").is_none());
    }

    #[tokio::test]
    async fn test_rule_enable_disable() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        engine.load_rule(rule).expect("Failed to load rule");
        assert!(engine.get_rules()[0].enabled);

        engine
            .set_rule_enabled("rule-1", false)
            .expect("Failed to disable rule");
        assert!(!engine.get_rules()[0].enabled);

        engine
            .set_rule_enabled("rule-1", true)
            .expect("Failed to enable rule");
        assert!(engine.get_rules()[0].enabled);
    }

    #[tokio::test]
    async fn test_rule_enable_disable_nonexistent() {
        let mut engine = DetectionEngine::new();
        assert!(engine.set_rule_enabled("nonexistent-rule", false).is_err());
    }

    #[tokio::test]
    async fn test_rule_execution() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Suspicious Process Rule".to_owned(),
            "Detects suspicious processes".to_owned(),
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'".to_owned(),
            "suspicious_process".to_owned(),
            AlertSeverity::High,
        );

        engine.load_rule(rule).expect("Failed to load rule");

        let mut process = ProcessRecord::new(1234, "suspicious-process".to_owned());
        process.name = "suspicious-process".to_owned();
        let processes = vec![process];

        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].title,
            "Suspicious process detected: suspicious-process"
        );
    }

    #[tokio::test]
    async fn test_rule_execution_no_matches() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Suspicious Process Rule".to_owned(),
            "Detects suspicious processes".to_owned(),
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'".to_owned(),
            "suspicious_process".to_owned(),
            AlertSeverity::High,
        );

        engine.load_rule(rule).expect("Failed to load rule");

        let process = ProcessRecord::new(1234, "normal-process".to_owned());
        let processes = vec![process];

        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 0);
    }

    #[tokio::test]
    async fn test_rule_execution_disabled_rule() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Suspicious Process Rule".to_owned(),
            "Detects suspicious processes".to_owned(),
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'".to_owned(),
            "suspicious_process".to_owned(),
            AlertSeverity::High,
        );

        engine.load_rule(rule).expect("Failed to load rule");
        engine
            .set_rule_enabled("rule-1", false)
            .expect("Failed to disable rule");

        let mut process = ProcessRecord::new(1234, "suspicious-process".to_owned());
        process.name = "suspicious-process".to_owned();
        let processes = vec![process];

        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 0);
    }

    #[tokio::test]
    async fn test_rule_execution_multiple_rules() {
        let mut engine = DetectionEngine::new();

        let rule1 = DetectionRule::new(
            "rule-1".to_owned(),
            "Suspicious Process Rule".to_owned(),
            "Detects suspicious processes".to_owned(),
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'".to_owned(),
            "suspicious_process".to_owned(),
            AlertSeverity::High,
        );

        let rule2 = DetectionRule::new(
            "rule-2".to_owned(),
            "High CPU Rule".to_owned(),
            "Detects high CPU processes".to_owned(),
            "SELECT * FROM processes WHERE cpu_usage > 80".to_owned(),
            "high_cpu".to_owned(),
            AlertSeverity::Medium,
        );

        engine.load_rule(rule1).expect("Failed to load rule1");
        engine.load_rule(rule2).expect("Failed to load rule2");

        let mut process1 = ProcessRecord::new(1234, "suspicious-process".to_owned());
        process1.name = "suspicious-process".to_owned();
        process1.cpu_usage = Some(90.0);

        let mut process2 = ProcessRecord::new(5678, "normal-process".to_owned());
        process2.name = "normal-process".to_owned();
        process2.cpu_usage = Some(50.0);

        let processes = vec![process1, process2];

        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 2);
    }

    #[tokio::test]
    async fn test_rule_execution_empty_processes() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        engine.load_rule(rule).expect("Failed to load rule");

        let processes = vec![];
        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 0);
    }

    #[tokio::test]
    async fn test_rule_execution_no_rules() {
        let engine = DetectionEngine::new();
        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let processes = vec![process];

        let alerts = engine.execute_rules(&processes);
        assert_eq!(alerts.len(), 0);
    }

    #[test]
    fn test_detection_engine_error_display() {
        let errors = vec![
            DetectionEngineError::ExecutionError("test error".to_owned()),
            DetectionEngineError::SqlValidationError("test error".to_owned()),
            DetectionEngineError::Timeout,
            DetectionEngineError::ResourceLimitExceeded("test error".to_owned()),
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(!error_string.is_empty());
        }
    }
}
