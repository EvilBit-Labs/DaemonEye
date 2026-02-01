//! Procmond-specific Monitor Collector implementation.
//!
//! This module provides a concrete implementation of the Monitor Collector framework
//! specifically for procmond, integrating process lifecycle tracking with the
//! collector-core `EventSource` trait.

use crate::{
    event_bus_connector::{EventBusConnector, ProcessEventType},
    lifecycle::{LifecycleTrackingConfig, ProcessLifecycleTracker},
    process_collector::{ProcessCollectionConfig, ProcessCollector, SysinfoProcessCollector},
};
use anyhow::Context;
use async_trait::async_trait;
use collector_core::{
    AnalysisChainCoordinator, CollectionEvent, EventSource,
    MonitorCollector as MonitorCollectorTrait, MonitorCollectorConfig, MonitorCollectorStats,
    MonitorCollectorStatsSnapshot, SourceCaps, TriggerManager,
};
use daemoneye_lib::{storage, telemetry::PerformanceTimer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, Semaphore, mpsc},
    time::{interval, timeout},
};
use tracing::{debug, error, info, instrument, warn};

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Messages for the ProcmondMonitorCollector actor.
///
/// The actor processes these messages sequentially to maintain consistent state
/// without complex locking. Request/response patterns use oneshot channels.
#[derive(Debug)]
#[non_exhaustive]
pub enum ActorMessage {
    /// Request health check data from the actor.
    HealthCheck {
        /// Channel to send the health check response.
        respond_to: tokio::sync::oneshot::Sender<HealthCheckData>,
    },
    /// Update the collector configuration at the next cycle boundary.
    UpdateConfig {
        /// New configuration to apply (boxed to reduce enum size).
        config: Box<ProcmondMonitorConfig>,
        /// Channel to send the result.
        respond_to: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
    },
    /// Request graceful shutdown, completing the current cycle.
    GracefulShutdown {
        /// Channel to signal shutdown completion.
        respond_to: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
    },
    /// Signal from agent to begin monitoring after startup coordination.
    BeginMonitoring,
    /// Adjust collection interval due to backpressure.
    AdjustInterval {
        /// New collection interval to use.
        new_interval: Duration,
    },
}

/// Current state of the collector actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CollectorState {
    /// Waiting for BeginMonitoring command from agent.
    WaitingForAgent,
    /// Actively collecting process data.
    Running,
    /// Graceful shutdown in progress.
    ShuttingDown,
    /// Collector has stopped.
    Stopped,
}

impl std::fmt::Display for CollectorState {
    #[allow(clippy::pattern_type_mismatch)] // Match ergonomics for enum Display
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaitingForAgent => write!(f, "waiting_for_agent"),
            Self::Running => write!(f, "running"),
            Self::ShuttingDown => write!(f, "shutting_down"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Health check response data from the collector actor.
#[derive(Debug, Clone)]
pub struct HealthCheckData {
    /// Current state of the collector.
    pub state: CollectorState,
    /// Current collection interval.
    pub collection_interval: Duration,
    /// Original collection interval (before any backpressure adjustments).
    pub original_interval: Duration,
    /// Whether connected to the event bus broker.
    pub event_bus_connected: bool,
    /// Current buffer level percentage (0-100) if available.
    pub buffer_level_percent: Option<u8>,
    /// Timestamp of last successful collection.
    pub last_collection: Option<Instant>,
    /// Number of collection cycles completed.
    pub collection_cycles: u64,
    /// Number of lifecycle events detected.
    pub lifecycle_events: u64,
    /// Number of collection errors.
    pub collection_errors: u64,
    /// Number of backpressure events.
    pub backpressure_events: u64,
}

/// Channel capacity for actor messages.
pub const ACTOR_CHANNEL_CAPACITY: usize = 100;

/// Error returned when the actor channel is full or closed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ActorError {
    /// The actor's message channel is full.
    #[error("Actor message channel is full (capacity: {capacity})")]
    ChannelFull { capacity: usize },
    /// The actor's message channel is closed.
    #[error("Actor message channel is closed")]
    ChannelClosed,
    /// The response channel was dropped before receiving a response.
    #[error("Response channel dropped")]
    ResponseDropped,
    /// The actor returned an error.
    #[error("Actor error: {0}")]
    ActorError(#[from] anyhow::Error),
}

/// Handle for sending messages to the ProcmondMonitorCollector actor.
///
/// This handle is cloneable and can be shared across tasks to communicate
/// with the actor. It provides typed methods for each message type.
#[derive(Clone)]
pub struct ActorHandle {
    sender: mpsc::Sender<ActorMessage>,
}

impl ActorHandle {
    /// Creates a new actor handle from an mpsc sender.
    pub const fn new(sender: mpsc::Sender<ActorMessage>) -> Self {
        Self { sender }
    }

    /// Requests health check data from the actor.
    ///
    /// Returns detailed health information including collector state,
    /// event bus connectivity, and statistics.
    pub async fn health_check(&self) -> Result<HealthCheckData, ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(ActorMessage::HealthCheck { respond_to: tx })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ActorError::ChannelFull {
                    capacity: ACTOR_CHANNEL_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
            })?;
        rx.await.map_err(|_recv_err| ActorError::ResponseDropped)
    }

