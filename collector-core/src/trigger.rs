//! Trigger system for analysis collector coordination.
//!
//! This module provides the infrastructure for coordinating between process
//! monitoring collectors and analysis collectors (Binary Hasher, Memory Analyzer, etc.).
//! It implements trigger condition evaluation, deduplication, rate limiting, and
//! metadata tracking for forensic analysis.

use crate::event::{AnalysisType, CollectionEvent, TriggerPriority, TriggerRequest};
use crate::event_bus::EventBus;
use serde::{Deserialize, Serialize};
use sqlparser::{dialect::GenericDialect, parser::Parser};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
/// process monitoring and analysis collectors with SQL-to-IPC integration.
pub struct TriggerManager {
    /// Configuration
    config: TriggerConfig,

    /// Registered trigger conditions
    conditions: Arc<Mutex<Vec<TriggerCondition>>>,

    /// Rate limiting state per collector
    rate_limits: Arc<Mutex<HashMap<String, RateLimitState>>>,

    /// Deduplication tracking
    deduplication_cache: Arc<Mutex<HashMap<DeduplicationKey, SystemTime>>>,

    /// Trigger metadata tracking
    metadata_cache: Arc<Mutex<HashMap<String, TriggerMetadata>>>,

    /// Pending trigger count
    pending_count: Arc<Mutex<usize>>,

    /// Collector capabilities registry
    collector_capabilities: Arc<Mutex<HashMap<String, TriggerCapabilities>>>,

    /// Priority-based trigger queue
    trigger_queue: Arc<Mutex<PriorityTriggerQueue>>,

    /// SQL trigger evaluator
    sql_evaluator: Arc<Mutex<SqlTriggerEvaluator>>,

    /// Event bus for trigger request emission
    event_bus: Arc<RwLock<Option<Box<dyn EventBus + Send + Sync>>>>,

    /// Trigger emission statistics
    emission_stats: Arc<Mutex<TriggerEmissionStats>>,

    /// Timeout tracking for pending trigger requests
    timeout_tracker: Arc<Mutex<HashMap<String, TriggerTimeout>>>,
}

impl TriggerManager {
    /// Creates a new trigger manager with the specified configuration.
    ///
    /// Initializes all internal state including rate limiting, deduplication caches,
    /// and the priority-based trigger queue. The manager starts with no registered
    /// conditions or collector capabilities.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration parameters for trigger behavior
    ///
    /// # Examples
    ///
    /// ```
    /// use collector_core::trigger::{TriggerManager, TriggerConfig};
    ///
    /// let config = TriggerConfig::default();
    /// let manager = TriggerManager::new(config);
    /// ```
    pub fn new(config: TriggerConfig) -> Self {
        let queue_size = config.max_pending_triggers;
        let backpressure_threshold = 0.8; // 80% threshold for backpressure

        Self {
            config,
            conditions: Arc::new(Mutex::new(Vec::new())),
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            deduplication_cache: Arc::new(Mutex::new(HashMap::new())),
            metadata_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_count: Arc::new(Mutex::new(0)),
            collector_capabilities: Arc::new(Mutex::new(HashMap::new())),
            trigger_queue: Arc::new(Mutex::new(PriorityTriggerQueue::new(
                queue_size,
                backpressure_threshold,
            ))),
            sql_evaluator: Arc::new(Mutex::new(SqlTriggerEvaluator::new())),
            event_bus: Arc::new(RwLock::new(None)),
            emission_stats: Arc::new(Mutex::new(TriggerEmissionStats::default())),
            timeout_tracker: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers collector capabilities for trigger condition validation.
    ///
    /// This method validates the provided capabilities and registers them with
    /// the SQL evaluator for condition matching. Capabilities define what types
    /// of analysis the collector can perform and its resource constraints.
    ///
    /// # Arguments
    ///
    /// * `capabilities` - Collector capability specification
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Capabilities validation fails (empty ID, invalid limits)
    /// - SQL evaluator registration fails
    /// - Lock acquisition fails
    pub fn register_collector_capabilities(
        &self,
        capabilities: TriggerCapabilities,
    ) -> Result<(), TriggerError> {
        info!(
            collector_id = %capabilities.collector_id,
            supported_conditions = capabilities.supported_conditions.len(),
            supported_analysis = capabilities.supported_analysis.len(),
            max_trigger_rate = capabilities.max_trigger_rate,
            "Registering collector trigger capabilities"
        );

        // Validate capabilities
        self.validate_collector_capabilities(&capabilities)?;

        // Register with SQL evaluator
        if let Ok(mut evaluator) = self.sql_evaluator.lock() {
            // Extract conditions that this collector can handle
            let conditions = self
                .conditions
                .lock()
                .map_err(|_| TriggerError::LockError("conditions".to_string()))?;
            let collector_conditions: Vec<TriggerCondition> = conditions
                .iter()
                .filter(|condition| {
                    condition.target_collector == capabilities.collector_id
                        && capabilities.supported_conditions.iter().any(|supported| {
                            self.condition_types_compatible(supported, &condition.condition_type)
                        })
                        && capabilities
                            .supported_analysis
                            .contains(&condition.analysis_type)
                })
                .cloned()
                .collect();

            evaluator.register_collector_conditions(
                capabilities.collector_id.clone(),
                collector_conditions,
            )?;
        }

        // Store capabilities
        if let Ok(mut caps) = self.collector_capabilities.lock() {
            caps.insert(capabilities.collector_id.clone(), capabilities);
        }

        Ok(())
    }

    /// Validates collector capabilities against system requirements.
    fn validate_collector_capabilities(
        &self,
        capabilities: &TriggerCapabilities,
    ) -> Result<(), TriggerError> {
        // Validate collector ID format
        if capabilities.collector_id.is_empty() {
            return Err(TriggerError::ConfigError(
                "Collector ID cannot be empty".to_string(),
            ));
        }

        // Validate trigger rate limits
        if capabilities.max_trigger_rate == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum trigger rate must be greater than 0".to_string(),
            ));
        }

