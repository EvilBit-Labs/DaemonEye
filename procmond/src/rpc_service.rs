//! RPC Service Handler for procmond.
//!
//! This module provides the `RpcServiceHandler` component that handles RPC requests
//! from the daemoneye-agent via the event bus. It subscribes to the control topic
//! `control.collector.procmond`, parses incoming RPC requests, and coordinates with
//! the `ProcmondMonitorCollector` actor to execute operations.
//!
//! # Supported Operations
//!
//! - `HealthCheck` - Returns health status and metrics from the collector
//! - `UpdateConfig` - Updates collector configuration at runtime
//! - `GracefulShutdown` - Initiates graceful shutdown of the collector
//!
//! # Architecture
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

use crate::event_bus_connector::EventBusConnector;
use crate::monitor_collector::{ActorHandle, HealthCheckData as ActorHealthCheckData};
use daemoneye_eventbus::rpc::{
    CollectorOperation, ComponentHealth, ConfigUpdateRequest, ErrorCategory, HealthCheckData,
    HealthStatus, RpcError, RpcPayload, RpcRequest, RpcResponse, RpcStatus,
};
// Re-export for tests
#[cfg(test)]
use daemoneye_eventbus::rpc::RpcCorrelationMetadata;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Default timeout for RPC operations in seconds.
const DEFAULT_RPC_TIMEOUT_SECS: u64 = 30;

/// The control topic for procmond RPC requests.
pub const PROCMOND_CONTROL_TOPIC: &str = "control.collector.procmond";

/// Errors that can occur in the RPC service handler.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RpcServiceError {
    /// Failed to subscribe to the control topic.
    #[error("Failed to subscribe to control topic: {0}")]
    SubscriptionFailed(String),

    /// Failed to publish RPC response.
    #[error("Failed to publish RPC response: {0}")]
    PublishFailed(String),

    /// Failed to forward request to actor.
    #[error("Failed to forward request to actor: {0}")]
    ActorError(String),

    /// Invalid RPC request.
    #[error("Invalid RPC request: {0}")]
    InvalidRequest(String),

    /// Operation not supported.
    #[error("Operation not supported: {operation:?}")]
    UnsupportedOperation { operation: CollectorOperation },

    /// Operation timed out.
    #[error("Operation timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Service is shutting down.
    #[error("Service is shutting down")]
    ShuttingDown,
}

/// Result type for RPC service operations.
pub type RpcServiceResult<T> = Result<T, RpcServiceError>;

/// Configuration for the RPC service handler.
#[derive(Debug, Clone)]
pub struct RpcServiceConfig {
    /// Collector identifier.
    pub collector_id: String,
    /// Control topic to subscribe to.
    pub control_topic: String,
    /// Response topic prefix.
    pub response_topic_prefix: String,
    /// Default operation timeout.
    pub default_timeout: Duration,
    /// Maximum concurrent RPC requests.
    pub max_concurrent_requests: usize,
}

impl Default for RpcServiceConfig {
    fn default() -> Self {
        Self {
            collector_id: "procmond".to_owned(),
            control_topic: PROCMOND_CONTROL_TOPIC.to_owned(),
            response_topic_prefix: "control.rpc.response".to_owned(),
            default_timeout: Duration::from_secs(DEFAULT_RPC_TIMEOUT_SECS),
            max_concurrent_requests: 10,
        }
    }
}

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
    ///
    /// TODO: Currently unused because EventBusConnector only supports ProcessEvent publishing.
    /// Will be used when the connector gains generic RPC message support.
    #[allow(dead_code)]
    event_bus: Arc<RwLock<EventBusConnector>>,
    /// Whether the service is running.
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Statistics tracking.
    stats: Arc<RwLock<RpcServiceStats>>,
    /// Service start time for uptime tracking.
    start_time: std::time::Instant,
}

/// Statistics for the RPC service.
#[derive(Debug, Clone, Default)]
pub struct RpcServiceStats {
    /// Total requests received.
    pub requests_received: u64,
    /// Successful requests.
    pub requests_succeeded: u64,
    /// Failed requests.
    pub requests_failed: u64,
    /// Timed out requests.
    pub requests_timed_out: u64,
    /// Health check requests.
    pub health_checks: u64,
    /// Config update requests.
    pub config_updates: u64,
    /// Shutdown requests.
    pub shutdown_requests: u64,
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
            | CollectorOperation::ExecuteTask => {
                Err(RpcServiceError::UnsupportedOperation { operation })
            }
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