    /// Updates the collector configuration.
    ///
    /// The configuration is applied at the start of the next collection cycle
    /// to ensure atomic configuration changes.
    pub async fn update_config(&self, config: ProcmondMonitorConfig) -> Result<(), ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(ActorMessage::UpdateConfig {
                config: Box::new(config),
                respond_to: tx,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ActorError::ChannelFull {
                    capacity: ACTOR_CHANNEL_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
            })?;
        rx.await
            .map_err(|_recv_err| ActorError::ResponseDropped)?
            .map_err(ActorError::ActorError)
    }

    /// Requests graceful shutdown of the collector.
    ///
    /// The collector will complete its current collection cycle before
    /// shutting down. Returns when shutdown is complete.
    pub async fn graceful_shutdown(&self) -> Result<(), ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .try_send(ActorMessage::GracefulShutdown { respond_to: tx })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ActorError::ChannelFull {
                    capacity: ACTOR_CHANNEL_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
            })?;
        rx.await
            .map_err(|_recv_err| ActorError::ResponseDropped)?
            .map_err(ActorError::ActorError)
    }

    /// Signals the collector to begin monitoring.
    ///
    /// This is called by the agent after startup coordination is complete.
    /// The collector transitions from WaitingForAgent to Running state.
    pub fn begin_monitoring(&self) -> Result<(), ActorError> {
        self.sender
            .try_send(ActorMessage::BeginMonitoring)
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ActorError::ChannelFull {
                    capacity: ACTOR_CHANNEL_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
            })
    }

    /// Adjusts the collection interval due to backpressure.
    ///
    /// Called by the EventBusConnector when backpressure is detected or released.
    pub fn adjust_interval(&self, new_interval: Duration) -> Result<(), ActorError> {
        self.sender
            .try_send(ActorMessage::AdjustInterval { new_interval })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ActorError::ChannelFull {
                    capacity: ACTOR_CHANNEL_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
            })
    }

    /// Checks if the actor channel is closed.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl std::fmt::Debug for ActorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorHandle")
            .field("closed", &self.sender.is_closed())
            .finish()
    }
}

// ============================================================================
// Configuration
// ============================================================================

/// Procmond-specific Monitor Collector configuration.
///
/// This extends the base `MonitorCollectorConfig` with procmond-specific
/// configuration for process collection and lifecycle tracking.
///
/// # Hot-Reload Support
///
/// Configuration can be updated at runtime via `ActorHandle::update_config()`.
/// Updates are applied atomically at collection cycle boundaries.
///
/// ## Hot-Reloadable Settings
///
/// These settings can be changed without restarting procmond:
/// - `base_config.collection_interval` - Collection frequency
/// - `base_config.max_events_in_flight` - Backpressure limit (note: semaphore not resized)
/// - `lifecycle_config.start_threshold` - Process start detection threshold
/// - `lifecycle_config.stop_threshold` - Process stop detection threshold
/// - `lifecycle_config.modification_threshold` - Process modification detection threshold
///
/// ## Requires Restart
///
/// These settings require procmond restart to take effect:
/// - `process_config.excluded_pids` - Affects collector initialization
/// - `base_config.enable_event_driven` - Requires recreating event bus
/// - `process_config.collection_timeout` - Affects collector initialization
#[derive(Debug, Clone, Default)]
pub struct ProcmondMonitorConfig {
    /// Base monitor collector configuration (collection_interval is hot-reloadable)
    pub base_config: MonitorCollectorConfig,
    /// Process collection configuration (mostly requires restart)
    pub process_config: ProcessCollectionConfig,
    /// Lifecycle tracking configuration (thresholds are hot-reloadable)
    pub lifecycle_config: LifecycleTrackingConfig,
}

impl ProcmondMonitorConfig {
    /// Validates the configuration parameters.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.base_config.validate()
    }
}

/// Procmond Monitor Collector implementation using actor pattern.
///
/// This collector integrates process lifecycle tracking with the collector-core
/// framework, providing event-driven process monitoring capabilities.
///
/// # Actor Pattern
///
/// The collector runs as an actor in a dedicated task, processing messages
/// sequentially to maintain consistent state. Messages are received via a
/// bounded mpsc channel (capacity: 100) and processed one at a time.
///
/// # Startup Coordination
///
/// The collector starts in `WaitingForAgent` state and waits for a
/// `BeginMonitoring` message from the agent before starting collection.
/// This ensures the agent has completed loading state before monitoring begins.
///
/// # Backpressure Handling
///
/// The actor receives `AdjustInterval` messages from the EventBusConnector
/// when backpressure is detected. The collection interval increases by 1.5x
/// during backpressure and is restored when backpressure is released.
#[allow(dead_code)]
pub struct ProcmondMonitorCollector {
    /// Current configuration (may be updated at cycle boundaries)
    config: ProcmondMonitorConfig,
    /// Pending configuration update to apply at next cycle boundary
    pending_config: Option<ProcmondMonitorConfig>,
    /// Database manager for audit logging
    database: Arc<Mutex<storage::DatabaseManager>>,
    /// Process collector implementation
    process_collector: Box<dyn ProcessCollector>,
    /// Process lifecycle tracker
    lifecycle_tracker: Arc<Mutex<ProcessLifecycleTracker>>,
    /// Trigger manager for analysis coordination
    trigger_manager: Arc<TriggerManager>,
    /// Analysis chain coordinator
    analysis_coordinator: Arc<AnalysisChainCoordinator>,
    /// Runtime statistics
    stats: Arc<MonitorCollectorStats>,
    /// Backpressure semaphore
    backpressure_semaphore: Arc<Semaphore>,
    /// Consecutive backpressure timeout counter for circuit breaker
    consecutive_backpressure_timeouts: Arc<std::sync::atomic::AtomicUsize>,
    /// Circuit breaker cooldown timestamp
    circuit_breaker_until: Arc<std::sync::Mutex<Option<Instant>>>,

