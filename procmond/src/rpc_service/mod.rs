//! RPC Service Handler for procmond.
//!
//! This module provides the `RpcServiceHandler` component that handles RPC requests
//! from the daemoneye-agent. It parses incoming RPC requests and coordinates with
//! the `ProcmondMonitorCollector` actor to execute operations.
//!
//! The handler subscribes to the control topic, handles incoming RPC requests by
//! forwarding them to the actor, and publishes responses to the event bus.
//!
//! # Supported Operations
//!
//! - `HealthCheck` - Returns health status and metrics from the collector
//! - `UpdateConfig` - Updates collector configuration at runtime
//! - `GracefulShutdown` - Initiates graceful shutdown of the collector
//!
//! # Architecture (Target Design)
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────────┐     ┌─────────────────────┐
//! │  Event Bus      │────▶│  RpcServiceHandler   │────▶│  ActorHandle        │
//! │  (subscribe)    │     │  (parse & dispatch)  │     │  (forward to actor) │
//! └─────────────────┘     └──────────────────────┘     └─────────────────────┘
//!                                   │
//!                                   ▼
//!                         ┌──────────────────────┐
//!                         │  Event Bus           │
//!                         │  (publish response)  │
//!                         └──────────────────────┘
//! ```

mod config;
mod error;
mod stats;

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
    clippy::redundant_clone,
    clippy::items_after_statements,
    clippy::let_underscore_must_use
)]
mod tests;

pub use config::RpcServiceConfig;
pub use error::{RpcServiceError, RpcServiceResult};
pub use stats::RpcServiceStats;

