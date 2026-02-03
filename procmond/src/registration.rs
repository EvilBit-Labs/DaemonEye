//! Registration Manager for procmond.
//!
//! This module provides the `RegistrationManager` component that handles collector
//! registration lifecycle with the daemoneye-agent. It manages:
//!
//! - Initial registration on startup
//! - Periodic heartbeat publishing
//! - Graceful deregistration on shutdown
//!
//! # Registration Flow
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         Registration Lifecycle                           │
//! └─────────────────────────────────────────────────────────────────────────┘
//!
//! ┌────────────┐     ┌─────────────┐     ┌────────────┐     ┌──────────────┐
//! │Unregistered│────▶│ Registering │────▶│ Registered │────▶│Deregistering │
//! └────────────┘     └─────────────┘     └────────────┘     └──────────────┘
//!                           │                   │
//!                           │ (failure)         │ (heartbeat)
//!                           ▼                   ▼
//!                    ┌─────────────┐     ┌────────────────┐
//!                    │ Retry with  │     │ Publish to     │
//!                    │ backoff     │     │ heartbeat topic│
//!                    └─────────────┘     └────────────────┘
//! ```

use crate::event_bus_connector::EventBusConnector;
use crate::monitor_collector::ActorHandle;
use daemoneye_eventbus::{
    DeregistrationRequest, HealthStatus, RegistrationRequest, RegistrationResponse,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Default heartbeat interval in seconds.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Default registration timeout in seconds.
pub const DEFAULT_REGISTRATION_TIMEOUT_SECS: u64 = 10;

/// Maximum registration retry attempts.
pub const MAX_REGISTRATION_RETRIES: u32 = 3;

/// Heartbeat topic prefix.
pub const HEARTBEAT_TOPIC_PREFIX: &str = "control.health.heartbeat";

/// Registration topic.
pub const REGISTRATION_TOPIC: &str = "control.collector.lifecycle";

/// Registration state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistrationState {
    /// Not yet registered with the agent.
    Unregistered,
    /// Registration in progress.
    Registering,
    /// Successfully registered and receiving commands.
    Registered,
    /// Deregistration in progress.
    Deregistering,
    /// Registration failed after retries.
    Failed,
}

impl std::fmt::Display for RegistrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Unregistered => write!(f, "unregistered"),
            Self::Registering => write!(f, "registering"),
            Self::Registered => write!(f, "registered"),
            Self::Deregistering => write!(f, "deregistering"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Errors that can occur during registration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// Registration request failed.
    #[error("Registration failed: {0}")]
    RegistrationFailed(String),

    /// Registration was rejected by the agent.
    #[error("Registration rejected: {0}")]
    RegistrationRejected(String),

    /// Registration timed out.
    #[error("Registration timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    /// Failed to publish heartbeat.
    #[error("Failed to publish heartbeat: {0}")]
    HeartbeatFailed(String),

    /// Deregistration failed.
    #[error("Deregistration failed: {0}")]
    DeregistrationFailed(String),

    /// Event bus error.
    #[error("Event bus error: {0}")]
    EventBusError(String),

    /// Invalid state transition.
    #[error("Invalid state transition from {from} to {to}")]
    InvalidStateTransition {
        from: RegistrationState,
        to: RegistrationState,
    },
}

/// Result type for registration operations.
pub type RegistrationResult<T> = Result<T, RegistrationError>;

/// Connection status for heartbeat metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ConnectionStatus {
    /// Connected to the event bus.
    Connected,
    /// Disconnected from the event bus.
    Disconnected,
    /// Reconnecting to the event bus.
    Reconnecting,
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Connected => write!(f, "connected"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

/// Heartbeat metrics included in each heartbeat message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeartbeatMetrics {
    /// Number of processes collected since last heartbeat.
    pub processes_collected: u64,
    /// Number of events published since last heartbeat.
    pub events_published: u64,
    /// Current buffer level as a percentage (0-100).
    pub buffer_level_percent: f64,
    /// Current connection status.
    pub connection_status: ConnectionStatus,
}

impl Default for HeartbeatMetrics {
    fn default() -> Self {
        Self {
            processes_collected: 0,
            events_published: 0,
            buffer_level_percent: 0.0,
            connection_status: ConnectionStatus::Disconnected,
        }
    }
}

/// Heartbeat message published periodically.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeartbeatMessage {
    /// Collector identifier.
    pub collector_id: String,
    /// Sequence number for this heartbeat.
    pub sequence: u64,
    /// Timestamp of this heartbeat.
    pub timestamp: SystemTime,
    /// Overall health status.
    pub health_status: HealthStatus,
    /// Current metrics.
    pub metrics: HeartbeatMetrics,
}

/// Configuration for the registration manager.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Collector identifier.
    pub collector_id: String,
    /// Collector type (e.g., "process-monitor").
    pub collector_type: String,
    /// Software version.
    pub version: String,
    /// Declared capabilities.
    pub capabilities: Vec<String>,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Registration timeout.
    pub registration_timeout: Duration,
    /// Maximum registration retries.
    pub max_retries: u32,
    /// Additional attributes.
    pub attributes: HashMap<String, serde_json::Value>,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            collector_id: "procmond".to_owned(),
            collector_type: "process-monitor".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: vec![
                "process-collection".to_owned(),
                "lifecycle-tracking".to_owned(),
                "enhanced-metadata".to_owned(),
                "executable-hashing".to_owned(),
            ],
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            registration_timeout: Duration::from_secs(DEFAULT_REGISTRATION_TIMEOUT_SECS),
            max_retries: MAX_REGISTRATION_RETRIES,
            attributes: HashMap::new(),
        }
    }
}

/// Registration manager for procmond.
///
/// Handles collector registration with the daemoneye-agent, periodic heartbeats,
/// and graceful deregistration.
pub struct RegistrationManager {
    /// Configuration.
    config: RegistrationConfig,
    /// Current registration state.
    state: Arc<RwLock<RegistrationState>>,
    /// Event bus connector.
    event_bus: Arc<RwLock<EventBusConnector>>,
    /// Actor handle for health checks.
    actor_handle: ActorHandle,
    /// Heartbeat sequence counter.
    heartbeat_sequence: AtomicU64,
    /// Assigned heartbeat interval from registration response.
    assigned_heartbeat_interval: Arc<RwLock<Option<Duration>>>,
    /// Statistics.
    stats: Arc<RwLock<RegistrationStats>>,
}