    // Actor-specific fields
    /// Actor message receiver
    message_receiver: mpsc::Receiver<ActorMessage>,
    /// Current collector state
    state: CollectorState,
    /// Current collection interval (may be adjusted due to backpressure)
    current_interval: Duration,
    /// Original collection interval (before backpressure adjustments)
    original_interval: Duration,
    /// Timestamp of last successful collection
    last_collection: Option<Instant>,
    /// Event bus connection status
    event_bus_connected: bool,
    /// Buffer level percentage (0-100) from EventBusConnector
    buffer_level_percent: Option<u8>,
    /// Pending graceful shutdown response channel
    pending_shutdown_response: Option<tokio::sync::oneshot::Sender<anyhow::Result<()>>>,

    // Event Bus Integration
    /// EventBusConnector for publishing events to the broker with WAL integration.
    event_bus_connector: Option<EventBusConnector>,
}

impl ProcmondMonitorCollector {
    /// Creates a new Procmond Monitor Collector as an actor.
    ///
    /// Returns both the collector and an `ActorHandle` for sending messages.
    /// The collector should be spawned in a dedicated task using the `run()` method.
    ///
    /// # Arguments
    ///
    /// * `database` - Database manager for audit logging
    /// * `config` - Collector configuration
    /// * `message_receiver` - Receiver end of the actor message channel
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    /// let handle = ActorHandle::new(tx);
    /// let collector = ProcmondMonitorCollector::new(database, config, rx)?;
    /// tokio::spawn(async move { collector.run(event_tx).await });
    /// ```
    pub fn new(
        database: Arc<Mutex<storage::DatabaseManager>>,
        config: ProcmondMonitorConfig,
        message_receiver: mpsc::Receiver<ActorMessage>,
    ) -> anyhow::Result<Self> {
        // Validate configuration first
        config
            .validate()
            .with_context(|| "Procmond Monitor Collector configuration validation failed")?;

        let collection_interval = config.base_config.collection_interval;

        info!(
            collection_interval_secs = collection_interval.as_secs(),
            max_events_in_flight = config.base_config.max_events_in_flight,
            event_driven = config.base_config.enable_event_driven,
            "Creating Procmond Monitor Collector (actor mode)"
        );

        // Create process collector
        let process_collector =
            Box::new(SysinfoProcessCollector::new(config.process_config.clone()));

        // Create lifecycle tracker
        let lifecycle_tracker = Arc::new(Mutex::new(ProcessLifecycleTracker::new(
            config.lifecycle_config.clone(),
        )));

        // Create trigger manager
        let trigger_manager = Arc::new(TriggerManager::new(
            config.base_config.trigger_config.clone(),
        ));

        // Create analysis chain coordinator (already returns Arc<Self>)
        let analysis_coordinator =
            AnalysisChainCoordinator::new(config.base_config.analysis_config.clone());

        // Create backpressure semaphore with validated capacity
        let backpressure_semaphore =
            Arc::new(Semaphore::new(config.base_config.max_events_in_flight));

        Ok(Self {
            config,
            pending_config: None,
            database,
            process_collector,
            lifecycle_tracker,
            trigger_manager,
            analysis_coordinator,
            stats: Arc::new(MonitorCollectorStats::default()),
            backpressure_semaphore,
            consecutive_backpressure_timeouts: Arc::new(AtomicUsize::new(0)),
            circuit_breaker_until: Arc::new(std::sync::Mutex::new(None)),
            // Actor-specific fields
            message_receiver,
            state: CollectorState::WaitingForAgent,
            current_interval: collection_interval,
            original_interval: collection_interval,
            last_collection: None,
            event_bus_connected: false,
            buffer_level_percent: None,
            pending_shutdown_response: None,
            // Event Bus Integration
            event_bus_connector: None,
        })
    }

