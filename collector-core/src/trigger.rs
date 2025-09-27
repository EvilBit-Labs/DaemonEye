//! Trigger system for analysis collector coordination.
//!
//! This module provides the infrastructure for coordinating between process
//! monitoring collectors and analysis collectors (Binary Hasher, Memory Analyzer, etc.).
//! It implements trigger condition evaluation, deduplication, rate limiting, and
//! metadata tracking for forensic analysis.

use crate::event::{AnalysisType, TriggerPriority, TriggerRequest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Configuration for the trigger system.
///
/// Controls trigger generation behavior, rate limiting, and deduplication
/// to prevent analysis collector overload while ensuring critical threats
/// are analyzed promptly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Maximum number of triggers per collector per time window
    pub max_triggers_per_collector: usize,

    /// Time window for rate limiting (in seconds)
    pub rate_limit_window_secs: u64,

    /// Deduplication window for identical triggers (in seconds)
    pub deduplication_window_secs: u64,

    /// Maximum number of pending triggers to track
    pub max_pending_triggers: usize,

    /// Enable trigger metadata collection for debugging
    pub enable_metadata_tracking: bool,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            max_triggers_per_collector: 100,
            rate_limit_window_secs: 60,
            deduplication_window_secs: 300, // 5 minutes
            max_pending_triggers: 1000,
            enable_metadata_tracking: true,
        }
    }
}

/// Trigger condition for evaluating when to request analysis.
///
/// Defines the criteria that must be met to trigger analysis collector
/// coordination. Conditions can be combined using logical operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCondition {
    /// Condition identifier
    pub id: String,

    /// Human-readable description
    pub description: String,

    /// Analysis type to trigger
    pub analysis_type: AnalysisType,

    /// Priority level for triggered analysis
    pub priority: TriggerPriority,

    /// Target collector name
    pub target_collector: String,

    /// Condition evaluation function (simplified for this implementation)
    pub condition_type: ConditionType,
}

/// Types of trigger conditions that can be evaluated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionType {
    /// Process name matches pattern
    ProcessNamePattern(String),

    /// Executable path matches pattern
    ExecutablePathPattern(String),

    /// Process has no executable file
    MissingExecutable,

    /// Process hash mismatch detected
    HashMismatch,

    /// Suspicious parent-child relationship
    SuspiciousParentChild,

    /// High resource usage anomaly
    ResourceAnomaly {
        cpu_threshold: f64,
        memory_threshold: u64,
    },

    /// Custom condition (extensible)
    Custom(String),
}

/// Deduplication key for trigger requests.
///
/// Used to identify identical or similar trigger requests to prevent
/// redundant analysis of the same target within the deduplication window.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeduplicationKey {
    /// Target collector name
    pub collector: String,

    /// Analysis type
    pub analysis_type: AnalysisType,

    /// Target process ID (if applicable)
    pub target_pid: Option<u32>,

    /// Target file path (if applicable)
    pub target_path: Option<String>,
}

/// Trigger metadata for correlation and debugging.
///
/// Tracks trigger generation context, evaluation results, and timing
/// information for forensic analysis and system debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerMetadata {
    /// Trigger generation timestamp
    pub generated_at: SystemTime,

    /// Source event that triggered the analysis
    pub source_event_type: String,

    /// Condition that matched
    pub matched_condition: String,

    /// Evaluation context
    pub evaluation_context: HashMap<String, String>,

    /// Correlation ID for tracking related events
    pub correlation_id: String,
}

/// Rate limiting state for a specific collector.
#[derive(Debug, Clone)]
struct RateLimitState {
    /// Number of triggers sent in current window
    trigger_count: usize,

    /// Window start time
    window_start: SystemTime,
}

/// Trigger system manager for analysis collector coordination.
///
/// Manages trigger condition evaluation, deduplication, rate limiting,
/// and metadata tracking. Provides thread-safe coordination between
/// process monitoring and analysis collectors.
pub struct TriggerManager {
    /// Configuration
    config: TriggerConfig,