        // Validate resource limits
        if capabilities.resource_limits.max_memory_per_task == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum memory per task must be greater than 0".to_string(),
            ));
        }

        if capabilities.resource_limits.max_analysis_time_ms == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum analysis time must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }

    /// Checks if two condition types are compatible (same variant, ignoring data).
    fn condition_types_compatible(
        &self,
        supported: &ConditionType,
        requested: &ConditionType,
    ) -> bool {
        use std::mem::discriminant;
        discriminant(supported) == discriminant(requested)
    }

    /// Validates a trigger condition against registered collector capabilities.
    pub fn validate_trigger_condition(
        &self,
        condition: &TriggerCondition,
    ) -> Result<(), TriggerError> {
        let capabilities = self
            .collector_capabilities
            .lock()
            .map_err(|_| TriggerError::LockError("collector_capabilities".to_string()))?;

        let collector_caps = capabilities
            .get(&condition.target_collector)
            .ok_or_else(|| {
                TriggerError::ConfigError(format!(
                    "No capabilities registered for collector: {}",
                    condition.target_collector
                ))
            })?;

        // Validate condition type support (check by discriminant, not exact match)
        let condition_supported = collector_caps
            .supported_conditions
            .iter()
            .any(|supported| self.condition_types_compatible(supported, &condition.condition_type));

        if !condition_supported {
            return Err(TriggerError::ConfigError(format!(
                "Collector {} does not support condition type: {:?}",
                condition.target_collector, condition.condition_type
            )));
        }

        // Validate analysis type support
        if !collector_caps
            .supported_analysis
            .contains(&condition.analysis_type)
        {
            return Err(TriggerError::ConfigError(format!(
                "Collector {} does not support analysis type: {:?}",
                condition.target_collector, condition.analysis_type
            )));
        }

        // Validate priority support
        if !collector_caps
            .supported_priorities
            .contains(&condition.priority)
        {
            return Err(TriggerError::ConfigError(format!(
                "Collector {} does not support priority level: {:?}",
                condition.target_collector, condition.priority
            )));
        }

        Ok(())
    }

    /// Registers a trigger condition for evaluation.
    ///
    /// Validates the condition against registered collector capabilities before
    /// adding it to the active conditions list. The condition will be evaluated
    /// against incoming process data during trigger evaluation.
    ///
    /// # Arguments
    ///
    /// * `condition` - Trigger condition to register
    ///
    /// # Errors
    ///
    /// Returns an error if the condition validation fails against collector capabilities.
    pub fn register_condition(&self, condition: TriggerCondition) -> Result<(), TriggerError> {
        info!(
            condition_id = %condition.id,
            analysis_type = ?condition.analysis_type,
            target_collector = %condition.target_collector,
            "Registering trigger condition"
        );

        // Validate condition against collector capabilities
        self.validate_trigger_condition(&condition)?;

        let collector_id = condition.target_collector.clone();

        // Add condition to the list
        {
            let mut conditions = self
                .conditions
                .lock()
                .map_err(|_| TriggerError::LockError("conditions".to_string()))?;
            conditions.push(condition);
        }

        // Get evaluator lock and register conditions
        let mut evaluator = self
            .sql_evaluator
            .lock()
            .map_err(|_| TriggerError::LockError("sql_evaluator".to_string()))?;

        // Get conditions for this collector
        let collector_conditions: Vec<_> = {
            let conditions = self
                .conditions
                .lock()
                .map_err(|_| TriggerError::LockError("conditions".to_string()))?;
            conditions
                .iter()
                .filter(|c| c.target_collector == collector_id)
                .cloned()
                .collect()
        };
        evaluator.register_collector_conditions(collector_id, collector_conditions)?;

        Ok(())
    }

    /// Evaluates trigger conditions using SQL-to-IPC integration.
    ///
    /// Processes the provided process data through the SQL evaluator to generate
    /// trigger requests. Successfully generated triggers are enqueued in the
    /// priority queue for processing.
    ///
    /// # Arguments
    ///
    /// * `collector_id` - ID of the collector requesting evaluation
    /// * `process_data` - Process data to evaluate against conditions
    ///
    /// # Returns
    ///
    /// Vector of successfully generated and enqueued trigger requests.
    ///
    /// # Errors
    ///
    /// Returns an error if SQL evaluation or queue operations fail.
    pub fn evaluate_sql_triggers(
        &self,
        collector_id: &str,
        process_data: &ProcessTriggerData,
    ) -> Result<Vec<TriggerRequest>, TriggerError> {
        let mut evaluator = self
            .sql_evaluator
            .lock()
            .map_err(|_| TriggerError::LockError("sql_evaluator".to_string()))?;

        let triggers = evaluator.evaluate_sql_triggers(collector_id, process_data)?;

        // Enqueue triggers with priority handling
        let mut queue = self
            .trigger_queue
            .lock()
            .map_err(|_| TriggerError::LockError("trigger_queue".to_string()))?;

        let mut successful_triggers = Vec::with_capacity(triggers.len());

        for trigger in triggers {
            let trigger_id = trigger.trigger_id.clone();
            let priority = trigger.priority.clone();

            match queue.enqueue(trigger.clone()) {
                Ok(()) => {
                    successful_triggers.push(trigger);
                    debug!(
                        trigger_id = %trigger_id,
                        priority = ?priority,
                        "Successfully enqueued trigger request"
                    );
                }
                Err(e) => {
                    warn!(
                        trigger_id = %trigger_id,
                        error = %e,
                        "Failed to enqueue trigger request"
                    );
                }
            }
        }

        Ok(successful_triggers)
    }

    /// Dequeues the next trigger request for processing.
    pub fn dequeue_trigger(&self) -> Option<TriggerRequest> {
        if let Ok(mut queue) = self.trigger_queue.lock() {
            queue.dequeue()
        } else {
            None
        }
    }

    /// Checks if the trigger queue is experiencing backpressure.
    pub fn is_backpressure_active(&self) -> bool {
        self.trigger_queue
            .lock()
            .map(|queue| queue.is_backpressure_active())
            .unwrap_or(false)
    }

    /// Returns the current queue depth.
    pub fn get_queue_depth(&self) -> usize {
        self.trigger_queue
            .lock()
            .map(|queue| queue.len())
            .unwrap_or(0)
    }

    /// Returns collector capabilities for a specific collector.
    ///
    /// # Arguments
    ///
    /// * `collector_id` - ID of the collector to query
    ///
    /// # Returns
    ///
    /// Collector capabilities if found, None otherwise.
    pub fn get_collector_capabilities(&self, collector_id: &str) -> Option<TriggerCapabilities> {
        self.collector_capabilities
            .lock()
            .ok()?
            .get(collector_id)
            .cloned()
    }

    /// Returns all registered collector capabilities.
    ///
    /// # Returns
    ///
    /// HashMap mapping collector IDs to their capabilities. Returns empty map if lock fails.
    pub fn get_all_collector_capabilities(&self) -> HashMap<String, TriggerCapabilities> {
        self.collector_capabilities
            .lock()
            .map(|caps| caps.clone())
            .unwrap_or_default()
    }

    /// Returns whether a trigger is currently being tracked for timeout.
    ///
    /// This method is primarily intended for testing and debugging purposes.
    pub fn is_trigger_tracked(&self, trigger_id: &str) -> bool {
        self.timeout_tracker
            .lock()
            .map(|tracker| tracker.contains_key(trigger_id))
            .unwrap_or(false)
    }

    /// Evaluates trigger conditions against process event data.
    ///
    /// Returns trigger requests that should be sent to analysis collectors
    /// after applying deduplication and rate limiting.
    pub fn evaluate_triggers(
        &self,
        process_data: &ProcessTriggerData,
    ) -> Result<Vec<TriggerRequest>, TriggerError> {
        let conditions = self
            .conditions
            .lock()
            .map_err(|_| TriggerError::LockError("conditions".to_string()))?;
        let mut triggers = Vec::with_capacity(conditions.len());

        // Evaluate each registered condition
        for condition in conditions.iter() {
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

        // Pending count is derived from the queue length, no separate bookkeeping needed
        // The queue itself is the source of truth for pending triggers

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
        if let Some(path) = &data.executable_path {
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

    /// Sets the event bus for trigger request emission.
    ///
    /// This method must be called before emitting trigger requests to analysis collectors.
    /// The event bus provides the communication channel for coordinating with analysis collectors.
    ///
    /// # Arguments
    ///
    /// * `event_bus` - Event bus implementation for inter-collector communication
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::trigger::{TriggerManager, TriggerConfig};
    /// use collector_core::{LocalEventBus, EventBusConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = TriggerConfig::default();
    ///     let mut manager = TriggerManager::new(config);
    ///
    ///     let event_bus_config = EventBusConfig::default();
    ///     let event_bus = LocalEventBus::new(event_bus_config).await?;
    ///     manager.set_event_bus(Box::new(event_bus)).await;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn set_event_bus(&self, event_bus: Box<dyn EventBus + Send + Sync>) {
        let mut bus_guard = self.event_bus.write().await;
        *bus_guard = Some(event_bus);
        info!("Event bus configured for trigger request emission");
    }

    /// Emits a trigger request to the appropriate analysis collector via the event bus.
    ///
    /// This method validates the trigger request against collector capabilities,
    /// routes it through the event bus, and tracks correlation metadata for
    /// forensic analysis and debugging.
    ///
    /// # Arguments
    ///
    /// * `trigger` - Trigger request to emit
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the trigger was successfully emitted, or an error if:
    /// - Event bus is not configured
    /// - Trigger validation fails
    /// - Event bus communication fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::trigger::{TriggerManager, TriggerConfig};
    /// use collector_core::{TriggerRequest, AnalysisType, TriggerPriority};
    /// use std::collections::HashMap;
    /// use std::time::SystemTime;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = TriggerConfig::default();
    ///     let manager = TriggerManager::new(config);
    ///
    ///     let trigger = TriggerRequest {
    ///         trigger_id: "trigger_123".to_string(),
    ///         target_collector: "binary-hasher".to_string(),
    ///         analysis_type: AnalysisType::BinaryHash,
    ///         priority: TriggerPriority::High,
    ///         target_pid: Some(1234),
    ///         target_path: Some("/usr/bin/suspicious".to_string()),
    ///         correlation_id: "corr_456".to_string(),
    ///         metadata: HashMap::new(),
    ///         timestamp: SystemTime::now(),
    ///     };
    ///
    ///     manager.emit_trigger_request(trigger).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn emit_trigger_request(&self, trigger: TriggerRequest) -> Result<(), TriggerError> {
        let start_time = SystemTime::now();

        // Validate trigger request against collector capabilities
        self.validate_trigger_request(&trigger).await?;

        // Clone the EventBus Arc to avoid holding the lock during async operations
        let event_bus_clone = {
            let bus_guard = self.event_bus.read().await;
            bus_guard.as_ref().ok_or_else(|| {
                TriggerError::EventBusError("Event bus not configured".to_string())
            })?;
            // We need to clone the Arc-wrapped reference, but since it's a Box<dyn EventBus>,
            // we can't directly clone it. For now, we'll work around this by re-acquiring
            // the lock in the internal function. A better long-term solution would be to
            // change the field type to Arc<dyn EventBus> instead of Box<dyn EventBus>.
            Arc::clone(&self.event_bus)
        };
        // Lock is dropped here

        self.emit_trigger_request_internal(event_bus_clone, trigger, start_time)
            .await
    }

    /// Internal implementation of trigger request emission.
    ///
    /// This method handles the actual emission logic, accepting an Arc-wrapped
    /// event bus reference to avoid holding locks during async operations.
    async fn emit_trigger_request_internal(
        &self,
        event_bus_arc: Arc<RwLock<Option<Box<dyn EventBus + Send + Sync>>>>,
        trigger: TriggerRequest,
        start_time: SystemTime,
    ) -> Result<(), TriggerError> {
        let trigger_id = trigger.trigger_id.clone();
        let correlation_id = trigger.correlation_id.clone();
        let target_collector = trigger.target_collector.clone();

        info!(
            trigger_id = %trigger_id,
            target_collector = %target_collector,
            analysis_type = ?trigger.analysis_type,
            priority = ?trigger.priority,
            correlation_id = %correlation_id,
            "Emitting trigger request to analysis collector"
        );

        // Create collection event for the trigger request
        let collection_event = CollectionEvent::TriggerRequest(trigger.clone());

        // Acquire lock only for the duration of the publish call
        let publish_result = {
            let mut bus_guard = event_bus_arc.write().await;
            let event_bus = bus_guard.as_mut().ok_or_else(|| {
                TriggerError::EventBusError("Event bus not configured".to_string())
            })?;

            event_bus
                .publish(
                    collection_event,
                    crate::event_bus::CorrelationMetadata::new(correlation_id.clone())
                        .with_tag("trigger_source".to_string(), "trigger_manager".to_string()),
                )
                .await
        };
        // Lock is dropped here

        if let Err(err) = publish_result {
            let mut stats = self
                .emission_stats
                .lock()
                .map_err(|_| TriggerError::LockError("emission_stats".to_string()))?;
            stats.event_bus_failures += 1;
            return Err(TriggerError::EventBusError(err.to_string()));
        }

        // Track timeout for this trigger request
        self.track_trigger_timeout(&trigger).await?;

        // Update emission statistics in a single lock acquisition
        {
            let mut stats = self
                .emission_stats
                .lock()
                .map_err(|_| TriggerError::LockError("emission_stats".to_string()))?;

            stats.total_emitted += 1;
            stats.pending_responses += 1;
            stats.successful_emissions += 1;

            // Calculate emission latency
            if let Ok(duration) = start_time.elapsed() {
                let latency_ms = duration.as_millis() as f64;

                // Update running average
                let total_emissions = stats.total_emitted as f64;
                stats.avg_emission_latency_ms =
                    (stats.avg_emission_latency_ms * (total_emissions - 1.0) + latency_ms)
                        / total_emissions;
            }
        }

        debug!(
            trigger_id = %trigger_id,
            target_collector = %target_collector,
            correlation_id = %correlation_id,
            "Trigger request emitted successfully"
        );

        Ok(())
    }

    /// Validates a trigger request against registered collector capabilities.
    ///
    /// This method ensures that the target collector exists, supports the requested
    /// analysis type, and can handle the specified priority level.
    pub async fn validate_trigger_request(
        &self,
        trigger: &TriggerRequest,
    ) -> Result<(), TriggerError> {
        let capabilities = self
            .collector_capabilities
            .lock()
            .map_err(|_| TriggerError::LockError("collector_capabilities".to_string()))?;

        // Check if target collector is registered
        let collector_caps = capabilities.get(&trigger.target_collector).ok_or_else(|| {
            TriggerError::ValidationError(format!(
                "Target collector '{}' not found in capabilities registry",
                trigger.target_collector
            ))
        })?;

        // Validate analysis type support
        if !collector_caps
            .supported_analysis
            .contains(&trigger.analysis_type)
        {
            return Err(TriggerError::ValidationError(format!(
                "Collector '{}' does not support analysis type: {:?}",
                trigger.target_collector, trigger.analysis_type
            )));
        }

        // Validate priority support
        if !collector_caps
            .supported_priorities
            .contains(&trigger.priority)
        {
            return Err(TriggerError::ValidationError(format!(
                "Collector '{}' does not support priority level: {:?}",
                trigger.target_collector, trigger.priority
            )));
        }

        // Validate resource constraints
        if trigger.target_path.as_ref().map(|p| p.len()).unwrap_or(0) > 4096 {
            return Err(TriggerError::ValidationError(
                "Target path exceeds maximum length (4096 characters)".to_string(),
            ));
        }

        // Validate metadata size
        let metadata_size: usize = trigger
            .metadata
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();

        if metadata_size > 65536 {
            // 64KB limit
            return Err(TriggerError::ValidationError(
                "Trigger metadata exceeds maximum size (64KB)".to_string(),
            ));
        }

        Ok(())
    }

    /// Tracks timeout for a trigger request to enable timeout handling.
    ///
    /// This method adds the trigger to the timeout tracker, allowing the system
    /// to detect and handle triggers that don't receive responses within the
    /// configured timeout period.
    pub async fn track_trigger_timeout(
        &self,
        trigger: &TriggerRequest,
    ) -> Result<(), TriggerError> {
        let timeout_duration = Duration::from_secs(30); // Default 30-second timeout

        let timeout_info = TriggerTimeout {
            target_collector: trigger.target_collector.clone(),
            emitted_at: SystemTime::now(),
            timeout_duration,
            correlation_id: trigger.correlation_id.clone(),
        };

        let mut tracker = self
            .timeout_tracker
            .lock()
            .map_err(|_| TriggerError::LockError("timeout_tracker".to_string()))?;

        tracker.insert(trigger.trigger_id.clone(), timeout_info);

        debug!(
            trigger_id = %trigger.trigger_id,
            timeout_duration_secs = timeout_duration.as_secs(),
            "Tracking trigger request timeout"
        );

        Ok(())
    }

    /// Handles timeout cleanup for expired trigger requests.
    ///
    /// This method should be called periodically to clean up triggers that have
    /// exceeded their timeout period and update statistics accordingly.
    pub async fn handle_trigger_timeouts(&self) -> Result<Vec<String>, TriggerError> {
        let mut timed_out_triggers = Vec::new();
        let now = SystemTime::now();

        {
            let mut tracker = self
                .timeout_tracker
                .lock()
                .map_err(|_| TriggerError::LockError("timeout_tracker".to_string()))?;

            let mut expired_triggers = Vec::new();

            // Find expired triggers
            for (trigger_id, timeout_info) in tracker.iter() {
                if let Ok(elapsed) = now.duration_since(timeout_info.emitted_at) {
                    if elapsed > timeout_info.timeout_duration {
                        expired_triggers.push(trigger_id.clone());
                        timed_out_triggers.push(trigger_id.clone());

                        warn!(
                            trigger_id = %trigger_id,
                            target_collector = %timeout_info.target_collector,
                            correlation_id = %timeout_info.correlation_id,
                            elapsed_secs = elapsed.as_secs(),
                            timeout_secs = timeout_info.timeout_duration.as_secs(),
                            "Trigger request timed out"
                        );
                    }
                }
            }

            // Remove expired triggers
            for trigger_id in &expired_triggers {
                tracker.remove(trigger_id);
            }
        }

        // Update statistics
        if !timed_out_triggers.is_empty() {
            let mut stats = self
                .emission_stats
                .lock()
                .map_err(|_| TriggerError::LockError("emission_stats".to_string()))?;

            stats.timeouts += timed_out_triggers.len() as u64;
            stats.pending_responses = stats
                .pending_responses
                .saturating_sub(timed_out_triggers.len());
        }

        Ok(timed_out_triggers)
    }

    /// Marks a trigger request as completed, removing it from timeout tracking.
    ///
    /// This method should be called when a trigger request receives a response
    /// from the analysis collector to clean up tracking state.
    pub async fn complete_trigger_request(&self, trigger_id: &str) -> Result<(), TriggerError> {
        let mut tracker = self
            .timeout_tracker
            .lock()
            .map_err(|_| TriggerError::LockError("timeout_tracker".to_string()))?;

        if tracker.remove(trigger_id).is_some() {
            // Update statistics
            let mut stats = self
                .emission_stats
                .lock()
                .map_err(|_| TriggerError::LockError("emission_stats".to_string()))?;

            stats.pending_responses = stats.pending_responses.saturating_sub(1);

            debug!(
                trigger_id = %trigger_id,
                "Trigger request completed successfully"
            );
        }

        Ok(())
    }

    /// Returns current trigger statistics for monitoring.
    pub fn get_statistics(&self) -> Result<TriggerStatistics, TriggerError> {
        let pending_count = self.pending_count.lock().map(|count| *count).unwrap_or(0);

        // Batch lock acquisitions to minimize lock contention
        let (
            dedup_cache_size,
            rate_limit_states,
            registered_capabilities,
            queue_stats,
            sql_evaluation_stats,
            emission_stats,
        ) = {
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
            let registered_capabilities = self
                .collector_capabilities
                .lock()
                .map(|caps| caps.len())
                .unwrap_or(0);
            let queue_stats = self
                .trigger_queue
                .lock()
                .map(|queue| queue.get_statistics())
                .unwrap_or_default();
            let sql_evaluation_stats = self
                .sql_evaluator
                .lock()
                .map(|evaluator| evaluator.get_evaluation_stats())
                .unwrap_or_default();
            let emission_stats = self
                .emission_stats
                .lock()
                .map(|stats| stats.clone())
                .unwrap_or_default();

            (
                dedup_cache_size,
                rate_limit_states,
                registered_capabilities,
                queue_stats,
                sql_evaluation_stats,
                emission_stats,
            )
        };

        let conditions_len = self
            .conditions
            .lock()
            .map_err(|_| TriggerError::LockError("conditions".to_string()))?
            .len();

        Ok(TriggerStatistics {
            registered_conditions: conditions_len,
            pending_triggers: pending_count,
            deduplication_cache_size: dedup_cache_size,
            rate_limit_states,
            registered_capabilities,
            queue_stats,
            sql_evaluation_stats,
            emission_stats,
        })
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

        // Clean metadata cache with incremental eviction to reduce lock contention
        if let Ok(cache) = self.metadata_cache.lock() {
            let max_pending_triggers = self.config.max_pending_triggers;

            // Check if the cache size exceeds the maximum allowed
            if cache.len() > max_pending_triggers {
                let target_size = max_pending_triggers / 2;
                let to_remove_count = cache.len() - target_size;

                // Collect keys and their generated_at timestamps while holding the lock briefly
                let mut entries_to_sort: Vec<(String, SystemTime)> = cache
                    .iter()
                    .map(|(id, metadata)| (id.clone(), metadata.generated_at))
                    .collect();

                // Release the lock for the heavy sorting work
                drop(cache);

                // Sort entries by generated_at time in ascending order (oldest first)
                entries_to_sort.sort_by_key(|(_, generated_at)| *generated_at);

                // Reacquire the lock to remove elements
                if let Ok(mut cache) = self.metadata_cache.lock() {
                    // Remove the oldest entries
                    for i in 0..to_remove_count {
                        if let Some((key, _)) = entries_to_sort.get(i) {
                            cache.remove(key);
                            debug!(
                                trigger_id = %key,
                                "Evicted old trigger metadata from cache due to size limit."
                            );
                        }
                    }
                    info!(
                        current_size = cache.len(),
                        max_size = max_pending_triggers,
                        "Cleaned up trigger metadata cache, removed {} entries.",
                        to_remove_count
                    );
                }
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

/// Priority-based trigger queue for managing analysis requests.
#[derive(Debug)]
pub struct PriorityTriggerQueue {
    /// High priority queue (Critical, High)
    high_priority: VecDeque<TriggerRequest>,

    /// Normal priority queue (Normal, Low)
    normal_priority: VecDeque<TriggerRequest>,

    /// Maximum queue size per priority level
    max_queue_size: usize,

    /// Backpressure threshold (percentage of max size)
    backpressure_threshold: f32,

    /// Queue statistics
    stats: QueueStatistics,
}

/// Queue statistics for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStatistics {
    /// Total triggers enqueued
    pub total_enqueued: u64,

    /// Total triggers processed
    pub total_processed: u64,

    /// Triggers dropped due to backpressure
    pub dropped_backpressure: u64,

    /// Triggers dropped due to queue full
    pub dropped_queue_full: u64,

    /// Current queue depths
    pub high_priority_depth: usize,
    pub normal_priority_depth: usize,
}

impl PriorityTriggerQueue {
    /// Creates a new priority trigger queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use collector_core::trigger::PriorityTriggerQueue;
    ///
    /// let queue = PriorityTriggerQueue::new(1000, 0.8);
    /// ```
    pub fn new(max_queue_size: usize, backpressure_threshold: f32) -> Self {
        Self {
            high_priority: VecDeque::with_capacity(max_queue_size / 2),
            normal_priority: VecDeque::with_capacity(max_queue_size / 2),
            max_queue_size,
            backpressure_threshold,
            stats: QueueStatistics::default(),
        }
    }

    /// Enqueues a trigger request with priority-based routing.
    pub fn enqueue(&mut self, trigger: TriggerRequest) -> Result<(), TriggerError> {
        self.stats.total_enqueued += 1;

        // Check for backpressure
        if self.is_backpressure_active() {
            // Drop low priority triggers during backpressure
            if matches!(trigger.priority, TriggerPriority::Low) {
                self.stats.dropped_backpressure += 1;
                warn!(
                    trigger_id = %trigger.trigger_id,
                    priority = ?trigger.priority,
                    "Dropping low priority trigger due to backpressure"
                );
                return Err(TriggerError::BackpressureActive);
            }
        }

        // Route to appropriate queue based on priority
        match trigger.priority {
            TriggerPriority::Critical | TriggerPriority::High => {
                if self.high_priority.len() >= self.max_queue_size / 2 {
                    self.stats.dropped_queue_full += 1;
                    return Err(TriggerError::QueueFull("high_priority".to_string()));
                }
                self.high_priority.push_back(trigger);
                self.stats.high_priority_depth = self.high_priority.len();
            }
            TriggerPriority::Normal | TriggerPriority::Low => {
                if self.normal_priority.len() >= self.max_queue_size / 2 {
                    self.stats.dropped_queue_full += 1;
                    return Err(TriggerError::QueueFull("normal_priority".to_string()));
                }
                self.normal_priority.push_back(trigger);
                self.stats.normal_priority_depth = self.normal_priority.len();
            }
        }

        Ok(())
    }

    /// Dequeues the next trigger request, prioritizing high priority items.
    pub fn dequeue(&mut self) -> Option<TriggerRequest> {
        // Always process high priority first
        if let Some(trigger) = self.high_priority.pop_front() {
            self.stats.total_processed += 1;
            self.stats.high_priority_depth = self.high_priority.len();
            return Some(trigger);
        }

        // Then process normal priority
        if let Some(trigger) = self.normal_priority.pop_front() {
            self.stats.total_processed += 1;
            self.stats.normal_priority_depth = self.normal_priority.len();
            return Some(trigger);
        }

        None
    }

    /// Checks if backpressure is currently active.
    pub fn is_backpressure_active(&self) -> bool {
        let total_depth = self.high_priority.len() + self.normal_priority.len();
        let threshold = (self.max_queue_size as f32 * self.backpressure_threshold) as usize;
        total_depth >= threshold
    }

    /// Returns current queue statistics.
    pub fn get_statistics(&self) -> QueueStatistics {
        let mut stats = self.stats.clone();
        stats.high_priority_depth = self.high_priority.len();
        stats.normal_priority_depth = self.normal_priority.len();
        stats
    }

    /// Returns the total number of pending triggers.
    pub fn len(&self) -> usize {
        self.high_priority.len() + self.normal_priority.len()
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.normal_priority.is_empty()
    }
}

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

    /// Total evaluation time (microseconds)
    pub total_evaluation_time_us: u64,

    /// Last evaluation timestamp
    pub last_evaluation: Option<SystemTime>,
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
                "Expression contains disallowed patterns (e.g., SELECT, FROM, table-qualified identifiers, subqueries): {}",
                sql_expr
            )));
        }

        // Wrap the expression in a SELECT statement for parsing
        let sql = format!("SELECT * FROM dummy WHERE {}", sql_expr);

        let statements = Parser::parse_sql(&self.dialect, &sql).map_err(|e| {
            TriggerError::SqlParsingError(format!("Failed to parse SQL expression: {}", e))
        })?;

        if let Some(sqlparser::ast::Statement::Query(query)) = statements.first() {
            if let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref() {
                if let Some(selection) = &select.selection {
                    return Ok(selection.clone());
                }
            }
        }

        Err(TriggerError::SqlParsingError(
            "Could not extract WHERE clause expression from SQL statement.".to_string(),
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
        let conditions = match self.compiled_conditions.get(collector_id) {
            Some(conditions) => conditions,
            None => return Ok(Vec::new()), // No conditions registered for this collector
        };

        let mut triggers = Vec::new();
        let mut match_results = Vec::new();

        for (index, compiled_condition) in conditions.iter().enumerate() {
            let condition_start = SystemTime::now();

            let matches = match self.evaluate_compiled_condition(compiled_condition, process_data) {
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
                    self.create_sql_trigger_request(&compiled_condition.condition, process_data)?;
                triggers.push(trigger);
            }
        }

        // Update condition statistics after the loop to avoid borrow conflicts
        for (index, matches, condition_start) in match_results {
            if let Some(conditions_mut) = self.compiled_conditions.get_mut(collector_id) {
                if let Some(compiled_condition_mut) = conditions_mut.get_mut(index) {
                    compiled_condition_mut.stats.evaluation_count += 1;
                    compiled_condition_mut.stats.last_evaluation = Some(SystemTime::now());

                    if matches {
                        compiled_condition_mut.stats.match_count += 1;
                    }

                    // Update condition evaluation time
                    if let Ok(elapsed) = condition_start.elapsed() {
                        compiled_condition_mut.stats.total_evaluation_time_us +=
                            elapsed.as_micros() as u64;
                    }
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
        &self,
        compiled_condition: &CompiledTriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<bool, TriggerError> {
        match &compiled_condition.condition.condition_type {
            ConditionType::ProcessNamePattern(pattern) => Ok(data.name.contains(pattern)),
            ConditionType::ExecutablePathPattern(pattern) => Ok(data
                .executable_path
                .as_ref()
                .map(|path| path.contains(pattern))
                .unwrap_or(false)),
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
        metadata.insert("evaluation_method".to_string(), "sql_trigger".to_string());

        if let Some(ref path) = data.executable_path {
            metadata.insert("executable_path".to_string(), path.clone());
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

/// Timeout tracking for trigger requests.
#[derive(Debug, Clone)]
struct TriggerTimeout {
    /// Target collector name
    target_collector: String,

    /// Emission timestamp
    emitted_at: SystemTime,

    /// Timeout duration
    timeout_duration: Duration,

    /// Correlation ID for forensic tracking
    correlation_id: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_process_data() -> ProcessTriggerData {
        ProcessTriggerData {
            pid: 1234,
            name: "test_process".to_string(), // This contains "test" which should match the pattern
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

    fn create_test_capabilities() -> TriggerCapabilities {
        TriggerCapabilities {
            collector_id: "binary-hasher".to_string(),
            supported_conditions: vec![
                ConditionType::ProcessNamePattern("test".to_string()), // Match the test condition
                ConditionType::ExecutablePathPattern("".to_string()),
                ConditionType::MissingExecutable,
                ConditionType::HashMismatch,
            ],
            supported_analysis: vec![AnalysisType::BinaryHash, AnalysisType::MemoryAnalysis],
            max_trigger_rate: 100,
            max_concurrent_analysis: 10,
            supported_priorities: vec![
                TriggerPriority::Low,
                TriggerPriority::Normal,
                TriggerPriority::High,
                TriggerPriority::Critical,
            ],
            resource_limits: TriggerResourceLimits::default(),
        }
    }

    #[test]
    fn test_trigger_manager_creation() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.registered_conditions, 0);
        assert_eq!(stats.pending_triggers, 0);
        assert_eq!(stats.registered_capabilities, 0);
    }

    #[test]
    fn test_condition_registration() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // First register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.registered_conditions, 1);
        assert_eq!(stats.registered_capabilities, 1);
    }

    #[test]
    fn test_process_name_pattern_matching() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register capabilities first
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

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
        let manager = TriggerManager::new(config);

        // Register capabilities first
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = TriggerCondition {
            id: "missing_exe".to_string(),
            description: "Missing executable".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_collector: "binary-hasher".to_string(),
            condition_type: ConditionType::MissingExecutable,
        };
        manager.register_condition(condition).unwrap();

        let mut process_data = create_test_process_data();
        process_data.file_exists = false;

        let triggers = manager.evaluate_triggers(&process_data).unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].priority, TriggerPriority::High);
    }

    #[test]
    fn test_resource_anomaly_condition() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register capabilities for behavior analyzer
        let mut capabilities = create_test_capabilities();
        capabilities.collector_id = "behavior-analyzer".to_string();
        capabilities.supported_analysis = vec![AnalysisType::BehavioralAnalysis];
        capabilities
            .supported_conditions
            .push(ConditionType::ResourceAnomaly {
                cpu_threshold: 0.0,
                memory_threshold: 0,
            });
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

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
        manager.register_condition(condition).unwrap();

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
        let manager = TriggerManager::new(config);

        // Register capabilities first
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

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
        let manager = TriggerManager::new(config);

        // Register capabilities first
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

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
        let manager = TriggerManager::new(config);

        // Register capabilities first
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

        let process_data = create_test_process_data();
        let triggers = manager.evaluate_triggers(&process_data).unwrap();

        assert_eq!(triggers.len(), 1);
        let trigger = &triggers[0];

        assert!(trigger.metadata.contains_key("condition_id"));
        assert!(trigger.metadata.contains_key("source_pid"));
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

    #[test]
    fn test_collector_capability_registration() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        let capabilities = create_test_capabilities();
        let collector_id = capabilities.collector_id.clone();

        assert!(
            manager
                .register_collector_capabilities(capabilities)
                .is_ok()
        );

        let stats = manager.get_statistics();
        assert_eq!(stats.unwrap().registered_capabilities, 1);

        let retrieved_caps = manager.get_collector_capabilities(&collector_id);
        assert!(retrieved_caps.is_some());
        assert_eq!(retrieved_caps.unwrap().collector_id, collector_id);
    }

    #[test]
    fn test_capability_validation() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Valid condition should pass validation
        let valid_condition = create_test_condition();
        assert!(manager.validate_trigger_condition(&valid_condition).is_ok());

        // Invalid condition (unsupported analysis type) should fail
        let invalid_condition = TriggerCondition {
            id: "invalid_condition".to_string(),
            description: "Invalid condition".to_string(),
            analysis_type: AnalysisType::YaraScan, // Not supported by test capabilities
            priority: TriggerPriority::Normal,
            target_collector: "binary-hasher".to_string(),
            condition_type: ConditionType::ProcessNamePattern("test".to_string()),
        };
        assert!(
            manager
                .validate_trigger_condition(&invalid_condition)
                .is_err()
        );
    }

    #[test]
    fn test_sql_trigger_evaluation() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register capabilities
        let capabilities = create_test_capabilities();
        let collector_id = capabilities.collector_id.clone();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Register a condition
        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

        // Evaluate triggers using the regular method (which now uses SQL evaluation internally)
        let process_data = create_test_process_data();
        let triggers = manager.evaluate_triggers(&process_data).unwrap();

        // Should generate a trigger for the matching process name
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].target_collector, collector_id);
    }

    #[test]
    fn test_priority_queue_operations() {
        let mut queue = PriorityTriggerQueue::new(100, 0.8);

        // Create triggers with different priorities
        let high_priority_trigger = TriggerRequest {
            trigger_id: "high".to_string(),
            target_collector: "test".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "corr1".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        let low_priority_trigger = TriggerRequest {
            trigger_id: "low".to_string(),
            target_collector: "test".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Low,
            target_pid: Some(5678),
            target_path: None,
            correlation_id: "corr2".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        // Enqueue in reverse priority order
        assert!(queue.enqueue(low_priority_trigger).is_ok());
        assert!(queue.enqueue(high_priority_trigger).is_ok());

        // High priority should be dequeued first
        let first = queue.dequeue().unwrap();
        assert_eq!(first.trigger_id, "high");
        assert_eq!(first.priority, TriggerPriority::High);

        let second = queue.dequeue().unwrap();
        assert_eq!(second.trigger_id, "low");
        assert_eq!(second.priority, TriggerPriority::Low);
    }

    #[test]
    fn test_backpressure_handling() {
        let mut queue = PriorityTriggerQueue::new(10, 0.5); // Small queue with 50% backpressure threshold

        // Fill queue to trigger backpressure (5 items = 50% of 10)
        for i in 0..5 {
            let trigger = TriggerRequest {
                trigger_id: format!("trigger_{}", i),
                target_collector: "test".to_string(),
                analysis_type: AnalysisType::BinaryHash,
                priority: TriggerPriority::Normal,
                target_pid: Some(i as u32),
                target_path: None,
                correlation_id: format!("corr_{}", i),
                metadata: HashMap::new(),
                timestamp: SystemTime::now(),
            };
            assert!(queue.enqueue(trigger).is_ok());
        }

        // Queue should now be in backpressure state
        assert!(queue.is_backpressure_active());

        // Low priority triggers should be dropped
        let low_priority_trigger = TriggerRequest {
            trigger_id: "low_priority".to_string(),
            target_collector: "test".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Low,
            target_pid: Some(9999),
            target_path: None,
            correlation_id: "corr_low".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        let result = queue.enqueue(low_priority_trigger);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::BackpressureActive
        ));

        // High priority triggers should still be accepted
        let high_priority_trigger = TriggerRequest {
            trigger_id: "high_priority".to_string(),
            target_collector: "test".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Critical,
            target_pid: Some(8888),
            target_path: None,
            correlation_id: "corr_high".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        assert!(queue.enqueue(high_priority_trigger).is_ok());
    }

    #[test]
    fn test_trigger_statistics_collection() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Register conditions
        let condition = create_test_condition();
        manager.register_condition(condition).unwrap();

        // Get statistics
        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.registered_conditions, 1);
        assert_eq!(stats.registered_capabilities, 1);
        assert_eq!(stats.queue_stats.total_enqueued, 0);
        assert_eq!(stats.sql_evaluation_stats.total_evaluations, 0);

        // Evaluate triggers to generate statistics
        let process_data = create_test_process_data();
        let _triggers = manager
            .evaluate_sql_triggers("binary-hasher", &process_data)
            .unwrap();

        // Check updated statistics
        let updated_stats = manager.get_statistics();
        assert!(
            updated_stats
                .unwrap()
                .sql_evaluation_stats
                .total_evaluations
                > 0
        );
    }

    #[test]
    fn test_invalid_capability_registration() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Test empty collector ID
        let mut invalid_capabilities = create_test_capabilities();
        invalid_capabilities.collector_id = String::new();
        assert!(
            manager
                .register_collector_capabilities(invalid_capabilities)
                .is_err()
        );

        // Test zero trigger rate
        let mut invalid_capabilities = create_test_capabilities();
        invalid_capabilities.max_trigger_rate = 0;
        assert!(
            manager
                .register_collector_capabilities(invalid_capabilities)
                .is_err()
        );

        // Test zero memory limit
        let mut invalid_capabilities = create_test_capabilities();
        invalid_capabilities.resource_limits.max_memory_per_task = 0;
        assert!(
            manager
                .register_collector_capabilities(invalid_capabilities)
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_trigger_request_emission() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Create a test trigger request
        let trigger = create_test_trigger_request();

        // Test emission without event bus (should fail)
        let result = manager.emit_trigger_request(trigger.clone()).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::EventBusError(_)
        ));

        // Check emission statistics
        let stats = manager.get_statistics().unwrap();
        assert_eq!(stats.emission_stats.total_emitted, 0);
        assert_eq!(stats.emission_stats.successful_emissions, 0);
    }

    #[tokio::test]
    async fn test_trigger_request_validation() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Test validation with valid trigger
        let valid_trigger = create_test_trigger_request();
        let result = manager.validate_trigger_request(&valid_trigger).await;
        assert!(result.is_ok());

        // Test validation with unknown collector
        let mut invalid_trigger = create_test_trigger_request();
        invalid_trigger.target_collector = "unknown-collector".to_string();
        let result = manager.validate_trigger_request(&invalid_trigger).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::ValidationError(_)
        ));

        // Test validation with unsupported analysis type
        let mut invalid_trigger = create_test_trigger_request();
        invalid_trigger.analysis_type = AnalysisType::Custom("unsupported".to_string());
        let result = manager.validate_trigger_request(&invalid_trigger).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::ValidationError(_)
        ));

        // Test validation with oversized metadata
        let mut invalid_trigger = create_test_trigger_request();
        invalid_trigger.metadata.insert(
            "large_key".to_string(),
            "x".repeat(70000), // Exceeds 64KB limit
        );
        let result = manager.validate_trigger_request(&invalid_trigger).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_trigger_timeout_tracking() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Create test trigger request
        let trigger = create_test_trigger_request();
        let trigger_id = trigger.trigger_id.clone();

        // Track timeout
        let result = manager.track_trigger_timeout(&trigger).await;
        assert!(result.is_ok());

        // Check that trigger is being tracked
        {
            let tracker = manager.timeout_tracker.lock().unwrap();
            assert!(tracker.contains_key(&trigger_id));
        }

        // Complete the trigger request
        let result = manager.complete_trigger_request(&trigger_id).await;
        assert!(result.is_ok());

        // Check that trigger is no longer tracked
        let tracker = manager.timeout_tracker.lock().unwrap();
        assert!(!tracker.contains_key(&trigger_id));
    }

    #[tokio::test]
    async fn test_trigger_timeout_handling() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Create test trigger request with immediate timeout
        let trigger = create_test_trigger_request();
        let trigger_id = trigger.trigger_id.clone();

        // Manually add expired timeout
        {
            let mut tracker = manager.timeout_tracker.lock().unwrap();
            let expired_timeout = TriggerTimeout {
                target_collector: trigger.target_collector.clone(),
                emitted_at: SystemTime::now() - Duration::from_secs(60), // 1 minute ago
                timeout_duration: Duration::from_secs(30),               // 30 second timeout
                correlation_id: trigger.correlation_id.clone(),
            };
            tracker.insert(trigger_id.clone(), expired_timeout);
        }

        // Handle timeouts
        let timed_out = manager.handle_trigger_timeouts().await.unwrap();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], trigger_id);

        // Check that expired trigger was removed
        let tracker = manager.timeout_tracker.lock().unwrap();
        assert!(!tracker.contains_key(&trigger_id));
    }

    #[tokio::test]
    async fn test_emission_statistics_tracking() {
        let config = TriggerConfig::default();
        let manager = TriggerManager::new(config);

        // Register collector capabilities
        let capabilities = create_test_capabilities();
        manager
            .register_collector_capabilities(capabilities)
            .unwrap();

        // Check initial statistics
        let initial_stats = manager.get_statistics().unwrap();
        assert_eq!(initial_stats.emission_stats.total_emitted, 0);
        assert_eq!(initial_stats.emission_stats.successful_emissions, 0);
        assert_eq!(initial_stats.emission_stats.validation_failures, 0);

        // Test validation failure tracking
        let invalid_trigger = TriggerRequest {
            trigger_id: "invalid_trigger".to_string(),
            target_collector: "unknown-collector".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "test_correlation".to_string(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        // This should fail validation
        let result = manager.emit_trigger_request(invalid_trigger).await;
        assert!(result.is_err());

        // Statistics should remain unchanged since validation failed before emission
        let stats = manager.get_statistics();
        assert_eq!(stats.unwrap().emission_stats.total_emitted, 0);
    }

    /// Helper function to create a test trigger request.
    fn create_test_trigger_request() -> TriggerRequest {
        TriggerRequest {
            trigger_id: "test_trigger_123".to_string(),
            target_collector: "binary-hasher".to_string(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_string()),
            correlation_id: "test_correlation_456".to_string(),
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("test_key".to_string(), "test_value".to_string());
                metadata
            },
            timestamp: SystemTime::now(),
        }
    }
}