    /// Creates a new actor channel and handle.
    ///
    /// This is a convenience method for creating the channel infrastructure.
    /// The returned handle should be used to send messages to the actor.
    pub fn create_channel() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
        let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
        (ActorHandle::new(tx), rx)
    }

    /// Sets the event bus connection status.
    ///
    /// Called by main.rs after EventBusConnector connects to the broker.
    pub const fn set_event_bus_connected(&mut self, connected: bool) {
        self.event_bus_connected = connected;
    }

    /// Sets the current buffer level percentage.
    ///
    /// Called when receiving buffer level updates from EventBusConnector.
    pub const fn set_buffer_level(&mut self, level_percent: u8) {
        self.buffer_level_percent = Some(level_percent);
    }

    /// Sets the EventBusConnector for publishing events to the broker.
    ///
    /// This should be called after constructing the collector and before
    /// calling `run()`. The connector should already be connected or will
    /// connect during the run loop.
    pub fn set_event_bus_connector(&mut self, connector: EventBusConnector) {
        self.event_bus_connected = connector.is_connected();
        self.buffer_level_percent = Some(connector.buffer_usage_percent());
        self.event_bus_connector = Some(connector);
    }

    /// Takes the EventBusConnector out of the collector for shutdown.
    ///
    /// Returns `None` if no connector was set.
    #[allow(clippy::missing_const_for_fn)] // Option::take() is not const
    pub fn take_event_bus_connector(&mut self) -> Option<EventBusConnector> {
        self.event_bus_connector.take()
    }

    /// Spawns a backpressure monitoring task that adjusts collection interval.
    ///
    /// This function should be called from main.rs after creating the actor.
    /// It spawns a background task that:
    /// 1. Receives `BackpressureSignal` from the EventBusConnector
    /// 2. On `Activated`: increases interval by 1.5x via `AdjustInterval` message
    /// 3. On `Released`: restores original interval via `AdjustInterval` message
    ///
    /// # Arguments
    ///
    /// * `handle` - The actor handle for sending messages
    /// * `backpressure_rx` - The receiver from `EventBusConnector::take_backpressure_receiver()`
    /// * `original_interval` - The original collection interval (before backpressure)
    ///
    /// # Returns
    ///
    /// A `JoinHandle` for the spawned task
    pub fn spawn_backpressure_monitor(
        handle: ActorHandle,
        mut backpressure_rx: mpsc::Receiver<crate::event_bus_connector::BackpressureSignal>,
        original_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                original_interval_ms = original_interval.as_millis(),
                "Starting backpressure monitor task"
            );

            while let Some(signal) = backpressure_rx.recv().await {
                match signal {
                    crate::event_bus_connector::BackpressureSignal::Activated => {
                        // Increase interval by 1.5x (50% slower collection), clamped to 1 hour max
                        const MAX_INTERVAL_MS: u128 = 3_600_000; // 1 hour
                        let scaled_ms = original_interval
                            .as_millis()
                            .saturating_mul(3)
                            .saturating_div(2);
                        let clamped_ms = if scaled_ms > MAX_INTERVAL_MS {
                            warn!(
                                original_interval_ms = original_interval.as_millis(),
                                scaled_interval_ms = scaled_ms,
                                max_interval_ms = MAX_INTERVAL_MS,
                                "Backpressure-adjusted interval exceeds maximum; clamping to 1 hour"
                            );
                            MAX_INTERVAL_MS
                        } else {
                            scaled_ms
                        };
                        #[allow(clippy::as_conversions)]
                        // Safe: clamped_ms <= 3_600_000 fits in u64
                        let new_interval = Duration::from_millis(clamped_ms as u64);
                        info!(
                            original_interval_ms = original_interval.as_millis(),
                            new_interval_ms = new_interval.as_millis(),
                            "Backpressure activated - increasing collection interval by 1.5x"
                        );
                        if let Err(e) = handle.adjust_interval(new_interval) {
                            warn!(error = %e, "Failed to send AdjustInterval message");
                        }
                    }
                    crate::event_bus_connector::BackpressureSignal::Released => {
                        // Restore original interval
                        info!(
                            original_interval_ms = original_interval.as_millis(),
                            "Backpressure released - restoring original collection interval"
                        );
                        if let Err(e) = handle.adjust_interval(original_interval) {
                            warn!(error = %e, "Failed to send AdjustInterval message");
                        }
                    }
                }
            }

            info!("Backpressure monitor task exiting (channel closed)");
        })
    }

    /// Runs the actor message processing loop.
    ///
    /// This method should be spawned in a dedicated task. It processes messages
    /// sequentially and runs the collection loop when in Running state.
    ///
    /// # Startup Coordination
    ///
    /// The actor starts in `WaitingForAgent` state. It waits for a `BeginMonitoring`
    /// message before starting the collection loop. This ensures the daemoneye-agent
    /// has completed loading state (privileges dropped, all collectors ready).
    ///
    /// # Arguments
    ///
    /// * `event_tx` - Channel for sending collection events to downstream processors
    #[instrument(skip(self, event_tx), fields(source = "procmond-monitor-collector"))]
    pub async fn run(mut self, event_tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        const MAX_CONSECUTIVE_FAILURES: u32 = 5;

        info!(
            state = %self.state,
            collection_interval_secs = self.current_interval.as_secs(),
            "Starting Procmond Monitor Collector actor"
        );

        let mut consecutive_failures = 0_u32;
        let mut collection_interval = interval(self.current_interval);
        // Skip first tick to avoid immediate collection
        collection_interval.tick().await;

        loop {
            // Check for pending config update at cycle boundary
            if let Some(new_config) = self.pending_config.take() {
                self.apply_config_update(new_config);
                // Update interval if changed
                let new_interval = self.config.base_config.collection_interval;
                if new_interval != self.original_interval {
                    self.original_interval = new_interval;
                    self.current_interval = new_interval;
                    collection_interval = interval(self.current_interval);
                    collection_interval.tick().await; // Reset interval
                    info!(
                        new_interval_secs = new_interval.as_secs(),
                        "Collection interval updated from config"
                    );
                }
            }

            tokio::select! {
                biased;

                // Process incoming messages (highest priority)
                msg = self.message_receiver.recv() => {
                    if let Some(message) = msg {
                        let should_exit = self.handle_message(message);
                        if should_exit {
                            info!("Actor received shutdown signal, exiting");
                            break;
                        }
                    } else {
                        info!("Actor message channel closed, exiting");
                        break;
                    }
                }

                // Collection tick (only when in Running state)
                _ = collection_interval.tick(), if self.state == CollectorState::Running => {
                    match self.collect_and_analyze_internal(&event_tx).await {
                        Ok(()) => {
                            consecutive_failures = 0;
                            self.last_collection = Some(Instant::now());
                        }
                        Err(e) => {
                            error!(error = %e, "Collection cycle failed");
                            consecutive_failures = consecutive_failures.saturating_add(1);

                            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                error!(
                                    consecutive_failures = consecutive_failures,
                                    "Too many consecutive failures, stopping collector"
                                );
                                self.state = CollectorState::Stopped;

                                // If there's a pending shutdown response, send error
                                // Ignore send result - receiver may have been dropped
                                if let Some(respond_to) = self.pending_shutdown_response.take() {
                                    drop(respond_to.send(Err(anyhow::anyhow!(
                                        "Collector stopped due to {consecutive_failures} consecutive failures"
                                    ))));
                                }
                                break;
                            }

                            // Exponential backoff for failures
                            let backoff_duration = Duration::from_secs(2_u64.pow(consecutive_failures.min(6)));
                            warn!(
                                backoff_seconds = backoff_duration.as_secs(),
                                "Applying backoff after collection failure"
                            );
                            tokio::time::sleep(backoff_duration).await;
                        }
                    }
                }
            }
        }

        // Final shutdown
        self.state = CollectorState::Stopped;
        info!("Procmond Monitor Collector actor stopped");

        // Send shutdown completion if there's a pending response
        // Ignore send result - receiver may have been dropped
        if let Some(respond_to) = self.pending_shutdown_response.take() {
            drop(respond_to.send(Ok(())));
        }

        Ok(())
    }

    /// Handles an incoming actor message.
    ///
    /// Returns `true` if the actor should exit.
    fn handle_message(&mut self, message: ActorMessage) -> bool {
        match message {
            ActorMessage::HealthCheck { respond_to } => {
                let health_data = self.build_health_data();
                // Ignore send result - receiver may have been dropped
                drop(respond_to.send(health_data));
                false
            }

            ActorMessage::UpdateConfig { config, respond_to } => {
                // Queue config for application at next cycle boundary
                // Ignore send result - receiver may have been dropped
                if let Err(e) = config.validate() {
                    drop(
                        respond_to
                            .send(Err(anyhow::anyhow!("Configuration validation failed: {e}"))),
                    );
                } else {
                    self.pending_config = Some(*config);
                    drop(respond_to.send(Ok(())));
                    info!("Configuration update queued for next cycle boundary");
                }
                false
            }

            ActorMessage::GracefulShutdown { respond_to } => {
                info!("Graceful shutdown requested");
                self.state = CollectorState::ShuttingDown;
                self.pending_shutdown_response = Some(respond_to);
                true // Signal to exit the loop
            }

            ActorMessage::BeginMonitoring => {
                if self.state == CollectorState::WaitingForAgent {
                    info!("Received BeginMonitoring command, starting collection");
                    self.state = CollectorState::Running;
                } else {
                    warn!(
                        current_state = %self.state,
                        "Received BeginMonitoring but not in WaitingForAgent state"
                    );
                }
                false
            }

            ActorMessage::AdjustInterval { new_interval } => {
                let old_interval = self.current_interval;
                self.current_interval = new_interval;
                info!(
                    old_interval_ms = old_interval.as_millis(),
                    new_interval_ms = new_interval.as_millis(),
                    is_backpressure = new_interval > self.original_interval,
                    "Collection interval adjusted"
                );
                false
            }
        }
    }

    /// Builds health check response data.
    fn build_health_data(&self) -> HealthCheckData {
        HealthCheckData {
            state: self.state,
            collection_interval: self.current_interval,
            original_interval: self.original_interval,
            event_bus_connected: self.event_bus_connected,
            buffer_level_percent: self.buffer_level_percent,
            last_collection: self.last_collection,
            collection_cycles: self.stats.collection_cycles.load(Ordering::Relaxed),
            lifecycle_events: self.stats.lifecycle_events.load(Ordering::Relaxed),
            collection_errors: self.stats.collection_errors.load(Ordering::Relaxed),
            backpressure_events: self.stats.backpressure_events.load(Ordering::Relaxed),
        }
    }

    /// Applies a configuration update.
    fn apply_config_update(&mut self, new_config: ProcmondMonitorConfig) {
        // Hot-reloadable settings:
        // - collection_interval
        // - lifecycle_config thresholds
        //
        // Requires restart (changes have no effect until restart):
        // - max_events_in_flight (semaphore capacity cannot be resized at runtime)
        // - process_config.excluded_pids (affects collector initialization)
        // - enable_event_driven (requires recreating event bus)

        let old_max_in_flight = self.config.base_config.max_events_in_flight;
        let new_max_in_flight = new_config.base_config.max_events_in_flight;

        info!(
            old_interval_secs = self.config.base_config.collection_interval.as_secs(),
            new_interval_secs = new_config.base_config.collection_interval.as_secs(),
            "Applying configuration update at cycle boundary"
        );

        // Warn if max_events_in_flight changed (not hot-reloadable)
        if old_max_in_flight != new_max_in_flight {
            warn!(
                old_max_events_in_flight = old_max_in_flight,
                requested_max_events_in_flight = new_max_in_flight,
                "max_events_in_flight is not hot-reloadable; \
                 semaphore capacity will remain unchanged until restart"
            );
        }

        // Update config
        self.config = new_config;

        debug!("Configuration update applied successfully");
    }

    /// Internal collection and analysis method used by the actor loop.
    ///
    /// This method:
    /// 1. Collects process data from the system
    /// 2. Performs lifecycle analysis to detect changes
    /// 3. Publishes events via EventBusConnector (if configured) to `events.process.*` topics
    /// 4. Sends events to the downstream collector-core channel
    #[instrument(skip(self, tx), fields(source = "procmond-monitor-collector"))]
    async fn collect_and_analyze_internal(
        &mut self,
        tx: &mpsc::Sender<CollectionEvent>,
    ) -> anyhow::Result<()> {
        let timer = PerformanceTimer::start("procmond_monitor_collection".to_owned());
        let collection_start = Instant::now();

        // Check state before starting
        if self.state != CollectorState::Running {
            debug!(state = %self.state, "Skipping collection, not in Running state");
            return Ok(());
        }

        // Collect process data with timeout
        let collection_result = timeout(
            Duration::from_secs(30),
            self.process_collector.collect_processes(),
        )
        .await;

        let (process_events, _collection_stats) = match collection_result {
            Ok(Ok((events, stats))) => (events, stats),
            Ok(Err(e)) => {
                error!(error = %e, "Process collection failed");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(e).with_context(|| "Process collection failed");
            }
            Err(_) => {
                error!("Process collection timed out");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(anyhow::anyhow!("Process collection timeout"));
            }
        };

        // Perform lifecycle analysis to detect process starts, stops, and modifications
        let lifecycle_events = {
            let mut tracker = self.lifecycle_tracker.lock().await;
            match tracker.update_and_detect_changes(process_events.clone()) {
                Ok(events) => events,
                Err(e) => {
                    error!(error = %e, "Lifecycle tracking failed");
                    self.stats.analysis_errors.fetch_add(1, Ordering::Relaxed);
                    Vec::new()
                }
            }
        };

        // Update statistics
        self.stats.collection_cycles.fetch_add(1, Ordering::Relaxed);
        #[allow(clippy::as_conversions)] // Safe: usize to u64 for counter
        let event_count = lifecycle_events.len() as u64;
        self.stats
            .lifecycle_events
            .fetch_add(event_count, Ordering::Relaxed);

        // Publish process events via EventBusConnector if configured
        // Events go to events.process.start, events.process.stop, or events.process.modify
        if let Some(ref mut connector) = self.event_bus_connector {
            // Update connection status
            self.event_bus_connected = connector.is_connected();
            self.buffer_level_percent = Some(connector.buffer_usage_percent());

            for process_event in &process_events {
                // Determine event type from lifecycle analysis
                // Default to Start for now; lifecycle events will refine this
                let event_type = ProcessEventType::Start;

                match connector.publish(process_event.clone(), event_type).await {
                    Ok(sequence) => {
                        debug!(
                            pid = process_event.pid,
                            sequence = sequence,
                            "Published process event to EventBus"
                        );
                    }
                    Err(e) => {
                        warn!(
                            pid = process_event.pid,
                            error = %e,
                            "Failed to publish to EventBus (event buffered)"
                        );
                    }
                }
            }
        }

        // Send process events to collector-core channel with backpressure handling
        for process_event in process_events {
            if self.state != CollectorState::Running {
                debug!("State changed during event emission, stopping");
                break;
            }

            if let Err(e) = self
                .send_event_with_backpressure_internal(tx, CollectionEvent::Process(process_event))
                .await
            {
                error!(error = %e, "Failed to send process event");
                self.stats.collection_errors.fetch_add(1, Ordering::Relaxed);
                return Err(e).with_context(|| "Failed to send process event with backpressure");
            }
        }

        let _duration = timer.finish();
        let collection_duration = collection_start.elapsed();

        debug!(
            lifecycle_events = lifecycle_events.len(),
            collection_duration_ms = collection_duration.as_millis(),
            "Procmond monitor collection cycle completed"
        );

        Ok(())
    }

    /// Internal backpressure handling for sending events.
    async fn send_event_with_backpressure_internal(
        &self,
        tx: &mpsc::Sender<CollectionEvent>,
        event: CollectionEvent,
    ) -> anyhow::Result<()> {
        const CIRCUIT_BREAKER_THRESHOLD: usize = 5;
        const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 10;

        // Check circuit breaker state
        #[allow(clippy::expect_used)] // Mutex poisoning indicates a panic - propagate it
        {
            let cooldown_until_opt = {
                let cooldown_guard = self
                    .circuit_breaker_until
                    .lock()
                    .expect("circuit_breaker_until mutex poisoned");
                *cooldown_guard
            };
            if let Some(cooldown_until) = cooldown_until_opt {
                if Instant::now() < cooldown_until {
                    self.stats
                        .backpressure_events
                        .fetch_add(1, Ordering::Relaxed);
                    warn!(
                        "Circuit breaker active, dropping event (cooldown until {:?})",
                        cooldown_until
                    );
                    return Err(anyhow::anyhow!("Circuit breaker active, event dropped"));
                }
                // Cooldown expired, reset circuit breaker
                *self
                    .circuit_breaker_until
                    .lock()
                    .expect("circuit_breaker_until mutex poisoned") = None;
                self.consecutive_backpressure_timeouts
                    .store(0, Ordering::Relaxed);
            }
        }

        // Try non-blocking acquire first
        let permit = match self.backpressure_semaphore.try_acquire() {
            Ok(permit) => {
                self.consecutive_backpressure_timeouts
                    .store(0, Ordering::Relaxed);
                permit
            }
            Err(_) => {
                match timeout(
                    Duration::from_secs(5),
                    self.backpressure_semaphore.acquire(),
                )
                .await
                {
                    Ok(Ok(permit)) => {
                        self.consecutive_backpressure_timeouts
                            .store(0, Ordering::Relaxed);
                        permit
                    }
                    Ok(Err(_)) => {
                        return Err(anyhow::anyhow!("Backpressure semaphore closed"));
                    }
                    Err(_) => {
                        let previous = self
                            .consecutive_backpressure_timeouts
                            .fetch_add(1, Ordering::Relaxed);
                        let consecutive = previous.saturating_add(1);

                        self.stats
                            .backpressure_events
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(
                            consecutive_timeouts = consecutive,
                            "Backpressure timeout while acquiring permit"
                        );

                        if consecutive >= CIRCUIT_BREAKER_THRESHOLD {
                            #[allow(clippy::arithmetic_side_effects)]
                            let cooldown_until =
                                Instant::now() + Duration::from_secs(CIRCUIT_BREAKER_COOLDOWN_SECS);
                            #[allow(clippy::expect_used)]
                            let mut guard = self
                                .circuit_breaker_until
                                .lock()
                                .expect("circuit_breaker_until mutex poisoned");
                            *guard = Some(cooldown_until);
                            warn!(
                                cooldown_seconds = CIRCUIT_BREAKER_COOLDOWN_SECS,
                                "Circuit breaker activated due to consecutive backpressure timeouts"
                            );
                        }

                        return Err(anyhow::anyhow!(
                            "Backpressure timeout while acquiring permit"
                        ));
                    }
                }
            }
        };

        // Update in-flight counter
        self.stats.events_in_flight.fetch_add(1, Ordering::Relaxed);

        // Send event with timeout
        let send_result = timeout(Duration::from_secs(5), tx.send(event)).await;

        // Update in-flight counter and release permit
        self.stats.events_in_flight.fetch_sub(1, Ordering::Relaxed);
        drop(permit);

        match send_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => {
                warn!("Event channel closed during send");
                Err(anyhow::anyhow!("Event channel closed"))
            }
            Err(_) => {
                warn!("Event send timed out");
                Err(anyhow::anyhow!("Event send timeout"))
            }
        }
    }
}

