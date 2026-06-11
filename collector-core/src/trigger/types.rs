//! Core trigger data types, configuration, and error definitions.
//!
//! This submodule holds the pure data types for the trigger system:
//! configuration, conditions, deduplication keys, metadata, process data,
//! capabilities, aggregate statistics, and the [`TriggerError`] enum.

use crate::event::{AnalysisType, TriggerPriority};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

use super::queue::QueueStatistics;
use super::sql_evaluator::SqlEvaluationStats;

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

    /// Default timeout for trigger requests (in seconds)
    pub default_timeout_secs: u64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            max_triggers_per_collector: 100,
            rate_limit_window_secs: 60,
            deduplication_window_secs: 300, // 5 minutes
            max_pending_triggers: 1000,
            enable_metadata_tracking: true,
            default_timeout_secs: 30,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
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

    /// Custom SQL condition (not yet supported, always evaluates to false)
    /// See issue #000 for tracking
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

/// Trigger capabilities advertised by collectors.
///
/// This structure defines the trigger conditions that a collector can evaluate
/// and the analysis types it can perform. These capabilities are advertised
/// during collector registration and used for trigger condition validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCapabilities {
    /// Collector identifier
    pub collector_id: String,

    /// Supported trigger condition types
    pub supported_conditions: Vec<ConditionType>,

    /// Supported analysis types
    pub supported_analysis: Vec<AnalysisType>,

    /// Maximum trigger rate per second
    pub max_trigger_rate: u32,

    /// Maximum concurrent analysis tasks
    pub max_concurrent_analysis: u32,

    /// Supported priority levels
    pub supported_priorities: Vec<TriggerPriority>,

    /// Resource limits for analysis
    pub resource_limits: TriggerResourceLimits,
}

/// Resource limits for trigger analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerResourceLimits {
    /// Maximum memory usage per analysis task (bytes)
    pub max_memory_per_task: u64,

    /// Maximum analysis time per task (milliseconds)
    pub max_analysis_time_ms: u64,

    /// Maximum queue depth for pending analysis
    pub max_queue_depth: usize,
}

impl Default for TriggerResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_per_task: 100 * 1024 * 1024, // 100MB
            max_analysis_time_ms: 30_000,           // 30 seconds
            max_queue_depth: 1000,
        }
    }
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

    /// Number of registered collector capabilities
    pub registered_capabilities: usize,

    /// Priority queue statistics
    pub queue_stats: QueueStatistics,

    /// SQL evaluation statistics
    pub sql_evaluation_stats: SqlEvaluationStats,

    /// Trigger emission statistics
    pub emission_stats: TriggerEmissionStats,
}

/// Statistics for trigger request emission and routing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriggerEmissionStats {
    /// Total trigger requests emitted
    pub total_emitted: u64,

    /// Successful emissions to event bus
    pub successful_emissions: u64,

    /// Failed emissions due to validation errors
    pub validation_failures: u64,

    /// Failed emissions due to event bus errors
    pub event_bus_failures: u64,

    /// Trigger requests that timed out
    pub timeouts: u64,

    /// Average emission latency in milliseconds
    pub avg_emission_latency_ms: f64,

    /// Trigger requests currently pending response
    pub pending_responses: usize,
}

/// Errors that can occur in the trigger system.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TriggerError {
    #[error("Lock error on {0}")]
    LockError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Condition evaluation error: {0}")]
    EvaluationError(String),

    #[error("Trigger generation error: {0}")]
    GenerationError(String),

    #[error("SQL parsing error: {0}")]
    SqlParsingError(String),

    #[error("Capability validation error: {0}")]
    CapabilityValidationError(String),

    #[error("Queue full: {0}")]
    QueueFull(String),

    #[error("Backpressure active - dropping low priority triggers")]
    BackpressureActive,

    #[error("Collector not found: {0}")]
    CollectorNotFound(String),

    #[error("Invalid trigger priority: {0:?}")]
    InvalidPriority(TriggerPriority),

    #[error("Trigger emission failed: {0}")]
    EmissionError(String),

    #[error("Event bus error: {0}")]
    EventBusError(String),

    #[error("Trigger validation failed: {0}")]
    ValidationError(String),

    #[error("Trigger timeout: {0}")]
    TimeoutError(String),

    #[error("Correlation tracking error: {0}")]
    CorrelationError(String),
}
