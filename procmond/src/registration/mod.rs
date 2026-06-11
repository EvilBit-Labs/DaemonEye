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

mod config;
mod error;
mod heartbeat;
mod state;

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::str_to_string,
    clippy::wildcard_enum_match_arm,
    clippy::significant_drop_in_scrutinee,
    clippy::uninlined_format_args,
    clippy::needless_pass_by_value,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::let_underscore_must_use,
    clippy::while_let_loop,
    clippy::semicolon_outside_block,
    clippy::significant_drop_tightening
)]
mod tests;

pub use config::RegistrationConfig;
pub use error::{RegistrationError, RegistrationResult};
pub use heartbeat::{ConnectionStatus, HeartbeatMessage, HeartbeatMetrics};
pub use state::RegistrationState;

use crate::event_bus_connector::EventBusConnector;
use crate::monitor_collector::ActorHandle;
use daemoneye_eventbus::{
    DeregistrationRequest, HealthStatus, RegistrationRequest, RegistrationResponse,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
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
        let (health_status, operational_sub_status) = match tokio::time::timeout(
            health_check_timeout,
            self.actor_handle.health_check(),
        )
        .await
        {
            Ok(Ok(health)) => {
                let sub_status = health.operational_sub_status.clone();
                let status = if health.event_bus_connected {
                    match health.state {
                        crate::monitor_collector::CollectorState::Running => HealthStatus::Healthy,
                        crate::monitor_collector::CollectorState::WaitingForAgent => {
                            HealthStatus::Healthy
                        }
                        crate::monitor_collector::CollectorState::ShuttingDown
                        | crate::monitor_collector::CollectorState::Stopped => {
                            HealthStatus::Unhealthy
                        }
                    }
                } else {
                    HealthStatus::Degraded
                };
                (status, sub_status)
            }
            Ok(Err(e)) => {
                warn!(
                    collector_id = %self.config.collector_id,
                    error = %e,
                    "Failed to get health check for heartbeat"
                );
                (HealthStatus::Unknown, None)
            }
            Err(_) => {
                warn!(
                    collector_id = %self.config.collector_id,
                    "Health check timed out for heartbeat, actor may be stalled"
                );
                (HealthStatus::Unknown, None)
            }
        };

        // Build heartbeat message
        let sequence = self.heartbeat_sequence.fetch_add(1, Ordering::Relaxed);
        let heartbeat = self
            .build_heartbeat_message(sequence, health_status, operational_sub_status)
            .await;

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
        operational_sub_status: Option<String>,
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
            operational_sub_status,
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
