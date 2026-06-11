//! Trigger system manager for analysis collector coordination.
//!
//! This submodule contains [`TriggerManager`], the central coordinator that
//! evaluates trigger conditions, applies deduplication and rate limiting,
//! emits trigger requests through the event bus, and tracks request timeouts.

#![allow(clippy::significant_drop_tightening)]
#![allow(clippy::as_conversions)]
#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::pattern_type_mismatch)]
#![allow(clippy::integer_division)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::option_if_let_else)]

/// Macro to acquire a `std::sync::Mutex` lock and convert poison errors to `TriggerError`.
///
/// This reduces boilerplate when locking mutexes that need error handling.
///
/// # Usage
/// ```ignore
/// let guard = lock_or_err!(self.conditions, "conditions")?;
/// ```
macro_rules! lock_or_err {
    ($mutex:expr, $name:literal) => {
        $mutex
            .lock()
            .map_err(|_| TriggerError::LockError($name.to_owned()))
    };
}

use crate::event::{CollectionEvent, TriggerRequest};
use crate::event_bus::EventBus;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::queue::PriorityTriggerQueue;
use super::sql_evaluator::SqlTriggerEvaluator;
use super::types::{
    ConditionType, DeduplicationKey, ProcessTriggerData, TriggerCapabilities, TriggerCondition,
    TriggerConfig, TriggerEmissionStats, TriggerError, TriggerMetadata, TriggerStatistics,
};

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
        Self::validate_collector_capabilities(&capabilities)?;

        // Register with SQL evaluator
        {
            let mut evaluator = lock_or_err!(self.sql_evaluator, "sql_evaluator")?;
            // Extract conditions that this collector can handle
            let conditions = lock_or_err!(self.conditions, "conditions")?;
            let collector_conditions: Vec<TriggerCondition> = conditions
                .iter()
                .filter(|condition| {
                    condition.target_collector == capabilities.collector_id
                        && capabilities.supported_conditions.iter().any(|supported| {
                            Self::condition_types_compatible(supported, &condition.condition_type)
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
            )?
        };

        // Store capabilities
        let mut caps = lock_or_err!(self.collector_capabilities, "collector_capabilities")?;
        caps.insert(capabilities.collector_id.clone(), capabilities);

        Ok(())
    }

    /// Validates collector capabilities against system requirements.
    fn validate_collector_capabilities(
        capabilities: &TriggerCapabilities,
    ) -> Result<(), TriggerError> {
        // Validate collector ID format
        if capabilities.collector_id.is_empty() {
            return Err(TriggerError::ConfigError(
                "Collector ID cannot be empty".to_owned(),
            ));
        }

        // Validate trigger rate limits
        if capabilities.max_trigger_rate == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum trigger rate must be greater than 0".to_owned(),
            ));
        }

        // Validate resource limits
        if capabilities.resource_limits.max_memory_per_task == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum memory per task must be greater than 0".to_owned(),
            ));
        }

        if capabilities.resource_limits.max_analysis_time_ms == 0 {
            return Err(TriggerError::ConfigError(
                "Maximum analysis time must be greater than 0".to_owned(),
            ));
        }

        Ok(())
    }

    /// Checks if two condition types are compatible (same variant, ignoring data).
    fn condition_types_compatible(supported: &ConditionType, requested: &ConditionType) -> bool {
        use std::mem::discriminant;
        discriminant(supported) == discriminant(requested)
    }

    /// Validates a trigger condition against registered collector capabilities.
    pub fn validate_trigger_condition(
        &self,
        condition: &TriggerCondition,
    ) -> Result<(), TriggerError> {
        let capabilities = lock_or_err!(self.collector_capabilities, "collector_capabilities")?;

        let collector_caps = capabilities
            .get(&condition.target_collector)
            .ok_or_else(|| {
                TriggerError::ConfigError(format!(
                    "No capabilities registered for collector: {}",
                    condition.target_collector
                ))
            })?;

        // Validate condition type support (check by discriminant, not exact match)
        let condition_supported = collector_caps.supported_conditions.iter().any(|supported| {
            Self::condition_types_compatible(supported, &condition.condition_type)
        });

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
            let mut conditions = lock_or_err!(self.conditions, "conditions")?;
            conditions.push(condition)
        };

        // Get evaluator lock and register conditions
        let mut evaluator = lock_or_err!(self.sql_evaluator, "sql_evaluator")?;

        // Get conditions for this collector
        let collector_conditions: Vec<_> = {
            let conditions = lock_or_err!(self.conditions, "conditions")?;
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
        let mut evaluator = lock_or_err!(self.sql_evaluator, "sql_evaluator")?;

        let triggers = evaluator.evaluate_sql_triggers(collector_id, process_data)?;

        // Enqueue triggers with priority handling
        let mut queue = lock_or_err!(self.trigger_queue, "trigger_queue")?;

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
    ///
    /// # Errors
    ///
    /// Returns an error if the queue lock is poisoned.
    pub fn dequeue_trigger(&self) -> Result<Option<TriggerRequest>, TriggerError> {
        let mut queue = lock_or_err!(self.trigger_queue, "trigger_queue")?;
        Ok(queue.dequeue())
    }

    /// Checks if the trigger queue is experiencing backpressure.
    pub fn is_backpressure_active(&self) -> bool {
        self.trigger_queue
            .lock()
            .is_ok_and(|queue| queue.is_backpressure_active())
    }

    /// Returns the current queue depth.
    pub fn get_queue_depth(&self) -> usize {
        self.trigger_queue.lock().map_or(0, |queue| queue.len())
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
    /// `HashMap` mapping collector IDs to their capabilities. Returns empty map if lock fails.
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
            .is_ok_and(|tracker| tracker.contains_key(trigger_id))
    }

    /// Evaluates trigger conditions against process event data.
    ///
    /// Returns trigger requests that should be sent to analysis collectors
    /// after applying deduplication and rate limiting.
    pub fn evaluate_triggers(
        &self,
        process_data: &ProcessTriggerData,
    ) -> Result<Vec<TriggerRequest>, TriggerError> {
        let conditions = lock_or_err!(self.conditions, "conditions")?;
        let mut triggers = Vec::with_capacity(conditions.len());

        // Evaluate each registered condition
        for condition in conditions.iter() {
            if Self::evaluate_condition(condition, process_data)? {
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
        condition: &TriggerCondition,
        data: &ProcessTriggerData,
    ) -> Result<bool, TriggerError> {
        match &condition.condition_type {
            ConditionType::ProcessNamePattern(pattern) => Ok(data.name.contains(pattern)),
            ConditionType::ExecutablePathPattern(pattern) => Ok(data
                .executable_path
                .as_ref()
                .is_some_and(|path| path.contains(pattern))),
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
                let cpu_anomaly = data.cpu_usage.is_some_and(|cpu| cpu > *cpu_threshold);
                let memory_anomaly = data.memory_usage.is_some_and(|mem| mem > *memory_threshold);
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
        metadata.insert("condition_id".to_owned(), condition.id.clone());
        metadata.insert("source_pid".to_owned(), data.pid.to_string());
        if let Some(path) = &data.executable_path {
            metadata.insert("executable_path".to_owned(), path.clone());
        }

        // Track trigger metadata if enabled
        if self.config.enable_metadata_tracking {
            let trigger_metadata = TriggerMetadata {
                generated_at: timestamp,
                source_event_type: "process".to_owned(),
                matched_condition: condition.id.clone(),
                evaluation_context: metadata.clone(),
                correlation_id: correlation_id.clone(),
            };

            let mut cache = lock_or_err!(self.metadata_cache, "metadata_cache")?;
            cache.insert(trigger_id.clone(), trigger_metadata);
        }

        let request = TriggerRequest {
            trigger_id,
            target_collector: condition.target_collector.clone(),
            analysis_type: condition.analysis_type.clone(),
            priority: condition.priority.clone(),
            target_pid: Some(data.pid),
            target_path: data.executable_path.clone(),
            correlation_id,
            metadata,
            timestamp,
        };
        request.validate().map_err(TriggerError::ValidationError)?;
        Ok(request)
    }

    /// Checks if a trigger should be deduplicated.
    fn should_deduplicate(&self, trigger: &TriggerRequest) -> Result<bool, TriggerError> {
        let dedup_key = DeduplicationKey {
            collector: trigger.target_collector.clone(),
            analysis_type: trigger.analysis_type.clone(),
            target_pid: trigger.target_pid,
            target_path: trigger.target_path.clone(),
        };

        let mut cache = lock_or_err!(self.deduplication_cache, "deduplication_cache")?;

        let now = SystemTime::now();
        let dedup_window = Duration::from_secs(self.config.deduplication_window_secs);

        // Clean expired entries
        cache.retain(|_, timestamp| {
            now.duration_since(*timestamp).unwrap_or(Duration::MAX) < dedup_window
        });

        // Check if this trigger is a duplicate
        if let Some(last_trigger_time) = cache.get(&dedup_key)
            && now
                .duration_since(*last_trigger_time)
                .unwrap_or(Duration::MAX)
                < Duration::from_secs(self.config.deduplication_window_secs)
        {
            return Ok(true); // Should deduplicate
        }

        // Record this trigger
        cache.insert(dedup_key, now);
        Ok(false) // Should not deduplicate
    }

    /// Checks if a trigger should be rate limited.
    fn should_rate_limit(&self, trigger: &TriggerRequest) -> Result<bool, TriggerError> {
        let mut rate_limits = lock_or_err!(self.rate_limits, "rate_limits")?;

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
    ///     let event_bus = LocalEventBus::new(event_bus_config);
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
    ///         trigger_id: "trigger_123".to_owned(),
    ///         target_collector: "binary-hasher".to_owned(),
    ///         analysis_type: AnalysisType::BinaryHash,
    ///         priority: TriggerPriority::High,
    ///         target_pid: Some(1234),
    ///         target_path: Some("/usr/bin/suspicious".to_owned()),
    ///         correlation_id: "corr_456".to_owned(),
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
                TriggerError::EventBusError("Event bus not configured".to_owned())
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
                TriggerError::EventBusError("Event bus not configured".to_owned())
            })?;

            event_bus
                .publish(
                    collection_event,
                    crate::event_bus::CorrelationMetadata::new(correlation_id.clone())
                        .with_tag("trigger_source".to_owned(), "trigger_manager".to_owned()),
                )
                .await
        };
        // Lock is dropped here

        if let Err(err) = publish_result {
            let mut stats = lock_or_err!(self.emission_stats, "emission_stats")?;
            stats.event_bus_failures += 1;
            return Err(TriggerError::EventBusError(err.to_string()));
        }

        // Track timeout for this trigger request
        self.track_trigger_timeout(&trigger).await?;

        // Update emission statistics in a single lock acquisition
        {
            let mut stats = lock_or_err!(self.emission_stats, "emission_stats")?;

            stats.total_emitted += 1;
            stats.pending_responses += 1;
            stats.successful_emissions += 1;

            // Calculate emission latency
            if let Ok(duration) = start_time.elapsed() {
                let latency_ms = duration.as_millis() as f64;

                // Update running average
                let total_emissions = stats.total_emitted as f64;
                stats.avg_emission_latency_ms = stats
                    .avg_emission_latency_ms
                    .mul_add(total_emissions - 1.0, latency_ms)
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
    #[allow(clippy::unused_async)]
    pub async fn validate_trigger_request(
        &self,
        trigger: &TriggerRequest,
    ) -> Result<(), TriggerError> {
        let capabilities = lock_or_err!(self.collector_capabilities, "collector_capabilities")?;

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
        if trigger
            .target_path
            .as_ref()
            .map_or(0, std::string::String::len)
            > 4096
        {
            return Err(TriggerError::ValidationError(
                "Target path exceeds maximum length (4096 characters)".to_owned(),
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
                "Trigger metadata exceeds maximum size (64KB)".to_owned(),
            ));
        }

        Ok(())
    }

    /// Tracks timeout for a trigger request to enable timeout handling.
    ///
    /// This method adds the trigger to the timeout tracker, allowing the system
    /// to detect and handle triggers that don't receive responses within the
    /// configured timeout period.
    #[allow(clippy::unused_async)]
    pub async fn track_trigger_timeout(
        &self,
        trigger: &TriggerRequest,
    ) -> Result<(), TriggerError> {
        let timeout_duration = Duration::from_secs(self.config.default_timeout_secs);

        let timeout_info = TriggerTimeout {
            target_collector: trigger.target_collector.clone(),
            emitted_at: SystemTime::now(),
            timeout_duration,
            correlation_id: trigger.correlation_id.clone(),
        };

        let mut tracker = lock_or_err!(self.timeout_tracker, "timeout_tracker")?;

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
    #[allow(clippy::unused_async)]
    pub async fn handle_trigger_timeouts(&self) -> Result<Vec<String>, TriggerError> {
        let mut timed_out_triggers = Vec::new();
        let now = SystemTime::now();

        {
            let mut tracker = lock_or_err!(self.timeout_tracker, "timeout_tracker")?;

            let mut expired_triggers = Vec::new();

            // Find expired triggers
            for (trigger_id, timeout_info) in tracker.iter() {
                if let Ok(elapsed) = now.duration_since(timeout_info.emitted_at)
                    && elapsed > timeout_info.timeout_duration
                {
                    expired_triggers.push(trigger_id.clone());
                    timed_out_triggers.push(trigger_id.clone());

                    debug!(
                        trigger_id = trigger_id.as_str(),
                        elapsed_ms = elapsed.as_millis(),
                        timeout_ms = timeout_info.timeout_duration.as_millis(),
                        "Trigger timed out"
                    );
                }
            }

            // Remove expired triggers
            for trigger_id in &expired_triggers {
                tracker.remove(trigger_id);
            }
        }

        // Update statistics
        if !timed_out_triggers.is_empty() {
            let mut stats = lock_or_err!(self.emission_stats, "emission_stats")?;

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
    #[allow(clippy::unused_async)]
    pub async fn complete_trigger_request(&self, trigger_id: &str) -> Result<(), TriggerError> {
        let mut tracker = lock_or_err!(self.timeout_tracker, "timeout_tracker")?;

        if tracker.remove(trigger_id).is_some() {
            // Update statistics
            let mut stats = lock_or_err!(self.emission_stats, "emission_stats")?;

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
        let pending_count = self.pending_count.lock().map_or(0, |count| *count);

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
                .map_or(0, |cache| cache.len());
            let rate_limit_states = self.rate_limits.lock().map_or(0, |limits| limits.len());
            let registered_capabilities = self
                .collector_capabilities
                .lock()
                .map_or(0, |caps| caps.len());
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

        let conditions_len = lock_or_err!(self.conditions, "conditions")?.len();

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
        {
            let now = SystemTime::now();
            let dedup_window = Duration::from_secs(self.config.deduplication_window_secs);
            let mut cache = lock_or_err!(self.deduplication_cache, "deduplication_cache")?;
            cache.retain(|_, timestamp| {
                now.duration_since(*timestamp).unwrap_or(Duration::MAX) < dedup_window
            })
        }

        // Clean metadata cache with incremental eviction to reduce lock contention
        let max_pending_triggers = self.config.max_pending_triggers;
        let maybe_evict = {
            let cache = lock_or_err!(self.metadata_cache, "metadata_cache")?;

            // Check if the cache size exceeds the maximum allowed
            (cache.len() > max_pending_triggers).then(|| {
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

                (entries_to_sort, to_remove_count)
            })
        };

        // Reacquire the lock to remove elements if eviction is needed
        if let Some((entries_to_sort, to_remove_count)) = maybe_evict {
            let mut cache = lock_or_err!(self.metadata_cache, "metadata_cache")?;
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

        Ok(())
    }
}

/// Timeout tracking for trigger requests.
#[derive(Debug, Clone)]
struct TriggerTimeout {
    /// Target collector name
    #[allow(dead_code)]
    target_collector: String,

    /// Emission timestamp
    emitted_at: SystemTime,

    /// Timeout duration
    timeout_duration: Duration,

    /// Correlation ID for forensic tracking
    #[allow(dead_code)]
    correlation_id: String,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]
mod tests {
    use super::*;
    use crate::event::{AnalysisType, TriggerPriority};
    use crate::trigger::queue::PriorityTriggerQueue;
    use crate::trigger::types::{
        ConditionType, DeduplicationKey, ProcessTriggerData, TriggerCapabilities, TriggerCondition,
        TriggerResourceLimits,
    };

    fn create_test_process_data() -> ProcessTriggerData {
        ProcessTriggerData {
            pid: 1234,
            name: "test_process".to_owned(), // This contains "test" which should match the pattern
            executable_path: Some("/usr/bin/test".to_owned()),
            file_exists: true,
            cpu_usage: Some(5.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_owned()),
        }
    }

    fn create_test_condition() -> TriggerCondition {
        TriggerCondition {
            id: "test_condition".to_owned(),
            description: "Test condition".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Normal,
            target_collector: "binary-hasher".to_owned(),
            condition_type: ConditionType::ProcessNamePattern("test".to_owned()),
        }
    }

    fn create_test_capabilities() -> TriggerCapabilities {
        TriggerCapabilities {
            collector_id: "binary-hasher".to_owned(),
            supported_conditions: vec![
                ConditionType::ProcessNamePattern("test".to_owned()), // Match the test condition
                ConditionType::ExecutablePathPattern("".to_owned()),
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
            id: "missing_exe".to_owned(),
            description: "Missing executable".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_collector: "binary-hasher".to_owned(),
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
        capabilities.collector_id = "behavior-analyzer".to_owned();
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
            id: "resource_anomaly".to_owned(),
            description: "Resource anomaly".to_owned(),
            analysis_type: AnalysisType::BehavioralAnalysis,
            priority: TriggerPriority::High,
            target_collector: "behavior-analyzer".to_owned(),
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
            collector: "binary-hasher".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_owned()),
        };

        let key2 = DeduplicationKey {
            collector: "binary-hasher".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_owned()),
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
            id: "invalid_condition".to_owned(),
            description: "Invalid condition".to_owned(),
            analysis_type: AnalysisType::YaraScan, // Not supported by test capabilities
            priority: TriggerPriority::Normal,
            target_collector: "binary-hasher".to_owned(),
            condition_type: ConditionType::ProcessNamePattern("test".to_owned()),
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
            trigger_id: "high".to_owned(),
            target_collector: "test".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "corr1".to_owned(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        };

        let low_priority_trigger = TriggerRequest {
            trigger_id: "low".to_owned(),
            target_collector: "test".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Low,
            target_pid: Some(5678),
            target_path: None,
            correlation_id: "corr2".to_owned(),
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
                target_collector: "test".to_owned(),
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
            trigger_id: "low_priority".to_owned(),
            target_collector: "test".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Low,
            target_pid: Some(9999),
            target_path: None,
            correlation_id: "corr_low".to_owned(),
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
            trigger_id: "high_priority".to_owned(),
            target_collector: "test".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Critical,
            target_pid: Some(8888),
            target_path: None,
            correlation_id: "corr_high".to_owned(),
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
        invalid_trigger.target_collector = "unknown-collector".to_owned();
        let result = manager.validate_trigger_request(&invalid_trigger).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::ValidationError(_)
        ));

        // Test validation with unsupported analysis type
        let mut invalid_trigger = create_test_trigger_request();
        invalid_trigger.analysis_type = AnalysisType::Custom("unsupported".to_owned());
        let result = manager.validate_trigger_request(&invalid_trigger).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TriggerError::ValidationError(_)
        ));

        // Test validation with oversized metadata
        let mut invalid_trigger = create_test_trigger_request();
        invalid_trigger.metadata.insert(
            "large_key".to_owned(),
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
                emitted_at: SystemTime::now() - Duration::from_mins(1),
                timeout_duration: Duration::from_secs(30), // 30 second timeout
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
            trigger_id: "invalid_trigger".to_owned(),
            target_collector: "unknown-collector".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: None,
            correlation_id: "test_correlation".to_owned(),
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
            trigger_id: "test_trigger_123".to_owned(),
            target_collector: "binary-hasher".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::High,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/test".to_owned()),
            correlation_id: "test_correlation_456".to_owned(),
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("test_key".to_owned(), "test_value".to_owned());
                metadata
            },
            timestamp: SystemTime::now(),
        }
    }
}
