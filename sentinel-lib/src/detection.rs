//! SQL-based detection engine with security validation.
//!
//! This module provides the core detection engine that executes SQL-based rules
//! against process data with comprehensive security validation to prevent SQL injection.

use crate::models::{Alert, DetectionRule, ProcessRecord};
// Removed unused imports
use std::collections::HashMap;
use thiserror::Error;

/// Detection engine errors.
#[derive(Debug, Error)]
pub enum DetectionEngineError {
    #[error("SQL validation failed: {0}")]
    SqlValidationError(String),

    #[error("Rule execution failed: {0}")]
    ExecutionError(String),

    #[error("Timeout during rule execution")]
    Timeout,

    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),
}

/// Detection engine for executing SQL-based rules.
pub struct DetectionEngine {
    rules: HashMap<String, DetectionRule>,
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
            max_execution_time_ms: 30000, // 30 seconds
            max_memory_mb: 100,           // 100 MB
        }
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
    /// ```rust,ignore
    /// // Create an engine and load a validated rule (see DetectionRule::new docs)
    /// // let mut engine = sentinel_lib::detection::DetectionEngine::new();
    /// // engine.load_rule(rule).unwrap();
    /// ```
    pub fn load_rule(&mut self, rule: DetectionRule) -> Result<(), DetectionEngineError> {
        // Validate the rule before loading
        rule.validate_sql()
            .map_err(|e| DetectionEngineError::SqlValidationError(e.to_string()))?;

        self.rules.insert(rule.id.raw().to_owned(), rule);
        Ok(())
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
    /// ```ignore
    /// // Illustrative async example; construct a rule and processes then call execute_rule().
    /// ```
    fn execute_rule(rule: &DetectionRule, processes: &[ProcessRecord]) -> Vec<Alert> {
        // In a real implementation, this would:
        // 1. Parse the SQL query using sqlparser
        // 2. Validate it against a whitelist of allowed operations
        // 3. Execute it against the process data
        // 4. Generate alerts based on results

        // For now, we'll create a simple placeholder implementation
        // Pre-allocate with an upper bound (processes.len()); some categories may alert often.
        let mut alerts = Vec::with_capacity(processes.len());
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
                    if let Some(cpu_usage) = process.cpu_usage {
                        if cpu_usage > 80.0 {
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
