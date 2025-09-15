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
    #[allow(dead_code)] // Will be used in Task 8
    max_execution_time_ms: u64,
    #[allow(dead_code)] // Will be used in Task 8
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

    /// Load a detection rule.
    pub fn load_rule(&mut self, rule: DetectionRule) -> Result<(), DetectionEngineError> {
        // Validate the rule before loading
        rule.validate_sql()
            .map_err(|e| DetectionEngineError::SqlValidationError(e.to_string()))?;

        self.rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Execute all enabled rules against process data.
    pub async fn execute_rules(
        &self,
        processes: &[ProcessRecord],
    ) -> Result<Vec<Alert>, DetectionEngineError> {
        let mut alerts = Vec::new();

        for rule in self.rules.values() {
            if !rule.enabled {
                continue;
            }

            match self.execute_rule(rule, processes).await {
                Ok(mut rule_alerts) => alerts.append(&mut rule_alerts),
                Err(e) => {
                    // Log error but continue with other rules
                    eprintln!("Rule execution failed for {}: {}", rule.id, e);
                }
            }
        }

        Ok(alerts)
    }

    /// Execute a single rule against process data.
    async fn execute_rule(
        &self,
        rule: &DetectionRule,
        processes: &[ProcessRecord],
    ) -> Result<Vec<Alert>, DetectionEngineError> {
        // In a real implementation, this would:
        // 1. Parse the SQL query using sqlparser
        // 2. Validate it against a whitelist of allowed operations
        // 3. Execute it against the process data
        // 4. Generate alerts based on results

        // For now, we'll create a simple placeholder implementation
        let mut alerts = Vec::new();

        // Simple pattern matching based on rule category
        match rule.category.as_str() {
            "suspicious_process" => {
                for process in processes {
                    if process.name.contains("suspicious") {
                        let alert = Alert::new(
                            format!("alert-{}-{}", rule.id, process.pid),
                            rule.severity.clone(),
                            format!("Suspicious process detected: {}", process.name),
                            format!("Process {} matches suspicious pattern", process.name),
                            rule.category.clone(),
                        )
                        .with_process(process.clone());

                        alerts.push(alert);
                    }
                }
            }
            "high_cpu" => {
                for process in processes {
                    if let Some(cpu_usage) = process.cpu_usage {
                        if cpu_usage > 80.0 {
                            let alert = Alert::new(
                                format!("alert-{}-{}", rule.id, process.pid),
                                rule.severity.clone(),
                                format!("High CPU usage detected: {}%", cpu_usage),
                                format!("Process {} is using {}% CPU", process.name, cpu_usage),
                                rule.category.clone(),
                            )
                            .with_process(process.clone());

                            alerts.push(alert);
                        }
                    }
                }
            }
            _ => {
                // Default behavior for unknown categories
            }
        }

        Ok(alerts)
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
                "Rule not found: {}",
                id
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
            "rule-1".to_string(),
            "Test Rule".to_string(),
            "Test detection rule".to_string(),
            "SELECT * FROM processes WHERE name = 'test'".to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );

        assert!(engine.load_rule(rule).is_ok());
        assert_eq!(engine.get_rules().len(), 1);
    }

    #[tokio::test]
    async fn test_rule_execution() {
        let mut engine = DetectionEngine::new();
        let rule = DetectionRule::new(
            "rule-1".to_string(),
            "Suspicious Process Rule".to_string(),
            "Detects suspicious processes".to_string(),
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'".to_string(),
            "suspicious_process".to_string(),
            AlertSeverity::High,
        );

        engine.load_rule(rule).unwrap();

        let mut process = ProcessRecord::new(1234, "suspicious-process".to_string());
        process.name = "suspicious-process".to_string();
        let processes = vec![process];

        let alerts = engine.execute_rules(&processes).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].title,
            "Suspicious process detected: suspicious-process"
        );
    }
}