    /// Registered trigger conditions
    conditions: Vec<TriggerCondition>,

    /// Rate limiting state per collector
    rate_limits: Arc<Mutex<HashMap<String, RateLimitState>>>,

    /// Deduplication tracking
    deduplication_cache: Arc<Mutex<HashMap<DeduplicationKey, SystemTime>>>,

    /// Trigger metadata tracking
    metadata_cache: Arc<Mutex<HashMap<String, TriggerMetadata>>>,

    /// Pending trigger count
    pending_count: Arc<Mutex<usize>>,
}

impl TriggerManager {
    /// Creates a new trigger manager with the specified configuration.
    pub fn new(config: TriggerConfig) -> Self {
        Self {
            config,
            conditions: Vec::new(),
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            deduplication_cache: Arc::new(Mutex::new(HashMap::new())),
            metadata_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Registers a trigger condition for evaluation.
    pub fn register_condition(&mut self, condition: TriggerCondition) {
        info!(
            condition_id = %condition.id,
            analysis_type = ?condition.analysis_type,
            target_collector = %condition.target_collector,
            "Registering trigger condition"
        );
        self.conditions.push(condition);
    }

    /// Evaluates trigger conditions against process event data.
    ///
    /// Returns trigger requests that should be sent to analysis collectors
    /// after applying deduplication and rate limiting.
    pub fn evaluate_triggers(
        &self,
        process_data: &ProcessTriggerData,
    ) -> Result<Vec<TriggerRequest>, TriggerError> {
        let mut triggers = Vec::new();

        // Evaluate each registered condition
        for condition in &self.conditions {
            if self.evaluate_condition(condition, process_data)? {
                debug!(
                    condition_id = %condition.id,
                    pid = process_data.pid,
                    "Trigger condition matched"
                );

                // Create trigger request
                let trigger = self.create_trigger_request(condition, process_data)?;

                // Apply deduplication
                if self.should_deduplicate(&trigger)? {
                    debug!(
                        trigger_id = %trigger.trigger_id,
                        "Trigger deduplicated - skipping"
                    );
                    continue;
                }

                // Apply rate limiting
                if self.should_rate_limit(&trigger)? {
                    warn!(
                        trigger_id = %trigger.trigger_id,
                        target_collector = %trigger.target_collector,
                        "Trigger rate limited - skipping"
                    );
                    continue;
                }

                triggers.push(trigger);
            }
        }

        // Update pending count
        if let Ok(mut count) = self.pending_count.lock() {
            *count = count.saturating_add(triggers.len());
        }

        Ok(triggers)
    }

    /// Evaluates a single trigger condition against process data.
    fn evaluate_condition(
        &self,
        condition: &TriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<bool, TriggerError> {
        match &condition.condition_type {
            ConditionType::ProcessNamePattern(pattern) => Ok(data.name.contains(pattern)),
            ConditionType::ExecutablePathPattern(pattern) => Ok(data
                .executable_path
                .as_ref()
                .map(|path| path.contains(pattern))
                .unwrap_or(false)),
            ConditionType::MissingExecutable => Ok(!data.file_exists),
            ConditionType::HashMismatch => {
                // This would be implemented with actual hash verification logic
                Ok(false)
            }
            ConditionType::SuspiciousParentChild => {
                // This would be implemented with parent-child relationship analysis
                Ok(false)
            }
            ConditionType::ResourceAnomaly {
                cpu_threshold,
                memory_threshold,
            } => {
                let cpu_anomaly = data
                    .cpu_usage
                    .map(|cpu| cpu > *cpu_threshold)
                    .unwrap_or(false);
                let memory_anomaly = data
                    .memory_usage
                    .map(|mem| mem > *memory_threshold)
                    .unwrap_or(false);
                Ok(cpu_anomaly || memory_anomaly)
            }
            ConditionType::Custom(_) => {
                // Custom conditions would be implemented by extending this enum
                Ok(false)
            }
        }
    }

    /// Creates a trigger request from a matched condition and process data.
    fn create_trigger_request(
        &self,
        condition: &TriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<TriggerRequest, TriggerError> {
        let trigger_id = Uuid::new_v4().to_string();
        let correlation_id = Uuid::new_v4().to_string();
        let timestamp = SystemTime::now();

        let mut metadata = HashMap::new();
        metadata.insert("condition_id".to_string(), condition.id.clone());
        metadata.insert("source_pid".to_string(), data.pid.to_string());
        if let Some(ref path) = data.executable_path {
            metadata.insert("executable_path".to_string(), path.clone());
        }

        // Track trigger metadata if enabled
        if self.config.enable_metadata_tracking {
            let trigger_metadata = TriggerMetadata {
                generated_at: timestamp,
                source_event_type: "process".to_string(),
                matched_condition: condition.id.clone(),
                evaluation_context: metadata.clone(),
                correlation_id: correlation_id.clone(),
            };

            if let Ok(mut cache) = self.metadata_cache.lock() {
                cache.insert(trigger_id.clone(), trigger_metadata);
            }
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

    /// Checks if a trigger should be deduplicated.
    fn should_deduplicate(&self, trigger: &TriggerRequest) -> Result<bool, TriggerError> {
        let dedup_key = DeduplicationKey {
            collector: trigger.target_collector.clone(),
            analysis_type: trigger.analysis_type.clone(),
            target_pid: trigger.target_pid,
            target_path: trigger.target_path.clone(),
        };

        let mut cache = self
            .deduplication_cache
            .lock()
            .map_err(|_| TriggerError::LockError("deduplication_cache".to_string()))?;

        let now = SystemTime::now();
        let dedup_window = Duration::from_secs(self.config.deduplication_window_secs);

        // Clean expired entries
        cache.retain(|_, timestamp| {
            now.duration_since(*timestamp).unwrap_or(Duration::MAX) < dedup_window
        });

        // Check if this trigger is a duplicate
        if let Some(last_trigger_time) = cache.get(&dedup_key) {
            if now
                .duration_since(*last_trigger_time)
                .unwrap_or(Duration::MAX)
                < dedup_window
            {
                return Ok(true); // Should deduplicate
            }
        }

        // Record this trigger
        cache.insert(dedup_key, now);
        Ok(false) // Should not deduplicate
    }

    /// Checks if a trigger should be rate limited.
    fn should_rate_limit(&self, trigger: &TriggerRequest) -> Result<bool, TriggerError> {
        let mut rate_limits = self
            .rate_limits
            .lock()
            .map_err(|_| TriggerError::LockError("rate_limits".to_string()))?;

        let now = SystemTime::now();
        let rate_window = Duration::from_secs(self.config.rate_limit_window_secs);

        let state = rate_limits
            .entry(trigger.target_collector.clone())
            .or_insert_with(|| RateLimitState {
                trigger_count: 0,
                window_start: now,
            });

        // Check if we need to reset the window
        if now
            .duration_since(state.window_start)
            .unwrap_or(Duration::MAX)
            >= rate_window
        {
            state.trigger_count = 0;
            state.window_start = now;
        }

        // Check rate limit
        if state.trigger_count >= self.config.max_triggers_per_collector {
            return Ok(true); // Should rate limit
        }

        // Increment counter
        state.trigger_count += 1;
        Ok(false) // Should not rate limit
    }

    /// Returns current trigger statistics for monitoring.
    pub fn get_statistics(&self) -> TriggerStatistics {
        let pending_count = self.pending_count.lock().map(|count| *count).unwrap_or(0);

        let dedup_cache_size = self
            .deduplication_cache
            .lock()
            .map(|cache| cache.len())
            .unwrap_or(0);

        let rate_limit_states = self
            .rate_limits
            .lock()
            .map(|limits| limits.len())
            .unwrap_or(0);

        TriggerStatistics {
            registered_conditions: self.conditions.len(),
            pending_triggers: pending_count,
            deduplication_cache_size: dedup_cache_size,
            rate_limit_states,
        }
    }

    /// Cleans up expired entries and resets counters.
    pub fn cleanup(&self) -> Result<(), TriggerError> {
        // Clean deduplication cache
        if let Ok(mut cache) = self.deduplication_cache.lock() {
            let now = SystemTime::now();
            let dedup_window = Duration::from_secs(self.config.deduplication_window_secs);
            cache.retain(|_, timestamp| {
                now.duration_since(*timestamp).unwrap_or(Duration::MAX) < dedup_window
            });
        }

        // Clean metadata cache
        if let Ok(mut cache) = self.metadata_cache.lock() {
            if cache.len() > self.config.max_pending_triggers {
                // Keep only the most recent entries
                let mut entries: Vec<_> = cache.drain().collect();
                entries.sort_by_key(|(_, metadata)| metadata.generated_at);
                entries.truncate(self.config.max_pending_triggers / 2);
                cache.extend(entries);
            }
        }

        Ok(())
    }
}

/// Process data for trigger evaluation.
///
/// Contains the process information needed to evaluate trigger conditions
/// and generate analysis requests.
#[derive(Debug, Clone)]
pub struct ProcessTriggerData {
    /// Process ID
    pub pid: u32,

    /// Process name
    pub name: String,

    /// Executable path
    pub executable_path: Option<String>,

    /// Whether executable file exists
    pub file_exists: bool,

    /// CPU usage percentage
    pub cpu_usage: Option<f64>,

    /// Memory usage in bytes
    pub memory_usage: Option<u64>,

    /// Process hash (if available)
    pub executable_hash: Option<String>,
}

/// Trigger system statistics for monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerStatistics {
    /// Number of registered trigger conditions
    pub registered_conditions: usize,

    /// Number of pending triggers
    pub pending_triggers: usize,

    /// Size of deduplication cache
    pub deduplication_cache_size: usize,

    /// Number of rate limit states tracked
    pub rate_limit_states: usize,
}

/// Errors that can occur in the trigger system.
#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("Lock error on {0}")]
    LockError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Condition evaluation error: {0}")]
    EvaluationError(String),

    #[error("Trigger generation error: {0}")]
    GenerationError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_process_data() -> ProcessTriggerData {
        ProcessTriggerData {
            pid: 1234,
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            file_exists: true,
            cpu_usage: Some(5.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_string()),
        }
    }

    fn create_test_condition() -> TriggerCondition {
        TriggerCondition {
            id: "test_condition".to_string(),
            description: "Test condition".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Normal,
            target_collector: "binary-hasher".to_string(),
            condition_type: ConditionType::ProcessNamePattern("test".to_string()),
        }
    }

    #[test]
    fn test_trigger_manager_creation() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        let stats = manager.get_statistics();
        assert_eq!(stats.registered_conditions, 0);
        assert_eq!(stats.pending_triggers, 0);
    }

    #[test]
    fn test_condition_registration() {
        let config = TriggerConfig::default();
        let mut manager = TriggerManager::new(config);

        let condition = create_test_condition();
        manager.register_condition(condition);

        let stats = manager.get_statistics();
        assert_eq!(stats.registered_conditions, 1);
    }

    #[test]
    fn test_process_name_pattern_matching() {
        let config = TriggerConfig::default();
        let mut manager = TriggerManager::new(config);

        let condition = create_test_condition();
        manager.register_condition(condition);

        let process_data = create_test_process_data();
        let triggers = manager.evaluate_triggers(&process_data).unwrap();

        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].target_collector, "binary-hasher");
        assert_eq!(triggers[0].analysis_type, AnalysisType::BinaryHash);
        assert_eq!(triggers[0].target_pid, Some(1234));
    }

    #[test]
    fn test_missing_executable_condition() {
        let config = TriggerConfig::default();
        let mut manager = TriggerManager::new(config);

        let condition = TriggerCondition {
            id: "missing_exe".to_string(),
            description: "Missing executable".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_collector: "binary-hasher".to_string(),
            condition_type: ConditionType::MissingExecutable,
        };
        manager.register_condition(condition);

        let mut process_data = create_test_process_data();
        process_data.file_exists = false;

        let triggers = manager.evaluate_triggers(&process_data).unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].priority, TriggerPriority::High);
    }

    #[test]
    fn test_resource_anomaly_condition() {
        let config = TriggerConfig::default();
        let mut manager = TriggerManager::new(config);

        let condition = TriggerCondition {
            id: "resource_anomaly".to_string(),
            description: "Resource anomaly".to_string(),
            analysis_type: AnalysisType::BehavioralAnalysis,
            priority: TriggerPriority::High,
            target_collector: "behavior-analyzer".to_string(),
            condition_type: ConditionType::ResourceAnomaly {
                cpu_threshold: 80.0,
                memory_threshold: 1024 * 1024 * 1024, // 1GB
            },
        };
        manager.register_condition(condition);

        let mut process_data = create_test_process_data();
        process_data.cpu_usage = Some(90.0); // Above threshold

        let triggers = manager.evaluate_triggers(&process_data).unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].analysis_type, AnalysisType::BehavioralAnalysis);
    }

    #[test]
    fn test_deduplication() {
        let config = TriggerConfig {
            deduplication_window_secs: 60,
            ..Default::default()
        };
        let mut manager = TriggerManager::new(config);

        let condition = create_test_condition();
        manager.register_condition(condition);

        let process_data = create_test_process_data();

        // First evaluation should generate trigger
        let triggers1 = manager.evaluate_triggers(&process_data).unwrap();
        assert_eq!(triggers1.len(), 1);

        // Second evaluation should be deduplicated
        let triggers2 = manager.evaluate_triggers(&process_data).unwrap();
        assert_eq!(triggers2.len(), 0);
    }

    #[test]
    fn test_rate_limiting() {
        let config = TriggerConfig {
            max_triggers_per_collector: 2,
            rate_limit_window_secs: 60,
            deduplication_window_secs: 0, // Disable deduplication for this test
            ..Default::default()
        };
        let mut manager = TriggerManager::new(config);

        let condition = create_test_condition();
        manager.register_condition(condition);

        // Generate triggers for different PIDs to avoid deduplication
        for i in 0..5 {
            let mut process_data = create_test_process_data();
            process_data.pid = 1000 + i;

            let triggers = manager.evaluate_triggers(&process_data).unwrap();

            if i < 2 {
                assert_eq!(triggers.len(), 1, "Trigger {} should be allowed", i);
            } else {
                assert_eq!(triggers.len(), 0, "Trigger {} should be rate limited", i);
            }
        }
    }

    #[test]
    fn test_trigger_metadata() {
        let config = TriggerConfig {
            enable_metadata_tracking: true,
            ..Default::default()
        };
        let mut manager = TriggerManager::new(config);

        let condition = create_test_condition();
        manager.register_condition(condition);

        let process_data = create_test_process_data();
        let triggers = manager.evaluate_triggers(&process_data).unwrap();

        assert_eq!(triggers.len(), 1);
        let trigger = &triggers[0];

        assert!(trigger.metadata.contains_key("condition_id"));
        assert!(trigger.metadata.contains_key("source_pid"));
        assert!(trigger.metadata.contains_key("executable_path"));
        assert!(!trigger.correlation_id.is_empty());
    }

    #[test]
    fn test_cleanup() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Cleanup should not fail even with empty caches
        assert!(manager.cleanup().is_ok());
    }

    #[test]
    fn test_deduplication_key_equality() {
        let key1 = DeduplicationKey {
            collector: "binary-hasher".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_string()),
        };

        let key2 = DeduplicationKey {
            collector: "binary-hasher".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_string()),
        };

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_trigger_priority_ordering() {
        assert!(TriggerPriority::Critical > TriggerPriority::High);
        assert!(TriggerPriority::High > TriggerPriority::Normal);
        assert!(TriggerPriority::Normal > TriggerPriority::Low);
    }
}