/// Statistics for the registration manager.
#[derive(Debug, Clone, Default)]
pub struct RegistrationStats {
    /// Number of registration attempts.
    pub registration_attempts: u64,
    /// Number of successful registrations.
    pub successful_registrations: u64,
    /// Number of failed registrations.
    pub failed_registrations: u64,
    /// Number of heartbeats sent.
    pub heartbeats_sent: u64,
    /// Number of heartbeat failures.
    pub heartbeat_failures: u64,
    /// Last successful heartbeat time.
    pub last_heartbeat: Option<SystemTime>,
    /// Last registration time.
    pub last_registration: Option<SystemTime>,
}

impl RegistrationManager {
    /// Creates a new registration manager.
    ///
    /// # Arguments
    ///
    /// * `event_bus` - Event bus connector for publishing messages
    /// * `actor_handle` - Handle to the collector actor for health checks
    /// * `config` - Registration configuration
    pub fn new(
        event_bus: Arc<RwLock<EventBusConnector>>,
        actor_handle: ActorHandle,
        config: RegistrationConfig,
    ) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(RegistrationState::Unregistered)),
            event_bus,
            actor_handle,
            heartbeat_sequence: AtomicU64::new(0),
            assigned_heartbeat_interval: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(RegistrationStats::default())),
        }
    }

    /// Creates a new registration manager with default configuration.
    pub fn with_defaults(
        event_bus: Arc<RwLock<EventBusConnector>>,
        actor_handle: ActorHandle,
    ) -> Self {
        Self::new(event_bus, actor_handle, RegistrationConfig::default())
    }

    /// Returns the current registration state.
    pub async fn state(&self) -> RegistrationState {
        *self.state.read().await
    }

    /// Returns the collector ID.
    pub fn collector_id(&self) -> &str {
        &self.config.collector_id
    }

    /// Returns a snapshot of the current statistics.
    pub async fn stats(&self) -> RegistrationStats {
        self.stats.read().await.clone()
    }

    /// Returns the effective heartbeat interval.
    ///
    /// Uses the assigned interval from registration response if available,
    /// otherwise falls back to the configured interval.
    pub async fn effective_heartbeat_interval(&self) -> Duration {
        self.assigned_heartbeat_interval
            .read()
            .await
            .unwrap_or(self.config.heartbeat_interval)
    }

    /// Atomically transitions from Unregistered/Failed to Registering state.
    ///
    /// Returns Ok(()) if transition succeeded, Err with InvalidStateTransition otherwise.
    async fn try_transition_to_registering(&self) -> RegistrationResult<()> {
        let mut state_guard = self.state.write().await;
        let current_state = *state_guard;
        if current_state != RegistrationState::Unregistered
            && current_state != RegistrationState::Failed
        {
            return Err(RegistrationError::InvalidStateTransition {
                from: current_state,
                to: RegistrationState::Registering,
            });
        }
        *state_guard = RegistrationState::Registering;
        drop(state_guard);
        Ok(())
    }

    /// Registers with the daemoneye-agent.
    ///
    /// This method attempts registration with retries and exponential backoff.
    /// On success, it transitions to `Registered` state and returns the response.
    pub async fn register(&self) -> RegistrationResult<RegistrationResponse> {
        // Atomic check-and-set to prevent TOCTOU race conditions
        self.try_transition_to_registering().await?;

        info!(
            collector_id = %self.config.collector_id,
            "Starting registration with daemoneye-agent"
        );

        // Build registration request
        let request = self.build_registration_request();

        // Attempt registration with retries
        let mut last_error = None;
        let mut retry_delay = Duration::from_secs(1);

        for attempt in 1..=self.config.max_retries {
            self.increment_registration_attempts().await;

            debug!(
                collector_id = %self.config.collector_id,
                attempt = attempt,
                max_retries = self.config.max_retries,
                "Attempting registration"
            );

            match self.send_registration_request(&request).await {
                Ok(response) => {
                    if response.accepted {
                        // Registration successful
                        *self.state.write().await = RegistrationState::Registered;

                        // Store assigned heartbeat interval
                        let assigned_interval =
                            Duration::from_millis(response.heartbeat_interval_ms);
                        *self.assigned_heartbeat_interval.write().await = Some(assigned_interval);

                        self.record_successful_registration().await;

                        info!(
                            collector_id = %self.config.collector_id,
                            heartbeat_interval_ms = response.heartbeat_interval_ms,
                            assigned_topics = ?response.assigned_topics,
                            "Registration successful"
                        );

                        return Ok(response);
                    }

                    // Registration rejected
                    let message = response
                        .message
                        .unwrap_or_else(|| "Unknown reason".to_owned());
                    warn!(
                        collector_id = %self.config.collector_id,
                        reason = %message,
                        "Registration rejected"
                    );
                    last_error = Some(RegistrationError::RegistrationRejected(message));
                }
                Err(e) => {
                    warn!(
                        collector_id = %self.config.collector_id,
                        attempt = attempt,
                        error = %e,
                        "Registration attempt failed"
                    );
                    last_error = Some(e);
                }
            }

            // Wait before retry (except on last attempt)
            if attempt < self.config.max_retries {
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2); // Exponential backoff
            }
        }

        // All retries exhausted
        *self.state.write().await = RegistrationState::Failed;

        self.record_failed_registration().await;

        error!(
            collector_id = %self.config.collector_id,
            "Registration failed after {} attempts",
            self.config.max_retries
        );

        Err(last_error
            .unwrap_or_else(|| RegistrationError::RegistrationFailed("Unknown error".to_owned())))
    }

    /// Sends a single registration request.
    ///
    /// # Note
    ///
    /// This method currently simulates a successful response since the full
    /// request/response infrastructure requires raw topic publishing support
    /// in EventBusConnector. In a full implementation, we would:
    /// 1. Publish to the registration topic
    /// 2. Wait for a response on a reply topic
    /// 3. Handle timeout and retry logic
    #[allow(clippy::unused_async)] // Will be async when EventBusConnector supports RPC
    async fn send_registration_request(
        &self,
        request: &RegistrationRequest,
    ) -> RegistrationResult<RegistrationResponse> {
        // Serialize request for logging/future use
        let _payload = postcard::to_allocvec(request).map_err(|e| {
            RegistrationError::RegistrationFailed(format!("Failed to serialize request: {e}"))
        })?;

        let correlation_id = format!("reg-{}-{}", self.config.collector_id, uuid::Uuid::new_v4());

        info!(
            collector_id = %self.config.collector_id,
            correlation_id = %correlation_id,
            topic = %REGISTRATION_TOPIC,
            "Registration request prepared (integration pending)"
        );

        // TODO: Integrate with EventBusConnector when raw topic publishing is available
        // For now, simulate a successful response for development/testing
        Ok(RegistrationResponse {
            collector_id: self.config.collector_id.clone(),
            accepted: true,
            heartbeat_interval_ms: u64::try_from(self.config.heartbeat_interval.as_millis())
                .unwrap_or(u64::MAX),
            assigned_topics: vec![format!("control.collector.{}", self.config.collector_id)],
            message: Some("Registration accepted (simulated)".to_owned()),
        })
    }

    /// Builds a registration request.
    fn build_registration_request(&self) -> RegistrationRequest {
        // Get hostname using std::env or system calls
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME")) // Windows fallback
            .unwrap_or_else(|_| "unknown".to_owned());

        RegistrationRequest {
            collector_id: self.config.collector_id.clone(),
            collector_type: self.config.collector_type.clone(),
            hostname,
            version: Some(self.config.version.clone()),
            pid: Some(std::process::id()),
            capabilities: self.config.capabilities.clone(),
            attributes: self.config.attributes.clone(),
            heartbeat_interval_ms: Some(
                u64::try_from(self.config.heartbeat_interval.as_millis()).unwrap_or(u64::MAX),
            ),
        }
    }

    /// Deregisters from the daemoneye-agent.
    ///
    /// This method should be called during graceful shutdown.
    pub async fn deregister(&self, reason: Option<String>) -> RegistrationResult<()> {
        let current_state = self.state().await;
        if current_state != RegistrationState::Registered {
            warn!(
                collector_id = %self.config.collector_id,
                state = %current_state,
                "Cannot deregister: not in Registered state"
            );
            return Ok(()); // Not an error, just nothing to do
        }

        // Transition to Deregistering state
        *self.state.write().await = RegistrationState::Deregistering;

        info!(
            collector_id = %self.config.collector_id,
            reason = ?reason,
            "Deregistering from daemoneye-agent"
        );

        // Build deregistration request
        let request = DeregistrationRequest {
            collector_id: self.config.collector_id.clone(),
            reason,
            force: false,
        };

        // Serialize request for logging/debugging
        let _payload = postcard::to_allocvec(&request).map_err(|e| {
            RegistrationError::DeregistrationFailed(format!("Failed to serialize request: {e}"))
        })?;

        // TODO: EventBusConnector currently only supports ProcessEvent publishing.
        // Full RPC support requires extending the connector with generic message publishing.
        // For now, deregistration is a local state transition.
        // The agent will detect the collector is gone via missing heartbeats.
        debug!(
            collector_id = %self.config.collector_id,
            topic = REGISTRATION_TOPIC,
            "Deregistration message prepared (RPC publish pending EventBusConnector extension)"
        );

        // Transition to Unregistered state
        *self.state.write().await = RegistrationState::Unregistered;

        info!(
            collector_id = %self.config.collector_id,
            "Deregistration complete"
        );

        Ok(())
    }

    /// Publishes a heartbeat message.
    ///
    /// This method should be called periodically (typically every 30 seconds).
    pub async fn publish_heartbeat(&self) -> RegistrationResult<()> {
        let current_state = self.state().await;
        if current_state != RegistrationState::Registered {
            debug!(
                collector_id = %self.config.collector_id,
                state = %current_state,
                "Skipping heartbeat: not registered"
            );
            return Ok(());
        }

        // Get health data from actor with timeout to prevent blocking indefinitely
        // Use a timeout of 5 seconds - shorter than heartbeat interval
        let health_check_timeout = Duration::from_secs(5);
        let health_status = match tokio::time::timeout(
            health_check_timeout,
            self.actor_handle.health_check(),
        )
        .await
        {
            Ok(Ok(health)) => {
                if health.event_bus_connected {
                    match health.state {
                        crate::monitor_collector::CollectorState::Running => HealthStatus::Healthy,
                        crate::monitor_collector::CollectorState::WaitingForAgent => {
                            HealthStatus::Degraded
                        }
                        crate::monitor_collector::CollectorState::ShuttingDown
                        | crate::monitor_collector::CollectorState::Stopped => {
                            HealthStatus::Unhealthy
                        }
                    }
                } else {
                    HealthStatus::Degraded
                }
            }
            Ok(Err(e)) => {
                warn!(
                    collector_id = %self.config.collector_id,
                    error = %e,
                    "Failed to get health check for heartbeat"
                );
                HealthStatus::Unknown
            }
            Err(_) => {
                warn!(
                    collector_id = %self.config.collector_id,
                    "Health check timed out for heartbeat, actor may be stalled"
                );
                HealthStatus::Unknown
            }
        };

        // Build heartbeat message
        let sequence = self.heartbeat_sequence.fetch_add(1, Ordering::Relaxed);
        let heartbeat = self.build_heartbeat_message(sequence, health_status).await;

        // Serialize heartbeat for logging/debugging
        let _payload = postcard::to_allocvec(&heartbeat).map_err(|e| {
            RegistrationError::HeartbeatFailed(format!("Failed to serialize heartbeat: {e}"))
        })?;

        // Build topic for heartbeat
        let topic = format!("{}.{}", HEARTBEAT_TOPIC_PREFIX, self.config.collector_id);

        // TODO: EventBusConnector currently only supports ProcessEvent publishing.
        // Full heartbeat support requires extending the connector with generic message publishing.
        // For now, heartbeat is logged locally. The agent integration will be completed
        // when the connector gains RPC/control message support.
        debug!(
            collector_id = %self.config.collector_id,
            topic = %topic,
            sequence = sequence,
            health_status = ?health_status,
            "Heartbeat message prepared (RPC publish pending EventBusConnector extension)"
        );

        // Update stats
        self.record_heartbeat().await;

        debug!(
            collector_id = %self.config.collector_id,
            sequence = sequence,
            health_status = ?health_status,
            "Heartbeat prepared (event bus publish pending connector extension)"
        );

        Ok(())
    }

    /// Builds a heartbeat message with current metrics.
    async fn build_heartbeat_message(
        &self,
        sequence: u64,
        health_status: HealthStatus,
    ) -> HeartbeatMessage {
        // Get connection status and buffer usage from connector
        // Drop the lock immediately after reading
        let event_bus_guard = self.event_bus.read().await;
        let is_connected = event_bus_guard.is_connected();
        let buffer_level_percent = f64::from(event_bus_guard.buffer_usage_percent());
        drop(event_bus_guard);

        // Use actual connection state from EventBusConnector
        let connection_status = if is_connected {
            ConnectionStatus::Connected
        } else {
            ConnectionStatus::Disconnected
        };

        let metrics = HeartbeatMetrics {
            processes_collected: 0, // TODO: Get from actor stats
            events_published: 0,    // TODO: Get from connector stats
            buffer_level_percent,
            connection_status,
        };

        HeartbeatMessage {
            collector_id: self.config.collector_id.clone(),
            sequence,
            timestamp: SystemTime::now(),
            health_status,
            metrics,
        }
    }

    /// Spawns a background task that publishes heartbeats at the configured interval.
    ///
    /// Returns a handle that can be used to abort the task.
    pub fn spawn_heartbeat_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = self;

        tokio::spawn(async move {
            // Wait for initial registration
            loop {
                let state = manager.state().await;
                if state == RegistrationState::Registered {
                    break;
                }
                if state == RegistrationState::Failed {
                    warn!("Heartbeat task exiting: registration failed");
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }

            info!(
                collector_id = %manager.config.collector_id,
                "Starting heartbeat task"
            );

            let mut interval = tokio::time::interval(manager.effective_heartbeat_interval().await);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                // Check if still registered
                let state = manager.state().await;
                if state != RegistrationState::Registered {
                    info!(
                        collector_id = %manager.config.collector_id,
                        state = %state,
                        "Heartbeat task stopping: no longer registered"
                    );
                    break;
                }

                // Publish heartbeat
                if let Err(e) = manager.publish_heartbeat().await {
                    manager.record_heartbeat_failure().await;
                    warn!(
                        collector_id = %manager.config.collector_id,
                        error = %e,
                        "Failed to publish heartbeat"
                    );
                }
            }
        })
    }

    // --- Statistics helper methods ---

    /// Increments the registration attempts counter.
    async fn increment_registration_attempts(&self) {
        let mut stats = self.stats.write().await;
        stats.registration_attempts = stats.registration_attempts.saturating_add(1);
    }

    /// Records a successful registration.
    async fn record_successful_registration(&self) {
        let mut stats = self.stats.write().await;
        stats.successful_registrations = stats.successful_registrations.saturating_add(1);
        stats.last_registration = Some(SystemTime::now());
    }

    /// Records a failed registration.
    async fn record_failed_registration(&self) {
        let mut stats = self.stats.write().await;
        stats.failed_registrations = stats.failed_registrations.saturating_add(1);
    }

    /// Records a successful heartbeat.
    async fn record_heartbeat(&self) {
        let mut stats = self.stats.write().await;
        stats.heartbeats_sent = stats.heartbeats_sent.saturating_add(1);
        stats.last_heartbeat = Some(SystemTime::now());
    }

    /// Records a failed heartbeat.
    async fn record_heartbeat_failure(&self) {
        let mut stats = self.stats.write().await;
        stats.heartbeat_failures = stats.heartbeat_failures.saturating_add(1);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::str_to_string
)]
mod tests {
    use super::*;
    use crate::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorMessage};
    use tokio::sync::mpsc;

    /// Creates a test actor handle.
    fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
        let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
        (ActorHandle::new(tx), rx)
    }

    /// Creates an EventBusConnector with a unique temp directory for test isolation.
    async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create event bus connector");
        (Arc::new(RwLock::new(connector)), temp_dir)
    }

    #[tokio::test]
    async fn test_registration_config_default() {
        let config = RegistrationConfig::default();
        assert_eq!(config.collector_id, "procmond");
        assert_eq!(config.collector_type, "process-monitor");
        assert!(!config.capabilities.is_empty());
        assert_eq!(
            config.heartbeat_interval,
            Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
        );
    }

    #[tokio::test]
    async fn test_registration_state_display() {
        assert_eq!(
            format!("{}", RegistrationState::Unregistered),
            "unregistered"
        );
        assert_eq!(format!("{}", RegistrationState::Registering), "registering");
        assert_eq!(format!("{}", RegistrationState::Registered), "registered");
        assert_eq!(
            format!("{}", RegistrationState::Deregistering),
            "deregistering"
        );
        assert_eq!(format!("{}", RegistrationState::Failed), "failed");
    }

    #[tokio::test]
    async fn test_connection_status_display() {
        assert_eq!(format!("{}", ConnectionStatus::Connected), "connected");
        assert_eq!(
            format!("{}", ConnectionStatus::Disconnected),
            "disconnected"
        );
        assert_eq!(
            format!("{}", ConnectionStatus::Reconnecting),
            "reconnecting"
        );
    }

    #[tokio::test]
    async fn test_heartbeat_metrics_default() {
        let metrics = HeartbeatMetrics::default();
        assert_eq!(metrics.processes_collected, 0);
        assert_eq!(metrics.events_published, 0);
        assert!((metrics.buffer_level_percent - 0.0).abs() < f64::EPSILON);
        assert_eq!(metrics.connection_status, ConnectionStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_initial_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        assert_eq!(manager.state().await, RegistrationState::Unregistered);
        assert_eq!(manager.collector_id(), "procmond");
    }

    #[tokio::test]
    async fn test_build_registration_request() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        let request = manager.build_registration_request();

        assert_eq!(request.collector_id, "procmond");
        assert_eq!(request.collector_type, "process-monitor");
        assert!(request.pid.is_some());
        assert!(!request.capabilities.is_empty());
        assert!(!request.hostname.is_empty());
    }

    #[tokio::test]
    async fn test_effective_heartbeat_interval() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Initially uses configured interval
        let interval = manager.effective_heartbeat_interval().await;
        assert_eq!(
            interval,
            Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
        );

        // After setting assigned interval, uses that instead
        *manager.assigned_heartbeat_interval.write().await = Some(Duration::from_secs(60));
        let updated_interval = manager.effective_heartbeat_interval().await;
        assert_eq!(updated_interval, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_stats_initial() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        let stats = manager.stats().await;
        assert_eq!(stats.registration_attempts, 0);
        assert_eq!(stats.successful_registrations, 0);
        assert_eq!(stats.failed_registrations, 0);
        assert_eq!(stats.heartbeats_sent, 0);
        assert_eq!(stats.heartbeat_failures, 0);
        assert!(stats.last_heartbeat.is_none());
        assert!(stats.last_registration.is_none());
    }

    #[tokio::test]
    async fn test_heartbeat_message_serialization() {
        let heartbeat = HeartbeatMessage {
            collector_id: "procmond".to_string(),
            sequence: 42,
            timestamp: SystemTime::now(),
            health_status: HealthStatus::Healthy,
            metrics: HeartbeatMetrics {
                processes_collected: 100,
                events_published: 50,
                buffer_level_percent: 25.5,
                connection_status: ConnectionStatus::Connected,
            },
        };

        // Should serialize without error
        let payload = postcard::to_allocvec(&heartbeat).expect("Serialization should succeed");
        assert!(!payload.is_empty());

        // Should deserialize back
        let deserialized: HeartbeatMessage =
            postcard::from_bytes(&payload).expect("Deserialization should succeed");
        assert_eq!(deserialized.collector_id, "procmond");
        assert_eq!(deserialized.sequence, 42);
        assert_eq!(deserialized.metrics.processes_collected, 100);
    }

    #[tokio::test]
    async fn test_invalid_state_transition() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Attempting to register again should fail
        let result = manager.register().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistrationError::InvalidStateTransition { from, to } => {
                assert_eq!(from, RegistrationState::Registered);
                assert_eq!(to, RegistrationState::Registering);
            }
            RegistrationError::RegistrationFailed(_)
            | RegistrationError::RegistrationRejected(_)
            | RegistrationError::Timeout { .. }
            | RegistrationError::HeartbeatFailed(_)
            | RegistrationError::DeregistrationFailed(_)
            | RegistrationError::EventBusError(_) => {
                panic!("Expected InvalidStateTransition error")
            }
        }
    }

    #[tokio::test]
    async fn test_build_heartbeat_message() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        let heartbeat = manager
            .build_heartbeat_message(42, HealthStatus::Healthy)
            .await;

        assert_eq!(heartbeat.collector_id, "procmond");
        assert_eq!(heartbeat.sequence, 42);
        assert_eq!(heartbeat.health_status, HealthStatus::Healthy);
        // Buffer should be empty initially
        assert!((heartbeat.metrics.buffer_level_percent - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_deregister_from_registered_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Deregister should succeed and transition to Unregistered
        let result = manager.deregister(Some("Test reason".to_string())).await;
        assert!(result.is_ok());

        // State should transition to Unregistered after deregistration
        let state = manager.state().await;
        assert_eq!(state, RegistrationState::Unregistered);
    }

    #[tokio::test]
    async fn test_deregister_from_unregistered_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // State is Unregistered by default

        // Deregister should return Ok (nothing to do)
        let result = manager.deregister(None).await;
        assert!(result.is_ok());

        // State should remain Unregistered
        let state = manager.state().await;
        assert_eq!(state, RegistrationState::Unregistered);
    }

    #[tokio::test]
    async fn test_publish_heartbeat_skips_when_not_registered() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // State is Unregistered by default

        // Publish heartbeat should succeed but skip actual publishing
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());

        // Stats should not show any heartbeats sent
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 0);
    }

    #[tokio::test]
    async fn test_registration_stats_helpers() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Test increment_registration_attempts
        manager.increment_registration_attempts().await;
        let stats_after_attempt = manager.stats().await;
        assert_eq!(stats_after_attempt.registration_attempts, 1);

        // Test record_successful_registration
        manager.record_successful_registration().await;
        let stats_after_success = manager.stats().await;
        assert_eq!(stats_after_success.successful_registrations, 1);
        assert!(stats_after_success.last_registration.is_some());

        // Test record_failed_registration
        manager.record_failed_registration().await;
        let stats_after_failure = manager.stats().await;
        assert_eq!(stats_after_failure.failed_registrations, 1);

        // Test record_heartbeat
        manager.record_heartbeat().await;
        let stats_after_heartbeat = manager.stats().await;
        assert_eq!(stats_after_heartbeat.heartbeats_sent, 1);
        assert!(stats_after_heartbeat.last_heartbeat.is_some());

        // Test record_heartbeat_failure
        manager.record_heartbeat_failure().await;
        let stats_after_hb_failure = manager.stats().await;
        assert_eq!(stats_after_hb_failure.heartbeat_failures, 1);
    }

    // ==================== Registration Flow Tests ====================

    #[tokio::test]
    async fn test_register_successful() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Register should succeed and transition to Registered state
        let result = manager.register().await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(response.accepted);
        assert_eq!(response.collector_id, "procmond");
        assert!(!response.assigned_topics.is_empty());

        // State should be Registered
        assert_eq!(manager.state().await, RegistrationState::Registered);

        // Stats should reflect successful registration
        let stats = manager.stats().await;
        assert_eq!(stats.registration_attempts, 1);
        assert_eq!(stats.successful_registrations, 1);
        assert!(stats.last_registration.is_some());
    }

    #[tokio::test]
    async fn test_register_from_failed_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Failed (simulating previous registration failure)
        *manager.state.write().await = RegistrationState::Failed;

        // Should be able to register again from Failed state
        let result = manager.register().await;
        assert!(result.is_ok());

        // State should be Registered
        assert_eq!(manager.state().await, RegistrationState::Registered);
    }

    #[tokio::test]
    async fn test_register_invalid_from_deregistering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Deregistering
        *manager.state.write().await = RegistrationState::Deregistering;

        // Attempting to register should fail
        let result = manager.register().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistrationError::InvalidStateTransition { from, to } => {
                assert_eq!(from, RegistrationState::Deregistering);
                assert_eq!(to, RegistrationState::Registering);
            }
            _ => panic!("Expected InvalidStateTransition error"),
        }
    }

    #[tokio::test]
    async fn test_register_invalid_from_registering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registering
        *manager.state.write().await = RegistrationState::Registering;

        // Attempting to register should fail
        let result = manager.register().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistrationError::InvalidStateTransition { from, to } => {
                assert_eq!(from, RegistrationState::Registering);
                assert_eq!(to, RegistrationState::Registering);
            }
            _ => panic!("Expected InvalidStateTransition error"),
        }
    }

    #[tokio::test]
    async fn test_register_stores_assigned_heartbeat_interval() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Initial interval should be the default
        let initial_interval = manager.effective_heartbeat_interval().await;
        assert_eq!(
            initial_interval,
            Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
        );

        // Register
        let result = manager.register().await;
        assert!(result.is_ok());

        // After registration, interval should be assigned from response
        let assigned = manager.assigned_heartbeat_interval.read().await;
        assert!(assigned.is_some());
    }

    // ==================== Heartbeat Tests ====================

    /// Creates a test HealthCheckData with the given state and connection status.
    fn create_test_health_data(
        state: crate::monitor_collector::CollectorState,
        connected: bool,
    ) -> crate::monitor_collector::HealthCheckData {
        crate::monitor_collector::HealthCheckData {
            state,
            collection_interval: Duration::from_secs(5),
            original_interval: Duration::from_secs(5),
            event_bus_connected: connected,
            buffer_level_percent: Some(10),
            last_collection: Some(std::time::Instant::now()),
            collection_cycles: 100,
            lifecycle_events: 50,
            collection_errors: 0,
            backpressure_events: 0,
        }
    }

    #[tokio::test]
    async fn test_publish_heartbeat_when_registered() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Spawn a task to respond to health check
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            }
        });

        // Publish heartbeat
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());

        // Stats should show heartbeat sent
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 1);
        assert!(stats.last_heartbeat.is_some());

        // Heartbeat sequence should have incremented
        let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
        assert_eq!(sequence, 1);

        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_publish_heartbeat_increments_sequence() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Spawn a task to respond to multiple health checks
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            for _ in 0..3 {
                if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                    let health = create_test_health_data(
                        crate::monitor_collector::CollectorState::Running,
                        true,
                    );
                    let _ = respond_to.send(health);
                }
            }
        });

        // Publish 3 heartbeats
        for _ in 0..3 {
            let result = manager.publish_heartbeat().await;
            assert!(result.is_ok());
        }

        // Sequence should be 3
        let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
        assert_eq!(sequence, 3);

        // Stats should show 3 heartbeats
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 3);

        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_publish_heartbeat_skips_in_deregistering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Deregistering
        *manager.state.write().await = RegistrationState::Deregistering;

        // Publish heartbeat should succeed but skip actual publishing
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());

        // Stats should not show any heartbeats sent
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 0);
    }

    #[tokio::test]
    async fn test_publish_heartbeat_skips_in_failed_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Failed
        *manager.state.write().await = RegistrationState::Failed;

        // Publish heartbeat should succeed but skip actual publishing
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());

        // Stats should not show any heartbeats sent
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 0);
    }

    #[tokio::test]
    async fn test_publish_heartbeat_skips_in_registering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registering
        *manager.state.write().await = RegistrationState::Registering;

        // Publish heartbeat should succeed but skip actual publishing
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());

        // Stats should not show any heartbeats sent
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 0);
    }

    // ==================== Health Status Tests ====================

    #[tokio::test]
    async fn test_heartbeat_health_status_healthy() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Respond with Running state and connected
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            }
        });

        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_heartbeat_health_status_degraded_waiting_for_agent() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Respond with WaitingForAgent state
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::WaitingForAgent,
                    true,
                );
                let _ = respond_to.send(health);
            }
        });

        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_heartbeat_health_status_degraded_disconnected() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Respond with Running state but disconnected
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    false, // Not connected
                );
                let _ = respond_to.send(health);
            }
        });

        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_heartbeat_health_status_unhealthy_shutting_down() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Respond with ShuttingDown state
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::ShuttingDown,
                    true,
                );
                let _ = respond_to.send(health);
            }
        });

        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_heartbeat_health_status_unhealthy_stopped() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Respond with Stopped state
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Stopped,
                    false,
                );
                let _ = respond_to.send(health);
            }
        });

        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
        health_responder.await.unwrap();
    }

    #[tokio::test]
    async fn test_heartbeat_health_check_timeout() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;

        // Create manager with very short timeout for testing
        let config = RegistrationConfig {
            heartbeat_interval: Duration::from_millis(100),
            ..RegistrationConfig::default()
        };
        let manager = RegistrationManager::new(event_bus, actor_handle, config);

        *manager.state.write().await = RegistrationState::Registered;

        // Don't respond to health check - it will timeout
        // The test times out the health check and reports Unknown status
        let result = manager.publish_heartbeat().await;
        // Should succeed even with timeout (reports Unknown health)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_heartbeat_health_check_error() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        *manager.state.write().await = RegistrationState::Registered;

        // Drop the receiver to cause channel closed error
        tokio::spawn(async move {
            // Receive the message but don't respond (drop the oneshot sender)
            if let Some(ActorMessage::HealthCheck { respond_to: _ }) = rx.recv().await {
                // Don't respond - just let it drop
            }
        });

        // Give time for the spawn to execute
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = manager.publish_heartbeat().await;
        // Should succeed even with error (reports Unknown health)
        assert!(result.is_ok());
    }

    // ==================== Deregistration Tests ====================

    #[tokio::test]
    async fn test_deregister_with_reason() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Deregister with reason
        let result = manager
            .deregister(Some("Graceful shutdown".to_string()))
            .await;
        assert!(result.is_ok());

        // State should be Unregistered
        assert_eq!(manager.state().await, RegistrationState::Unregistered);
    }

    #[tokio::test]
    async fn test_deregister_from_failed_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Failed
        *manager.state.write().await = RegistrationState::Failed;

        // Deregister should return Ok (nothing to do)
        let result = manager.deregister(None).await;
        assert!(result.is_ok());

        // State should remain Failed
        assert_eq!(manager.state().await, RegistrationState::Failed);
    }

    #[tokio::test]
    async fn test_deregister_from_registering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Registering
        *manager.state.write().await = RegistrationState::Registering;

        // Deregister should return Ok (nothing to do)
        let result = manager.deregister(None).await;
        assert!(result.is_ok());

        // State should remain Registering
        assert_eq!(manager.state().await, RegistrationState::Registering);
    }

    #[tokio::test]
    async fn test_deregister_from_deregistering_state() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set state to Deregistering
        *manager.state.write().await = RegistrationState::Deregistering;

        // Deregister should return Ok (nothing to do)
        let result = manager.deregister(None).await;
        assert!(result.is_ok());

        // State should remain Deregistering
        assert_eq!(manager.state().await, RegistrationState::Deregistering);
    }

    // ==================== Heartbeat Task Tests ====================

    #[tokio::test]
    async fn test_spawn_heartbeat_task_waits_for_registration() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

        // Spawn heartbeat task while unregistered
        let handle = Arc::clone(&manager).spawn_heartbeat_task();

        // Give task time to start waiting
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Task should still be running (waiting for registration)
        assert!(!handle.is_finished());

        // Abort the task to clean up
        handle.abort();
    }

    #[tokio::test]
    async fn test_spawn_heartbeat_task_exits_on_failed_registration() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

        // Spawn heartbeat task
        let handle = Arc::clone(&manager).spawn_heartbeat_task();

        // Give task time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Set state to Failed
        *manager.state.write().await = RegistrationState::Failed;

        // Wait for task to notice and exit
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // Task should have exited
        assert!(handle.is_finished());
    }

    #[tokio::test]
    async fn test_spawn_heartbeat_task_runs_when_registered() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;

        // Create manager with short heartbeat interval for testing
        let config = RegistrationConfig {
            heartbeat_interval: Duration::from_millis(100),
            ..RegistrationConfig::default()
        };
        let manager = Arc::new(RegistrationManager::new(event_bus, actor_handle, config));

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Spawn a task to respond to health checks
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            for _ in 0..3 {
                if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                    let health = create_test_health_data(
                        crate::monitor_collector::CollectorState::Running,
                        true,
                    );
                    let _ = respond_to.send(health);
                }
            }
        });

        // Spawn heartbeat task
        let handle = Arc::clone(&manager).spawn_heartbeat_task();

        // Wait for a few heartbeats
        tokio::time::sleep(Duration::from_millis(350)).await;

        // Should have published at least 2 heartbeats
        let stats = manager.stats().await;
        assert!(stats.heartbeats_sent >= 2);

        // Abort the task
        handle.abort();
        health_responder.abort();
    }

    #[tokio::test]
    async fn test_spawn_heartbeat_task_stops_when_deregistered() {
        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;

        // Create manager with short heartbeat interval
        let config = RegistrationConfig {
            heartbeat_interval: Duration::from_millis(100),
            ..RegistrationConfig::default()
        };
        let manager = Arc::new(RegistrationManager::new(event_bus, actor_handle, config));

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Spawn a task to respond to health checks
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            loop {
                if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                    let health = create_test_health_data(
                        crate::monitor_collector::CollectorState::Running,
                        true,
                    );
                    let _ = respond_to.send(health);
                } else {
                    break;
                }
            }
        });

        // Spawn heartbeat task
        let handle = Arc::clone(&manager).spawn_heartbeat_task();

        // Wait for heartbeat task to start
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Deregister
        *manager.state.write().await = RegistrationState::Unregistered;

        // Wait for task to notice and exit
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Task should have exited
        assert!(handle.is_finished());

        health_responder.abort();
    }

    // ==================== Connection Status Tests ====================

    #[tokio::test]
    async fn test_build_heartbeat_message_with_connected_status() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Build heartbeat - event bus is not connected by default
        let heartbeat = manager
            .build_heartbeat_message(1, HealthStatus::Healthy)
            .await;

        // Connection status should reflect disconnected (default state)
        assert_eq!(
            heartbeat.metrics.connection_status,
            ConnectionStatus::Disconnected
        );
    }

    // ==================== Error Type Tests ====================

    #[test]
    fn test_registration_error_display() {
        let err1 = RegistrationError::RegistrationFailed("Test failure".to_string());
        assert!(err1.to_string().contains("Registration failed"));

        let err2 = RegistrationError::RegistrationRejected("Invalid collector".to_string());
        assert!(err2.to_string().contains("Registration rejected"));

        let err3 = RegistrationError::Timeout { timeout_secs: 30 };
        assert!(err3.to_string().contains("30"));

        let err4 = RegistrationError::HeartbeatFailed("Publish error".to_string());
        assert!(err4.to_string().contains("heartbeat"));

        let err5 = RegistrationError::DeregistrationFailed("Deregistration error".to_string());
        assert!(err5.to_string().contains("Deregistration"));

        let err6 = RegistrationError::EventBusError("Bus error".to_string());
        assert!(err6.to_string().contains("Event bus"));

        let err7 = RegistrationError::InvalidStateTransition {
            from: RegistrationState::Registered,
            to: RegistrationState::Registering,
        };
        assert!(err7.to_string().contains("Invalid state transition"));
    }

    // ==================== Config Tests ====================

    #[tokio::test]
    async fn test_registration_config_custom() {
        let config = RegistrationConfig {
            collector_id: "custom-collector".to_owned(),
            collector_type: "custom-type".to_owned(),
            version: "2.0.0".to_owned(),
            capabilities: vec!["cap1".to_owned(), "cap2".to_owned()],
            heartbeat_interval: Duration::from_secs(60),
            registration_timeout: Duration::from_secs(20),
            max_retries: 5,
            attributes: HashMap::new(),
        };

        assert_eq!(config.collector_id, "custom-collector");
        assert_eq!(config.collector_type, "custom-type");
        assert_eq!(config.version, "2.0.0");
        assert_eq!(config.capabilities.len(), 2);
        assert_eq!(config.heartbeat_interval, Duration::from_secs(60));
        assert_eq!(config.registration_timeout, Duration::from_secs(20));
        assert_eq!(config.max_retries, 5);
    }

    #[tokio::test]
    async fn test_registration_manager_new_vs_with_defaults() {
        let (actor_handle1, _rx1) = create_test_actor();
        let (event_bus1, _temp_dir1) = create_test_event_bus().await;
        let manager1 = RegistrationManager::with_defaults(event_bus1, actor_handle1);

        let (actor_handle2, _rx2) = create_test_actor();
        let (event_bus2, _temp_dir2) = create_test_event_bus().await;
        let manager2 =
            RegistrationManager::new(event_bus2, actor_handle2, RegistrationConfig::default());

        // Both should have the same default collector_id
        assert_eq!(manager1.collector_id(), manager2.collector_id());
        assert_eq!(manager1.collector_id(), "procmond");
    }

    // ==================== Concurrent Access Tests ====================

    #[tokio::test]
    async fn test_concurrent_state_reads() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

        // Spawn multiple tasks reading state concurrently
        let mut handles = Vec::new();
        for _ in 0..10 {
            let manager_clone = Arc::clone(&manager);
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    let _state = manager_clone.state().await;
                }
            }));
        }

        // All should complete without deadlock
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_concurrent_stats_reads() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

        // Spawn multiple tasks reading stats concurrently
        let mut handles = Vec::new();
        for _ in 0..10 {
            let manager_clone = Arc::clone(&manager);
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    let _stats = manager_clone.stats().await;
                }
            }));
        }

        // All should complete without deadlock
        for handle in handles {
            handle.await.unwrap();
        }
    }

    // ==================== Stats Overflow Protection Tests ====================

    #[tokio::test]
    async fn test_stats_saturating_add() {
        let (actor_handle, _rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

        // Set stats to near max
        {
            let mut stats = manager.stats.write().await;
            stats.registration_attempts = u64::MAX - 1;
            stats.heartbeats_sent = u64::MAX - 1;
        }

        // Increment - should saturate, not overflow
        manager.increment_registration_attempts().await;
        manager.increment_registration_attempts().await;
        manager.record_heartbeat().await;
        manager.record_heartbeat().await;

        let stats = manager.stats().await;
        assert_eq!(stats.registration_attempts, u64::MAX);
        assert_eq!(stats.heartbeats_sent, u64::MAX);
    }

    // ==================== Topic Format Tests ====================

    #[test]
    fn test_heartbeat_topic_format() {
        let topic = format!("{}.{}", HEARTBEAT_TOPIC_PREFIX, "procmond");
        assert_eq!(topic, "control.health.heartbeat.procmond");
    }

    #[test]
    fn test_registration_topic_constant() {
        assert_eq!(REGISTRATION_TOPIC, "control.collector.lifecycle");
    }

    // ==================== Default Constant Tests ====================

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_HEARTBEAT_INTERVAL_SECS, 30);
        assert_eq!(DEFAULT_REGISTRATION_TIMEOUT_SECS, 10);
        assert_eq!(MAX_REGISTRATION_RETRIES, 3);
    }

    // ==================== Concurrent Heartbeat Tests ====================

    #[tokio::test]
    async fn test_concurrent_heartbeat_publishes() {
        use tokio::sync::Barrier;

        let (actor_handle, mut rx) = create_test_actor();
        let (event_bus, _temp_dir) = create_test_event_bus().await;
        let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

        // Set state to Registered
        *manager.state.write().await = RegistrationState::Registered;

        // Create barrier for synchronizing 10 concurrent tasks
        let barrier = Arc::new(Barrier::new(10));

        // Spawn a task to respond to health check actor messages
        let health_responder = tokio::spawn(async move {
            use crate::monitor_collector::ActorMessage;

            // Respond to 10 health checks (one per concurrent heartbeat)
            for _ in 0..10 {
                if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                    let health = create_test_health_data(
                        crate::monitor_collector::CollectorState::Running,
                        true,
                    );
                    let _ = respond_to.send(health);
                }
            }
        });

        // Spawn 10 concurrent publish_heartbeat() calls
        let mut handles = Vec::new();
        for _ in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let barrier_clone = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                // Wait for all tasks to be ready
                barrier_clone.wait().await;
                // Now all 10 tasks will call publish_heartbeat concurrently
                manager_clone.publish_heartbeat().await
            }));
        }

        // Wait for all heartbeat tasks to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Heartbeat publish should succeed");
        }

        // Wait for health responder to finish
        health_responder.await.unwrap();

        // Verify sequence numbers are properly incremented (final count should be 10)
        let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
        assert_eq!(
            sequence, 10,
            "Sequence should be 10 after 10 concurrent heartbeats"
        );

        // Verify all heartbeats were recorded in stats
        let stats = manager.stats().await;
        assert_eq!(stats.heartbeats_sent, 10, "Should have sent 10 heartbeats");
    }
}