#[async_trait]
impl EventSource for ProcmondMonitorCollector {
    fn name(&self) -> &'static str {
        "procmond-monitor-collector"
    }

    fn capabilities(&self) -> SourceCaps {
        let mut caps = SourceCaps::PROCESS | SourceCaps::SYSTEM_WIDE;

        // Add real-time capability if collecting frequently
        if self.config.base_config.collection_interval <= Duration::from_secs(10) {
            caps |= SourceCaps::REALTIME;
        }

        caps
    }

    /// Legacy start method - use `run()` for actor-based operation.
    ///
    /// This method is retained for API compatibility with the EventSource trait,
    /// but the actor-based `run()` method should be used for new code.
    #[instrument(
        skip(self, _tx, _shutdown_signal),
        fields(source = "procmond-monitor-collector")
    )]
    async fn start(
        &self,
        _tx: mpsc::Sender<CollectionEvent>,
        _shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        // In actor mode, this method should not be called directly.
        // The actor is started via the `run()` method instead.
        warn!(
            "EventSource::start() called on actor-based collector. \
             Use ProcmondMonitorCollector::run() instead for actor-based operation."
        );
        Err(anyhow::anyhow!(
            "EventSource::start() is deprecated for actor-based collectors. \
             Use ProcmondMonitorCollector::run() instead."
        ))
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // In actor mode, shutdown is handled via ActorHandle::graceful_shutdown()
        info!("EventSource::stop() called - use ActorHandle::graceful_shutdown() for actor mode");
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        // In actor mode, health check is handled via ActorHandle::health_check()
        // This basic check verifies the collector is in a valid state
        if self.state == CollectorState::Stopped {
            return Err(anyhow::anyhow!("Collector is stopped"));
        }
        Ok(())
    }
}