use crate::event_bus_connector::EventBusConnector;
use crate::monitor_collector::{ActorHandle, HealthCheckData as ActorHealthCheckData};
use daemoneye_eventbus::rpc::{
    CollectorOperation, ComponentHealth, ConfigUpdateRequest, ErrorCategory, HealthCheckData,
    HealthStatus, RpcError, RpcPayload, RpcRequest, RpcResponse, RpcStatus,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// The control topic for procmond RPC requests.
pub const PROCMOND_CONTROL_TOPIC: &str = "control.collector.procmond";

/// RPC service handler for procmond.
///
/// This component subscribes to the control topic and handles incoming RPC requests
/// by forwarding them to the `ProcmondMonitorCollector` actor and publishing responses.
pub struct RpcServiceHandler {
    /// Configuration.
    config: RpcServiceConfig,
    /// Actor handle for communicating with the collector.
    actor_handle: ActorHandle,
    /// Event bus connector for publishing responses.
    event_bus: Arc<RwLock<EventBusConnector>>,
    /// Whether the service is running.
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Statistics tracking.
    stats: Arc<RwLock<RpcServiceStats>>,
    /// Service start time for uptime tracking.
    start_time: std::time::Instant,
}

impl RpcServiceHandler {
    /// Creates a new RPC service handler.
    ///
    /// # Arguments
    ///
    /// * `actor_handle` - Handle to communicate with the collector actor
    /// * `event_bus` - Event bus connector for publishing responses
    /// * `config` - Service configuration
    pub fn new(
        actor_handle: ActorHandle,
        event_bus: Arc<RwLock<EventBusConnector>>,
        config: RpcServiceConfig,
    ) -> Self {
        Self {
            config,
            actor_handle,
            event_bus,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(RpcServiceStats::default())),
            start_time: std::time::Instant::now(),
        }
    }

    /// Creates a new RPC service handler with default configuration.
    pub fn with_defaults(
        actor_handle: ActorHandle,
        event_bus: Arc<RwLock<EventBusConnector>>,
    ) -> Self {
        Self::new(actor_handle, event_bus, RpcServiceConfig::default())
    }

    /// Returns the collector ID.
    pub fn collector_id(&self) -> &str {
        &self.config.collector_id
    }

    /// Returns the configuration.
    pub const fn config(&self) -> &RpcServiceConfig {
        &self.config
    }

    /// Returns whether the service is running.
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns a snapshot of the current statistics.
    pub async fn stats(&self) -> RpcServiceStats {
        self.stats.read().await.clone()
    }

    /// Handles an incoming RPC request.
    ///
    /// This method parses the request, forwards it to the appropriate handler,
    /// and returns the response to be published.
    pub async fn handle_request(&self, request: RpcRequest) -> RpcResponse {
        let start_time = std::time::Instant::now();
        let request_id = request.request_id.clone();
        let operation = request.operation;

        // Update stats
        self.record_request_received().await;

        debug!(
            request_id = %request_id,
            operation = ?operation,
            target = %request.target,
            "Handling RPC request"
        );

        // Check if request has expired
        if request.deadline < SystemTime::now() {
            warn!(
                request_id = %request_id,
                "RPC request deadline has passed"
            );
            self.record_request_timeout().await;
            return self.create_timeout_response(&request, start_time);
        }

        // Dispatch to appropriate handler
        let result = match operation {
            CollectorOperation::HealthCheck => self.handle_health_check(&request).await,
            CollectorOperation::UpdateConfig => self.handle_config_update(&request).await,
            CollectorOperation::GracefulShutdown => self.handle_graceful_shutdown(&request).await,
            CollectorOperation::Register
            | CollectorOperation::Deregister
            | CollectorOperation::Start
            | CollectorOperation::Stop
            | CollectorOperation::Restart
            | CollectorOperation::GetCapabilities
            | CollectorOperation::ForceShutdown
            | CollectorOperation::Pause
            | CollectorOperation::Resume
            | CollectorOperation::ExecuteTask
            | _ => Err(RpcServiceError::UnsupportedOperation { operation }),
        };

        // Update stats and create response
        let response = match result {
            Ok(payload) => {
                self.record_request_success(operation).await;
                self.create_success_response(&request, payload, start_time)
            }
            Err(e) => {
                self.record_request_failure().await;
                error!(
                    request_id = %request_id,
                    error = %e,
                    "RPC request failed"
                );
                self.create_error_response(&request, &e, start_time)
            }
        };

        info!(
            request_id = %request_id,
            operation = ?operation,
            status = ?response.status,
            execution_time_ms = response.execution_time_ms,
            "RPC request completed"
        );

        response
    }

    /// Calculates the effective timeout for an operation.
    ///
    /// Uses the minimum of the request deadline and the configured default timeout.
    fn calculate_timeout(&self, request: &RpcRequest) -> Duration {
        let now = SystemTime::now();
        let deadline_duration = request
            .deadline
            .duration_since(now)
            .unwrap_or(Duration::ZERO);
        std::cmp::min(deadline_duration, self.config.default_timeout)
    }

    /// Handles a health check request.
    async fn handle_health_check(
        &self,
        request: &RpcRequest,
    ) -> RpcServiceResult<Option<RpcPayload>> {
        debug!(
            request_id = %request.request_id,
            "Processing health check request"
        );

        let timeout = self.calculate_timeout(request);

        // Forward to actor and get health data with timeout
        #[allow(clippy::as_conversions)] // Safe: timeout millis will be well under u64::MAX
        let timeout_ms = timeout.as_millis() as u64;
        let actor_health = tokio::time::timeout(timeout, self.actor_handle.health_check())
            .await
            .map_err(|_elapsed| RpcServiceError::Timeout { timeout_ms })?
            .map_err(|e| RpcServiceError::ActorError(e.to_string()))?;

        // Convert actor health data to RPC health data
        let health_data = self.convert_health_data(&actor_health);

        Ok(Some(RpcPayload::HealthCheck(health_data)))
    }

    /// Handles a configuration update request.
    async fn handle_config_update(
        &self,
        request: &RpcRequest,
    ) -> RpcServiceResult<Option<RpcPayload>> {
        debug!(
            request_id = %request.request_id,
            "Processing config update request"
        );

        // Extract config update from payload
        let config_request = match request.payload {
            RpcPayload::ConfigUpdate(ref req) => req,
            RpcPayload::Lifecycle(_)
            | RpcPayload::Registration(_)
            | RpcPayload::RegistrationResponse(_)
            | RpcPayload::Deregistration(_)
            | RpcPayload::HealthCheck(_)
            | RpcPayload::Capabilities(_)
            | RpcPayload::Shutdown(_)
            | RpcPayload::Task(_)
            | RpcPayload::TaskResult(_)
            | RpcPayload::Generic(_)
            | RpcPayload::Empty
            | _ => {
                return Err(RpcServiceError::InvalidRequest(
                    "Expected ConfigUpdate payload".to_owned(),
                ));
            }
        };

        // If validate_only, just return success without applying
        if config_request.validate_only {
            info!(
                request_id = %request.request_id,
                "Config validation only - no changes applied"
            );
            return Ok(Some(RpcPayload::Empty));
        }

        // Build new configuration from changes (validates inputs)
        let new_config = Self::build_config_from_changes(config_request)?;

        let timeout = self.calculate_timeout(request);

        // Forward to actor with timeout
        #[allow(clippy::as_conversions)] // Safe: timeout millis will be well under u64::MAX
        let timeout_ms = timeout.as_millis() as u64;
        tokio::time::timeout(timeout, self.actor_handle.update_config(new_config))
            .await
            .map_err(|_elapsed| RpcServiceError::Timeout { timeout_ms })?
            .map_err(|e| RpcServiceError::ActorError(e.to_string()))?;

        info!(
            request_id = %request.request_id,
            changed_fields = ?config_request.config_changes.keys().collect::<Vec<_>>(),
            "Configuration updated successfully"
        );

        Ok(Some(RpcPayload::Empty))
    }

    /// Handles a graceful shutdown request.
    async fn handle_graceful_shutdown(
        &self,
        request: &RpcRequest,
    ) -> RpcServiceResult<Option<RpcPayload>> {
        debug!(
            request_id = %request.request_id,
            "Processing graceful shutdown request"
        );

        // Extract shutdown request from payload if present
        let reason = match request.payload {
            RpcPayload::Shutdown(ref shutdown_req) => shutdown_req.reason.clone(),
            RpcPayload::Lifecycle(_)
            | RpcPayload::Registration(_)
            | RpcPayload::RegistrationResponse(_)
            | RpcPayload::Deregistration(_)
            | RpcPayload::HealthCheck(_)
            | RpcPayload::ConfigUpdate(_)
            | RpcPayload::Capabilities(_)
            | RpcPayload::Task(_)
            | RpcPayload::TaskResult(_)
            | RpcPayload::Generic(_)
            | RpcPayload::Empty
            | _ => None,
        };

        info!(
            request_id = %request.request_id,
            reason = ?reason,
            "Initiating graceful shutdown"
        );

        let timeout = self.calculate_timeout(request);

        // Forward to actor with timeout
        #[allow(clippy::as_conversions)] // Safe: timeout millis will be well under u64::MAX
        let timeout_ms = timeout.as_millis() as u64;
        tokio::time::timeout(timeout, self.actor_handle.graceful_shutdown())
            .await
            .map_err(|_elapsed| RpcServiceError::Timeout { timeout_ms })?
            .map_err(|e| RpcServiceError::ActorError(e.to_string()))?;

        // Mark service as not running
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);

        Ok(Some(RpcPayload::Empty))
    }

    /// Converts actor health data to RPC health data format.
    fn convert_health_data(&self, actor_health: &ActorHealthCheckData) -> HealthCheckData {
        // Determine overall health status
        let status = match actor_health.state {
            crate::monitor_collector::CollectorState::Running => {
                if actor_health.event_bus_connected {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Degraded
                }
            }
            crate::monitor_collector::CollectorState::WaitingForAgent => HealthStatus::Healthy,
            crate::monitor_collector::CollectorState::ShuttingDown => HealthStatus::Unhealthy,
            crate::monitor_collector::CollectorState::Stopped => HealthStatus::Unresponsive,
        };

        // Build component health map
        let mut components = HashMap::new();

        // Event bus component
        let event_bus_status = if actor_health.event_bus_connected {
            HealthStatus::Healthy
        } else {
            HealthStatus::Degraded
        };
        components.insert(
            "event_bus".to_owned(),
            ComponentHealth {
                name: "event_bus".to_owned(),
                status: event_bus_status,
                message: Some(if actor_health.event_bus_connected {
                    "Connected".to_owned()
                } else {
                    "Disconnected - buffering events".to_owned()
                }),
                last_check: SystemTime::now(),
                check_interval_seconds: 30,
            },
        );

        // Collector component
        let collector_status = match actor_health.state {
            crate::monitor_collector::CollectorState::Running
            | crate::monitor_collector::CollectorState::WaitingForAgent => HealthStatus::Healthy,
            crate::monitor_collector::CollectorState::ShuttingDown
            | crate::monitor_collector::CollectorState::Stopped => HealthStatus::Unhealthy,
        };
        let collector_message =
            if actor_health.state == crate::monitor_collector::CollectorState::WaitingForAgent {
                "idle-awaiting-begin".to_owned()
            } else {
                format!("State: {}", actor_health.state)
            };
        components.insert(
            "collector".to_owned(),
            ComponentHealth {
                name: "collector".to_owned(),
                status: collector_status,
                message: Some(collector_message),
                last_check: SystemTime::now(),
                check_interval_seconds: 30,
            },
        );

        // Build metrics map
        // Note: u64 to f64 conversion may lose precision for very large values,
        // but these are counters that won't exceed f64's precise integer range
        #[allow(clippy::as_conversions)] // Safe: counter values within f64 precision range
        let metrics = HashMap::from([
            (
                "collection_cycles".to_owned(),
                actor_health.collection_cycles as f64,
            ),
            (
                "lifecycle_events".to_owned(),
                actor_health.lifecycle_events as f64,
            ),
            (
                "collection_errors".to_owned(),
                actor_health.collection_errors as f64,
            ),
            (
                "backpressure_events".to_owned(),
                actor_health.backpressure_events as f64,
            ),
        ]);
        // Add optional buffer level if available
        let mut final_metrics = metrics;
        if let Some(buffer_level) = actor_health.buffer_level_percent {
            final_metrics.insert("buffer_level_percent".to_owned(), f64::from(buffer_level));
        }

        // Calculate uptime from service start time
        let uptime_seconds = self.start_time.elapsed().as_secs();

        HealthCheckData {
            collector_id: self.config.collector_id.clone(),
            status,
            components,
            metrics: final_metrics,
            last_heartbeat: SystemTime::now(),
            uptime_seconds,
            error_count: actor_health.collection_errors,
        }
    }

    /// Maximum allowed value for max_events_in_flight to prevent resource exhaustion.
    const MAX_EVENTS_IN_FLIGHT_LIMIT: u64 = 100_000;

    /// Maximum allowed value for max_processes to prevent resource exhaustion.
    const MAX_PROCESSES_LIMIT: u64 = 1_000_000;

    /// Builds a new configuration from the change request.
    ///
    /// # Limitations
    ///
    /// This method starts from the default configuration and applies changes.
    /// This means partial updates will reset unspecified fields to defaults.
    /// A future enhancement should fetch the current config from the actor
    /// and overlay changes on top of it.
    ///
    /// # Errors
    ///
    /// Returns `InvalidRequest` if any configuration value is out of bounds
    /// or cannot be represented on this platform.
    #[allow(clippy::unused_self)] // Will use self when we fetch current config from actor
    fn build_config_from_changes(
        config_request: &ConfigUpdateRequest,
    ) -> RpcServiceResult<crate::monitor_collector::ProcmondMonitorConfig> {
        use crate::monitor_collector::ProcmondMonitorConfig;

        // Start with default config
        // TODO: Fetch current config from actor to enable partial updates
        // without resetting unspecified fields to defaults
        let mut config = ProcmondMonitorConfig::default();

        // Apply changes from request with validation
        for (key, value) in &config_request.config_changes {
            match key.as_str() {
                "collection_interval_secs" => {
                    if let Some(secs) = value.as_u64() {
                        config.base_config.collection_interval = Duration::from_secs(secs);
                    }
                }
                "max_events_in_flight" => {
                    if let Some(max) = value.as_u64() {
                        if max > Self::MAX_EVENTS_IN_FLIGHT_LIMIT {
                            return Err(RpcServiceError::InvalidRequest(format!(
                                "max_events_in_flight value {max} exceeds maximum allowed ({})",
                                Self::MAX_EVENTS_IN_FLIGHT_LIMIT
                            )));
                        }
                        config.base_config.max_events_in_flight =
                            usize::try_from(max).map_err(|_overflow| {
                                RpcServiceError::InvalidRequest(format!(
                                    "max_events_in_flight value {max} cannot be represented on this platform"
                                ))
                            })?;
                    }
                }
                "collect_enhanced_metadata" => {
                    if let Some(enabled) = value.as_bool() {
                        config.process_config.collect_enhanced_metadata = enabled;
                    }
                }
                "compute_executable_hashes" => {
                    if let Some(enabled) = value.as_bool() {
                        config.process_config.compute_executable_hashes = enabled;
                    }
                }
                "max_processes" => {
                    if let Some(max) = value.as_u64() {
                        if max > Self::MAX_PROCESSES_LIMIT {
                            return Err(RpcServiceError::InvalidRequest(format!(
                                "max_processes value {max} exceeds maximum allowed ({})",
                                Self::MAX_PROCESSES_LIMIT
                            )));
                        }
                        config.process_config.max_processes = usize::try_from(max).map_err(
                            |_overflow| {
                                RpcServiceError::InvalidRequest(format!(
                                    "max_processes value {max} cannot be represented on this platform"
                                ))
                            },
                        )?;
                    }
                }
                _ => {
                    warn!(key = %key, "Unknown configuration key, ignoring");
                }
            }
        }

        Ok(config)
    }

    /// Creates a success response.
    fn create_success_response(
        &self,
        request: &RpcRequest,
        payload: Option<RpcPayload>,
        start_time: std::time::Instant,
    ) -> RpcResponse {
        #[allow(clippy::as_conversions)] // Safe: execution time will be well under u64::MAX ms
        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        RpcResponse {
            request_id: request.request_id.clone(),
            service_id: self.config.collector_id.clone(),
            operation: request.operation,
            status: RpcStatus::Success,
            payload,
            timestamp: SystemTime::now(),
            execution_time_ms,
            queue_time_ms: None,
            total_time_ms: execution_time_ms,
            error_details: None,
            correlation_metadata: request.correlation_metadata.clone(),
        }
    }

    /// Creates an error response.
    fn create_error_response(
        &self,
        request: &RpcRequest,
        error: &RpcServiceError,
        start_time: std::time::Instant,
    ) -> RpcResponse {
        #[allow(clippy::as_conversions)] // Safe: execution time will be well under u64::MAX ms
        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        let (code, category) = match *error {
            RpcServiceError::SubscriptionFailed(_) => {
                ("SUBSCRIPTION_FAILED", ErrorCategory::Communication)
            }
            RpcServiceError::PublishFailed(_) => ("PUBLISH_FAILED", ErrorCategory::Communication),
            RpcServiceError::ActorError(_) => ("ACTOR_ERROR", ErrorCategory::Internal),
            RpcServiceError::InvalidRequest(_) => ("INVALID_REQUEST", ErrorCategory::Configuration),
            RpcServiceError::UnsupportedOperation { .. } => {
                ("UNSUPPORTED_OPERATION", ErrorCategory::Configuration)
            }
            RpcServiceError::Timeout { .. } => ("TIMEOUT", ErrorCategory::Timeout),
            RpcServiceError::ShuttingDown => ("SHUTTING_DOWN", ErrorCategory::Internal),
        };

        RpcResponse {
            request_id: request.request_id.clone(),
            service_id: self.config.collector_id.clone(),
            operation: request.operation,
            status: RpcStatus::Error,
            payload: None,
            timestamp: SystemTime::now(),
            execution_time_ms,
            queue_time_ms: None,
            total_time_ms: execution_time_ms,
            error_details: Some(RpcError {
                code: code.to_owned(),
                #[allow(clippy::str_to_string)] // This is Display::to_string(), not str::to_string()
                message: error.to_string(),
                context: HashMap::new(),
                category,
            }),
            correlation_metadata: request.correlation_metadata.clone(),
        }
    }

    /// Creates a timeout response.
    fn create_timeout_response(
        &self,
        request: &RpcRequest,
        start_time: std::time::Instant,
    ) -> RpcResponse {
        #[allow(clippy::as_conversions)] // Safe: execution time will be well under u64::MAX ms
        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        RpcResponse {
            request_id: request.request_id.clone(),
            service_id: self.config.collector_id.clone(),
            operation: request.operation,
            status: RpcStatus::Timeout,
            payload: None,
            timestamp: SystemTime::now(),
            execution_time_ms,
            queue_time_ms: None,
            total_time_ms: execution_time_ms,
            error_details: Some(RpcError {
                code: "DEADLINE_EXCEEDED".to_owned(),
                message: "Request deadline has passed".to_owned(),
                context: HashMap::new(),
                category: ErrorCategory::Timeout,
            }),
            correlation_metadata: request.correlation_metadata.clone(),
        }
    }

    /// Publishes an RPC response to the event bus.
    ///
    /// The response is published to `control.rpc.response.{collector_id}`.
    pub async fn publish_response(&self, response: RpcResponse) -> RpcServiceResult<()> {
        let response_topic = format!(
            "{}.{}",
            self.config.response_topic_prefix, self.config.collector_id
        );

        debug!(
            request_id = %response.request_id,
            topic = %response_topic,
            status = ?response.status,
            "Publishing RPC response"
        );

        // Serialize response
        let payload = postcard::to_allocvec(&response).map_err(|e| {
            RpcServiceError::PublishFailed(format!("Failed to serialize response: {e}"))
        })?;

        // Publish to event bus. The read guard is held across .await because
        // publish_raw is async on &EventBusConnector. This is acceptable: the
        // read lock doesn't block other reads, and publish is short-lived.
        #[allow(clippy::await_holding_lock)]
        let publish_result = {
            let event_bus = self.event_bus.read().await;
            event_bus.publish_raw(&response_topic, payload).await
        };

        if let Err(e) = publish_result {
            warn!(
                request_id = %response.request_id,
                topic = %response_topic,
                error = %e,
                "Failed to publish RPC response"
            );
            self.record_response_failure().await;
            return Err(RpcServiceError::PublishFailed(format!(
                "Failed to publish response to {response_topic}: {e}"
            )));
        }

        info!(
            request_id = %response.request_id,
            topic = %response_topic,
            status = ?response.status,
            "RPC response published"
        );

        Ok(())
    }

    /// Records a failed response publish.
    async fn record_response_failure(&self) {
        let mut stats = self.stats.write().await;
        stats.responses_failed = stats.responses_failed.saturating_add(1);
    }

    // --- Statistics helper methods ---

    /// Records a received request.
    async fn record_request_received(&self) {
        let mut stats = self.stats.write().await;
        stats.requests_received = stats.requests_received.saturating_add(1);
    }

    /// Records a timed out request.
    async fn record_request_timeout(&self) {
        let mut stats = self.stats.write().await;
        stats.requests_timed_out = stats.requests_timed_out.saturating_add(1);
    }

    /// Records a successful request with operation-specific tracking.
    async fn record_request_success(&self, operation: CollectorOperation) {
        let mut stats = self.stats.write().await;
        stats.requests_succeeded = stats.requests_succeeded.saturating_add(1);
        match operation {
            CollectorOperation::HealthCheck => {
                stats.health_checks = stats.health_checks.saturating_add(1);
            }
            CollectorOperation::UpdateConfig => {
                stats.config_updates = stats.config_updates.saturating_add(1);
            }
            CollectorOperation::GracefulShutdown | CollectorOperation::ForceShutdown => {
                stats.shutdown_requests = stats.shutdown_requests.saturating_add(1);
            }
            CollectorOperation::Register
            | CollectorOperation::Deregister
            | CollectorOperation::Start
            | CollectorOperation::Stop
            | CollectorOperation::Restart
            | CollectorOperation::GetCapabilities
            | CollectorOperation::Pause
            | CollectorOperation::Resume
            | CollectorOperation::ExecuteTask
            | _ => {
                // Other operations don't have specific counters yet
            }
        }
    }

    /// Records a failed request.
    async fn record_request_failure(&self) {
        let mut stats = self.stats.write().await;
        stats.requests_failed = stats.requests_failed.saturating_add(1);
    }
}