    /// Handles a health check request.
    async fn handle_health_check(
        &self,
        request: &RpcRequest,
    ) -> RpcServiceResult<Option<RpcPayload>> {
        debug!(
            request_id = %request.request_id,
            "Processing health check request"
        );

        // Forward to actor and get health data
        let actor_health = self
            .actor_handle
            .health_check()
            .await
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
            | RpcPayload::Empty => {
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

        // Build new configuration from changes
        let new_config = Self::build_config_from_changes(config_request);

        // Forward to actor
        self.actor_handle
            .update_config(new_config)
            .await
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
            | RpcPayload::Empty => None,
        };

        info!(
            request_id = %request.request_id,
            reason = ?reason,
            "Initiating graceful shutdown"
        );

        // Forward to actor
        self.actor_handle
            .graceful_shutdown()
            .await
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
            crate::monitor_collector::CollectorState::WaitingForAgent => HealthStatus::Degraded,
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
            crate::monitor_collector::CollectorState::Running => HealthStatus::Healthy,
            crate::monitor_collector::CollectorState::WaitingForAgent => HealthStatus::Degraded,
            crate::monitor_collector::CollectorState::ShuttingDown
            | crate::monitor_collector::CollectorState::Stopped => HealthStatus::Unhealthy,
        };
        components.insert(
            "collector".to_owned(),
            ComponentHealth {
                name: "collector".to_owned(),
                status: collector_status,
                message: Some(format!("State: {}", actor_health.state)),
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

    /// Builds a new configuration from the change request.
    #[allow(clippy::unused_self)] // Will use self when we fetch current config from actor
    fn build_config_from_changes(
        config_request: &ConfigUpdateRequest,
    ) -> crate::monitor_collector::ProcmondMonitorConfig {
        use crate::monitor_collector::ProcmondMonitorConfig;

        // Start with default config (in practice, we'd get current config from actor)
        let mut config = ProcmondMonitorConfig::default();

        // Apply changes from request
        for (key, value) in &config_request.config_changes {
            match key.as_str() {
                "collection_interval_secs" => {
                    if let Some(secs) = value.as_u64() {
                        config.base_config.collection_interval = Duration::from_secs(secs);
                    }
                }
                "max_events_in_flight" => {
                    if let Some(max) = value.as_u64() {
                        config.base_config.max_events_in_flight =
                            usize::try_from(max).unwrap_or(usize::MAX);
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
                        config.process_config.max_processes =
                            usize::try_from(max).unwrap_or(usize::MAX);
                    }
                }
                _ => {
                    warn!(key = %key, "Unknown configuration key, ignoring");
                }
            }
        }

        config
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
    /// The response is published to the topic derived from the correlation metadata.
    ///
    /// # Note
    ///
    /// This method is currently a placeholder. Full integration with the EventBusConnector
    /// for RPC response publishing requires additional API support (raw topic publishing).
    /// For now, the response should be handled by the caller.
    #[allow(clippy::unused_async)] // Will be async when EventBusConnector supports RPC
    pub async fn publish_response(&self, response: RpcResponse) -> RpcServiceResult<()> {
        let response_topic = format!(
            "{}.{}",
            self.config.response_topic_prefix, response.correlation_metadata.correlation_id
        );

        debug!(
            request_id = %response.request_id,
            topic = %response_topic,
            status = ?response.status,
            "RPC response ready for publishing"
        );

        // Serialize response for future use
        let _payload = postcard::to_allocvec(&response).map_err(|e| {
            RpcServiceError::PublishFailed(format!("Failed to serialize response: {e}"))
        })?;

        // TODO: Integrate with EventBusConnector when raw topic publishing is available
        // For now, responses are logged and the serialized payload is available for
        // integration with the broker when that API is extended.
        info!(
            request_id = %response.request_id,
            topic = %response_topic,
            status = ?response.status,
            "RPC response serialized and ready"
        );

        Ok(())
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
            | CollectorOperation::ExecuteTask => {
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
    use crate::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorMessage, CollectorState};
    use tokio::sync::mpsc;

    /// Creates a test actor handle with a receiver for inspecting messages.
    fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
        let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
        (ActorHandle::new(tx), rx)
    }

    #[tokio::test]
    async fn test_rpc_service_config_default() {
        let config = RpcServiceConfig::default();
        assert_eq!(config.collector_id, "procmond");
        assert_eq!(config.control_topic, PROCMOND_CONTROL_TOPIC);
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert_eq!(config.max_concurrent_requests, 10);
    }

    #[tokio::test]
    async fn test_create_success_response() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let request = RpcRequest {
            request_id: "test-123".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::HealthCheck,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
            correlation_metadata: RpcCorrelationMetadata::new("corr-123".to_string()),
        };

        let start_time = std::time::Instant::now();
        let response =
            handler.create_success_response(&request, Some(RpcPayload::Empty), start_time);

        assert_eq!(response.request_id, "test-123");
        assert_eq!(response.service_id, "procmond");
        assert_eq!(response.operation, CollectorOperation::HealthCheck);
        assert_eq!(response.status, RpcStatus::Success);
        assert!(response.error_details.is_none());
    }

    #[tokio::test]
    async fn test_create_error_response() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let request = RpcRequest {
            request_id: "test-456".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::UpdateConfig,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
            correlation_metadata: RpcCorrelationMetadata::new("corr-456".to_string()),
        };

        let error = RpcServiceError::InvalidRequest("Missing config payload".to_string());
        let start_time = std::time::Instant::now();
        let response = handler.create_error_response(&request, &error, start_time);

        assert_eq!(response.request_id, "test-456");
        assert_eq!(response.status, RpcStatus::Error);
        assert!(response.error_details.is_some());
        let error_details = response.error_details.unwrap();
        assert_eq!(error_details.code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn test_convert_health_data() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let actor_health = ActorHealthCheckData {
            state: CollectorState::Running,
            collection_interval: Duration::from_secs(30),
            original_interval: Duration::from_secs(30),
            event_bus_connected: true,
            buffer_level_percent: Some(25),
            last_collection: Some(std::time::Instant::now()),
            collection_cycles: 100,
            lifecycle_events: 50,
            collection_errors: 2,
            backpressure_events: 5,
        };

        let health_data = handler.convert_health_data(&actor_health);

        assert_eq!(health_data.collector_id, "procmond");
        assert_eq!(health_data.status, HealthStatus::Healthy);
        assert!(health_data.components.contains_key("event_bus"));
        assert!(health_data.components.contains_key("collector"));
        assert_eq!(
            health_data.metrics.get("collection_cycles"),
            Some(&100.0_f64)
        );
        assert_eq!(health_data.metrics.get("lifecycle_events"), Some(&50.0_f64));
    }

    #[tokio::test]
    async fn test_convert_health_data_degraded() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let actor_health = ActorHealthCheckData {
            state: CollectorState::Running,
            collection_interval: Duration::from_secs(30),
            original_interval: Duration::from_secs(30),
            event_bus_connected: false, // Disconnected
            buffer_level_percent: Some(80),
            last_collection: Some(std::time::Instant::now()),
            collection_cycles: 50,
            lifecycle_events: 25,
            collection_errors: 10,
            backpressure_events: 15,
        };

        let health_data = handler.convert_health_data(&actor_health);

        assert_eq!(health_data.status, HealthStatus::Degraded);
        let event_bus_health = health_data.components.get("event_bus").unwrap();
        assert_eq!(event_bus_health.status, HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn test_build_config_from_changes() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let mut changes = HashMap::new();
        changes.insert(
            "collection_interval_secs".to_string(),
            serde_json::json!(60),
        );
        changes.insert("max_processes".to_string(), serde_json::json!(500));
        changes.insert(
            "compute_executable_hashes".to_string(),
            serde_json::json!(true),
        );

        let config_request = ConfigUpdateRequest {
            collector_id: "procmond".to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false,
            rollback_on_failure: true,
        };

        let _ = handler; // Silence unused warning
        let config = RpcServiceHandler::build_config_from_changes(&config_request);

        assert_eq!(
            config.base_config.collection_interval,
            Duration::from_secs(60)
        );
        assert_eq!(config.process_config.max_processes, 500);
        assert!(config.process_config.compute_executable_hashes);
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let stats = handler.stats().await;
        assert_eq!(stats.requests_received, 0);
        assert_eq!(stats.requests_succeeded, 0);
        assert_eq!(stats.requests_failed, 0);
    }

    #[tokio::test]
    async fn test_expired_deadline_returns_timeout() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        // Create request with deadline in the past
        let request = RpcRequest {
            request_id: "test-expired".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::HealthCheck,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now() - Duration::from_secs(60),
            deadline: SystemTime::now() - Duration::from_secs(30), // Past deadline
            correlation_metadata: RpcCorrelationMetadata::new("corr-expired".to_string()),
        };

        let response = handler.handle_request(request).await;

        assert_eq!(response.status, RpcStatus::Timeout);
        assert!(response.error_details.is_some());
        let error = response.error_details.unwrap();
        assert_eq!(error.code, "DEADLINE_EXCEEDED");
        assert_eq!(error.category, ErrorCategory::Timeout);
    }

    #[tokio::test]
    async fn test_health_check_sends_message_to_actor() {
        let (actor_handle, mut rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let request = RpcRequest {
            request_id: "test-health".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::HealthCheck,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
            correlation_metadata: RpcCorrelationMetadata::new("corr-health".to_string()),
        };

        // Spawn response handler - the actor needs to respond to the health check
        let handle_task = tokio::spawn(async move { handler.handle_request(request).await });

        // Wait for the health check message from the actor
        let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(msg.is_ok(), "Actor should receive a message");
        let actor_msg = msg.unwrap();
        assert!(actor_msg.is_some(), "Message should be present");

        // Verify it's a health check message
        match actor_msg.unwrap() {
            ActorMessage::HealthCheck { respond_to } => {
                // Respond with mock health data
                let health_data = crate::monitor_collector::HealthCheckData {
                    state: CollectorState::Running,
                    collection_interval: Duration::from_secs(30),
                    original_interval: Duration::from_secs(30),
                    event_bus_connected: true,
                    buffer_level_percent: Some(10),
                    last_collection: Some(std::time::Instant::now()),
                    collection_cycles: 5,
                    lifecycle_events: 2,
                    collection_errors: 0,
                    backpressure_events: 0,
                };
                // Intentionally ignore send result - receiver may have been dropped
                drop(respond_to.send(health_data));
            }
            ActorMessage::UpdateConfig { .. }
            | ActorMessage::GracefulShutdown { .. }
            | ActorMessage::BeginMonitoring
            | ActorMessage::AdjustInterval { .. } => {
                panic!("Expected HealthCheck message")
            }
        }

        // Wait for the response
        let response = handle_task.await.expect("Handle task should complete");
        assert_eq!(response.status, RpcStatus::Success);
        assert!(response.payload.is_some());
    }

    #[tokio::test]
    async fn test_config_update_invalid_payload() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        // Create config update request with wrong payload type (Empty instead of ConfigUpdate)
        let request = RpcRequest {
            request_id: "test-bad-config".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::UpdateConfig,
            payload: RpcPayload::Empty, // Wrong payload type
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
            correlation_metadata: RpcCorrelationMetadata::new("corr-bad-config".to_string()),
        };

        let response = handler.handle_request(request).await;

        assert_eq!(response.status, RpcStatus::Error);
        assert!(response.error_details.is_some());
        let error = response.error_details.unwrap();
        assert_eq!(error.code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn test_create_timeout_response() {
        let (actor_handle, _rx) = create_test_actor();
        let event_bus = Arc::new(RwLock::new(
            EventBusConnector::new(std::path::PathBuf::from("/tmp/test-wal"))
                .await
                .expect("Failed to create event bus connector"),
        ));
        let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

        let request = RpcRequest {
            request_id: "test-timeout".to_string(),
            client_id: "client-1".to_string(),
            target: "control.collector.procmond".to_string(),
            operation: CollectorOperation::HealthCheck,
            payload: RpcPayload::Empty,
            timestamp: SystemTime::now(),
            deadline: SystemTime::now() - Duration::from_secs(1), // Already expired
            correlation_metadata: RpcCorrelationMetadata::new("corr-timeout".to_string()),
        };

        let start_time = std::time::Instant::now();
        let response = handler.create_timeout_response(&request, start_time);

        assert_eq!(response.request_id, "test-timeout");
        assert_eq!(response.service_id, "procmond");
        assert_eq!(response.status, RpcStatus::Timeout);
        assert!(response.error_details.is_some());
        let error = response.error_details.unwrap();
        assert_eq!(error.code, "DEADLINE_EXCEEDED");
        assert_eq!(error.category, ErrorCategory::Timeout);
    }
}