impl MonitorCollectorTrait for ProcmondMonitorCollector {
    fn stats(&self) -> MonitorCollectorStatsSnapshot {
        self.stats.snapshot()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::clone_on_ref_ptr
)]
mod tests {
    use super::*;
    use daemoneye_lib::storage::DatabaseManager;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn create_test_database() -> Arc<Mutex<DatabaseManager>> {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let db_manager = DatabaseManager::new(db_path.to_str().unwrap())
            .expect("Failed to create database manager");
        Arc::new(Mutex::new(db_manager))
    }

    /// Helper to create a collector with its actor channel
    fn create_collector_with_channel(
        db_manager: Arc<Mutex<DatabaseManager>>,
        config: ProcmondMonitorConfig,
    ) -> anyhow::Result<(ProcmondMonitorCollector, ActorHandle)> {
        let (handle, receiver) = ProcmondMonitorCollector::create_channel();
        let collector = ProcmondMonitorCollector::new(db_manager, config, receiver)?;
        Ok((collector, handle))
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_creation() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let result = create_collector_with_channel(db_manager, config);
        assert!(result.is_ok());

        let (collector, _handle) = result.unwrap();
        assert_eq!(collector.name(), "procmond-monitor-collector");

        let caps = collector.capabilities();
        assert!(caps.contains(SourceCaps::PROCESS));
        assert!(caps.contains(SourceCaps::SYSTEM_WIDE));
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_capabilities() {
        let db_manager = create_test_database().await;

        // Test real-time capability with fast collection interval
        let fast_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_secs(5),
                ..Default::default()
            },
            ..Default::default()
        };

        let (collector, _handle) =
            create_collector_with_channel(db_manager.clone(), fast_config).unwrap();
        let caps = collector.capabilities();
        assert!(caps.contains(SourceCaps::REALTIME));

        // Test without real-time capability with slow collection interval
        let slow_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_secs(60),
                ..Default::default()
            },
            ..Default::default()
        };

        let (collector, _handle) = create_collector_with_channel(db_manager, slow_config).unwrap();
        let caps = collector.capabilities();
        assert!(!caps.contains(SourceCaps::REALTIME));
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_health_check() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let (collector, _handle) = create_collector_with_channel(db_manager, config).unwrap();

        // Initial health check should pass (collector is in WaitingForAgent state)
        let health_result = collector.health_check().await;
        assert!(health_result.is_ok());
    }

    #[tokio::test]
    async fn test_procmond_monitor_collector_statistics() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let (collector, _handle) = create_collector_with_channel(db_manager, config).unwrap();

        // Initial statistics should be zero
        let stats = collector.stats();
        assert_eq!(stats.collection_cycles, 0);
        assert_eq!(stats.lifecycle_events, 0);
        assert_eq!(stats.trigger_requests, 0);
        assert_eq!(stats.analysis_workflows, 0);
        assert_eq!(stats.events_in_flight, 0);
        assert_eq!(stats.collection_errors, 0);
        assert_eq!(stats.trigger_errors, 0);
        assert_eq!(stats.analysis_errors, 0);
        assert_eq!(stats.backpressure_events, 0);
    }

    #[tokio::test]
    async fn test_actor_handle_operations() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let (collector, handle) = create_collector_with_channel(db_manager, config).unwrap();

        // Verify initial state
        assert_eq!(collector.state, CollectorState::WaitingForAgent);

        // Test that handle methods work (before actor is running, they should fail)
        // This is expected because the receiver is held by the collector
        assert!(!handle.is_closed());
    }

    #[tokio::test]
    async fn test_collector_state_display() {
        assert_eq!(
            CollectorState::WaitingForAgent.to_string(),
            "waiting_for_agent"
        );
        assert_eq!(CollectorState::Running.to_string(), "running");
        assert_eq!(CollectorState::ShuttingDown.to_string(), "shutting_down");
        assert_eq!(CollectorState::Stopped.to_string(), "stopped");
    }

    #[tokio::test]
    async fn test_health_check_data() {
        let db_manager = create_test_database().await;
        let config = ProcmondMonitorConfig::default();

        let (collector, _handle) = create_collector_with_channel(db_manager, config).unwrap();

        let health_data = collector.build_health_data();
        assert_eq!(health_data.state, CollectorState::WaitingForAgent);
        assert!(!health_data.event_bus_connected);
        assert!(health_data.last_collection.is_none());
        assert_eq!(health_data.collection_cycles, 0);
    }

    #[test]
    fn test_config_validation() {
        // Test valid configuration
        let valid_config = ProcmondMonitorConfig::default();
        assert!(valid_config.validate().is_ok());

        // Test invalid collection interval
        let invalid_interval_config = ProcmondMonitorConfig {
            base_config: MonitorCollectorConfig {
                collection_interval: Duration::from_millis(500),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(invalid_interval_config.validate().is_err());
    }

    #[test]
    fn test_actor_channel_capacity() {
        assert_eq!(ACTOR_CHANNEL_CAPACITY, 100);
    }
}
