//! RPC call patterns for collector lifecycle management
//!
//! This module defines RPC service patterns for managing collector processes through
//! the daemoneye-eventbus message broker. It provides structured request/response
//! patterns for collector start/stop/restart operations, health checks, configuration
//! updates, and graceful shutdown coordination.
//!
//! ## Design Principles
//!
//! - **Request/Response Pattern**: All RPC calls follow a structured request/response model
//! - **Timeout Handling**: All operations have configurable timeouts with graceful degradation
//! - **Error Propagation**: Comprehensive error types with context for debugging
//! - **Correlation Tracking**: All RPC calls include correlation IDs for distributed tracing
//! - **Capability-Based**: Operations respect collector capabilities and constraints
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use daemoneye_eventbus::rpc::{CollectorRpcClient, CollectorLifecycleRequest, RpcRequest, CollectorOperation};
//! use std::time::Duration;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut rpc_client = CollectorRpcClient::new("control.collector.procmond").await?;
//!
//!     // Create a lifecycle request
//!     let lifecycle_request = CollectorLifecycleRequest::start("procmond", None);
//!     let request = RpcRequest::lifecycle(
//!         "client-id".to_string(),
//!         "control.collector.procmond".to_string(),
//!         CollectorOperation::Start,
//!         lifecycle_request,
//!         Duration::from_secs(30)
//!     );
//!     let response = rpc_client.call(request, Duration::from_secs(30)).await?;
//!
//!     println!("Collector started: {:?}", response);
//!     Ok(())
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

use crate::error::{EventBusError, Result};
use crate::message::{Message, MessageType};

/// RPC client for collector lifecycle management
#[derive(Debug)]
pub struct CollectorRpcClient {
    /// Target topic for RPC calls
    pub target_topic: String,
    /// Client identifier for correlation
    pub client_id: String,
    /// Default timeout for RPC calls
    pub default_timeout: Duration,
    /// Pending requests awaiting responses
    pending_requests: HashMap<String, tokio::sync::oneshot::Sender<RpcResponse>>,
}

/// RPC service for handling collector lifecycle operations
#[derive(Debug)]
pub struct CollectorRpcService {
    /// Service identifier
    pub service_id: String,
    /// Supported operations
    pub supported_operations: Vec<CollectorOperation>,
    /// Service capabilities
    pub capabilities: ServiceCapabilities,
}

/// RPC request message for collector operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Unique request identifier for correlation
    pub request_id: String,
    /// Client identifier making the request
    pub client_id: String,
    /// Target service or collector
    pub target: String,
    /// Operation to perform
    pub operation: CollectorOperation,
    /// Request payload
    pub payload: RpcPayload,
    /// Request timestamp
    pub timestamp: SystemTime,
    /// Timeout for this request
    pub timeout_ms: u64,
    /// Correlation ID for distributed tracing
    pub correlation_id: String,
}

/// RPC response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Request ID this response corresponds to
    pub request_id: String,
    /// Service identifier that handled the request
    pub service_id: String,
    /// Operation that was performed
    pub operation: CollectorOperation,
    /// Response status
    pub status: RpcStatus,
    /// Response payload
    pub payload: Option<RpcPayload>,
    /// Response timestamp
    pub timestamp: SystemTime,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Error details if status is Error
    pub error_details: Option<RpcError>,
}

/// RPC operation types for collector lifecycle management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectorOperation {
    /// Start a collector process
    Start,
    /// Stop a collector process
    Stop,
    /// Restart a collector process
    Restart,
    /// Get collector status and health
    HealthCheck,
    /// Update collector configuration
    UpdateConfig,
    /// Get collector capabilities
    GetCapabilities,
    /// Graceful shutdown coordination
    GracefulShutdown,
    /// Force shutdown (emergency)
    ForceShutdown,
    /// Pause collector operations
    Pause,
    /// Resume collector operations
    Resume,
}

/// RPC payload containing operation-specific data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcPayload {
    /// Collector lifecycle request
    Lifecycle(CollectorLifecycleRequest),
    /// Health check request/response
    HealthCheck(HealthCheckData),
    /// Configuration update request
    ConfigUpdate(ConfigUpdateRequest),
    /// Capabilities request/response
    Capabilities(CapabilitiesData),
    /// Shutdown coordination request
    Shutdown(ShutdownRequest),
    /// Generic key-value payload
    Generic(HashMap<String, serde_json::Value>),
    /// Empty payload
    Empty,
}

/// RPC response status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RpcStatus {
    /// Operation completed successfully
    Success,
    /// Operation failed with error
    Error,
    /// Operation is in progress
    InProgress,
    /// Operation timed out
    Timeout,
    /// Operation was cancelled
    Cancelled,
    /// Service is unavailable
    Unavailable,
}

/// RPC error details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Error code for programmatic handling
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Additional error context
    pub context: HashMap<String, serde_json::Value>,
    /// Error category for classification
    pub category: ErrorCategory,
}

/// Error categories for RPC operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// Configuration or validation error
    Configuration,
    /// Resource constraint or limit exceeded
    Resource,
    /// Network or communication error
    Communication,
    /// Permission or authorization error
    Permission,
    /// Internal service error
    Internal,
    /// Timeout or deadline exceeded
    Timeout,
}

/// Collector lifecycle request data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorLifecycleRequest {
    /// Collector identifier
    pub collector_id: String,
    /// Collector type (procmond, netmond, etc.)
    pub collector_type: String,
    /// Configuration overrides
    pub config_overrides: Option<HashMap<String, serde_json::Value>>,
    /// Environment variables
    pub environment: Option<HashMap<String, String>>,
    /// Working directory
    pub working_directory: Option<String>,
    /// Resource limits
    pub resource_limits: Option<ResourceLimits>,
    /// Startup timeout
    pub startup_timeout_ms: Option<u64>,
}

/// Health check data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckData {
    /// Collector identifier
    pub collector_id: String,
    /// Overall health status
    pub status: HealthStatus,
    /// Component-level health details
    pub components: HashMap<String, ComponentHealth>,
    /// Performance metrics
    pub metrics: HashMap<String, f64>,
    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Error count since last reset
    pub error_count: u64,
}

/// Health status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Service is healthy and operational
    Healthy,
    /// Service is degraded but functional
    Degraded,
    /// Service is unhealthy but running
    Unhealthy,
    /// Service is not responding
    Unresponsive,
    /// Service status is unknown
    Unknown,
}

/// Component health details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,
    /// Component status
    pub status: HealthStatus,
    /// Status message
    pub message: Option<String>,
    /// Last check timestamp
    pub last_check: SystemTime,
    /// Check interval in seconds
    pub check_interval_seconds: u64,
}

/// Configuration update request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateRequest {
    /// Collector identifier
    pub collector_id: String,
    /// Configuration changes (key-value pairs)
    pub config_changes: HashMap<String, serde_json::Value>,
    /// Whether to validate configuration before applying
    pub validate_only: bool,
    /// Whether to restart collector after update
    pub restart_required: bool,
    /// Rollback configuration if update fails
    pub rollback_on_failure: bool,
}

/// Capabilities data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesData {
    /// Collector identifier
    pub collector_id: String,
    /// Supported event types
    pub event_types: Vec<String>,
    /// Supported operations
    pub operations: Vec<CollectorOperation>,
    /// Resource requirements
    pub resource_requirements: ResourceRequirements,
    /// Platform compatibility
    pub platform_support: Vec<String>,
    /// Feature flags
    pub features: HashMap<String, bool>,
}

/// Shutdown coordination request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownRequest {
    /// Collector identifier
    pub collector_id: String,
    /// Shutdown type
    pub shutdown_type: ShutdownType,
    /// Graceful shutdown timeout
    pub graceful_timeout_ms: u64,
    /// Force shutdown after timeout
    pub force_after_timeout: bool,
    /// Reason for shutdown
    pub reason: Option<String>,
}

/// Shutdown type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShutdownType {
    /// Graceful shutdown with cleanup
    Graceful,
    /// Immediate shutdown
    Immediate,
    /// Emergency shutdown (force kill)
    Emergency,
}

/// Resource limits for collector processes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum memory usage in bytes
    pub max_memory_bytes: Option<u64>,
    /// Maximum CPU percentage (0-100)
    pub max_cpu_percent: Option<f64>,
    /// Maximum file descriptors
    pub max_file_descriptors: Option<u64>,
    /// Maximum network connections
    pub max_network_connections: Option<u64>,
}

/// Resource requirements for collector capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequirements {
    /// Minimum memory required in bytes
    pub min_memory_bytes: u64,
    /// Minimum CPU percentage required
    pub min_cpu_percent: f64,
    /// Required privileges
    pub required_privileges: Vec<String>,
    /// Required system capabilities
    pub required_capabilities: Vec<String>,
}

/// Service capabilities for RPC services
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCapabilities {
    /// Supported RPC operations
    pub operations: Vec<CollectorOperation>,
    /// Maximum concurrent requests
    pub max_concurrent_requests: u32,
    /// Request timeout limits
    pub timeout_limits: TimeoutLimits,
    /// Supported collector types
    pub supported_collectors: Vec<String>,
}

/// Timeout limits for RPC operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutLimits {
    /// Minimum allowed timeout in milliseconds
    pub min_timeout_ms: u64,
    /// Maximum allowed timeout in milliseconds
    pub max_timeout_ms: u64,
    /// Default timeout in milliseconds
    pub default_timeout_ms: u64,
}

impl CollectorRpcClient {
    /// Create a new RPC client for the specified target topic
    pub async fn new(target_topic: &str) -> Result<Self> {
        Ok(Self {
            target_topic: target_topic.to_string(),
            client_id: Uuid::new_v4().to_string(),
            default_timeout: Duration::from_secs(30),
            pending_requests: HashMap::new(),
        })
    }

    /// Make an RPC call with the specified timeout
    pub async fn call(&mut self, request: RpcRequest, timeout: Duration) -> Result<RpcResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_requests.insert(request.request_id.clone(), tx);

        // Serialize and send request
        let _message = self.create_request_message(request)?;

        // TODO: Send message through event bus
        // This would be implemented when integrating with the actual event bus

        // Wait for response with timeout
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(EventBusError::rpc("Response channel closed".to_string())),
            Err(_) => Err(EventBusError::rpc("Request timeout".to_string())),
        }
    }

    /// Create a request message for the event bus
    fn create_request_message(&self, request: RpcRequest) -> Result<Message> {
        let payload = bincode::serde::encode_to_vec(&request, bincode::config::standard())
            .map_err(|e| EventBusError::serialization(e.to_string()))?;

        Ok(Message::new(
            self.target_topic.clone(),
            request.correlation_id.clone(),
            payload,
            0, // Sequence will be set by event bus
            MessageType::Control,
        ))
    }

    /// Handle incoming response message
    pub fn handle_response(&mut self, response: RpcResponse) -> Result<()> {
        if let Some(tx) = self.pending_requests.remove(&response.request_id) {
            tx.send(response)
                .map_err(|_| EventBusError::rpc("Failed to send response".to_string()))?;
        }
        Ok(())
    }
}

impl CollectorRpcService {
    /// Create a new RPC service
    pub fn new(service_id: String, capabilities: ServiceCapabilities) -> Self {
        Self {
            service_id,
            supported_operations: capabilities.operations.clone(),
            capabilities,
        }
    }

    /// Handle an incoming RPC request
    pub async fn handle_request(&self, request: RpcRequest) -> RpcResponse {
        let start_time = SystemTime::now();

        // Validate operation is supported
        let operation = request.operation;
        if !self.supported_operations.contains(&operation) {
            return self.create_error_response(
                request,
                RpcError {
                    code: "UNSUPPORTED_OPERATION".to_string(),
                    message: format!("Operation {:?} not supported by this service", operation),
                    context: HashMap::new(),
                    category: ErrorCategory::Configuration,
                },
                start_time,
            );
        }

        // Process the request based on operation type
        match operation {
            CollectorOperation::Start => self.handle_start_request(request, start_time).await,
            CollectorOperation::Stop => self.handle_stop_request(request, start_time).await,
            CollectorOperation::Restart => self.handle_restart_request(request, start_time).await,
            CollectorOperation::HealthCheck => {
                self.handle_health_check_request(request, start_time).await
            }
            CollectorOperation::UpdateConfig => {
                self.handle_config_update_request(request, start_time).await
            }
            CollectorOperation::GetCapabilities => {
                self.handle_capabilities_request(request, start_time).await
            }
            CollectorOperation::GracefulShutdown => {
                self.handle_graceful_shutdown_request(request, start_time)
                    .await
            }
            CollectorOperation::ForceShutdown => {
                self.handle_force_shutdown_request(request, start_time)
                    .await
            }
            CollectorOperation::Pause => self.handle_pause_request(request, start_time).await,
            CollectorOperation::Resume => self.handle_resume_request(request, start_time).await,
        }
    }

    /// Handle collector start request
    async fn handle_start_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual collector start logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle collector stop request
    async fn handle_stop_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual collector stop logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle collector restart request
    async fn handle_restart_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual collector restart logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle health check request
    async fn handle_health_check_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual health check logic
        let health_data = HealthCheckData {
            collector_id: "example".to_string(),
            status: HealthStatus::Healthy,
            components: HashMap::new(),
            metrics: HashMap::new(),
            last_heartbeat: SystemTime::now(),
            uptime_seconds: 3600,
            error_count: 0,
        };

        self.create_success_response(
            request,
            Some(RpcPayload::HealthCheck(health_data)),
            start_time,
        )
    }

    /// Handle configuration update request
    async fn handle_config_update_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual configuration update logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle capabilities request
    async fn handle_capabilities_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        let capabilities_data = CapabilitiesData {
            collector_id: self.service_id.clone(),
            event_types: vec!["process".to_string()],
            operations: self.supported_operations.clone(),
            resource_requirements: ResourceRequirements {
                min_memory_bytes: 50 * 1024 * 1024, // 50MB
                min_cpu_percent: 1.0,
                required_privileges: vec![],
                required_capabilities: vec![],
            },
            platform_support: vec![
                "linux".to_string(),
                "macos".to_string(),
                "windows".to_string(),
            ],
            features: HashMap::new(),
        };

        self.create_success_response(
            request,
            Some(RpcPayload::Capabilities(capabilities_data)),
            start_time,
        )
    }

    /// Handle graceful shutdown request
    async fn handle_graceful_shutdown_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual graceful shutdown logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle force shutdown request
    async fn handle_force_shutdown_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual force shutdown logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle pause request
    async fn handle_pause_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual pause logic
        self.create_success_response(request, None, start_time)
    }

    /// Handle resume request
    async fn handle_resume_request(
        &self,
        request: RpcRequest,
        start_time: SystemTime,
    ) -> RpcResponse {
        // TODO: Implement actual resume logic
        self.create_success_response(request, None, start_time)
    }

    /// Create a success response
    fn create_success_response(
        &self,
        request: RpcRequest,
        payload: Option<RpcPayload>,
        start_time: SystemTime,
    ) -> RpcResponse {
        let execution_time = start_time.elapsed().unwrap_or(Duration::ZERO);

        RpcResponse {
            request_id: request.request_id,
            service_id: self.service_id.clone(),
            operation: request.operation,
            status: RpcStatus::Success,
            payload,
            timestamp: SystemTime::now(),
            execution_time_ms: execution_time.as_millis() as u64,
            error_details: None,
        }
    }

    /// Create an error response
    fn create_error_response(
        &self,
        request: RpcRequest,
        error: RpcError,
        start_time: SystemTime,
    ) -> RpcResponse {
        let execution_time = start_time.elapsed().unwrap_or(Duration::ZERO);

        RpcResponse {
            request_id: request.request_id,
            service_id: self.service_id.clone(),
            operation: request.operation,
            status: RpcStatus::Error,
            payload: None,
            timestamp: SystemTime::now(),
            execution_time_ms: execution_time.as_millis() as u64,
            error_details: Some(error),
        }
    }
}

impl CollectorLifecycleRequest {
    /// Create a start collector request
    pub fn start(
        collector_id: &str,
        config_overrides: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            collector_id: collector_id.to_string(),
            collector_type: collector_id.to_string(), // Default to same as ID
            config_overrides,
            environment: None,
            working_directory: None,
            resource_limits: None,
            startup_timeout_ms: Some(30000), // 30 seconds default
        }
    }

    /// Create a stop collector request
    pub fn stop(collector_id: &str) -> Self {
        Self {
            collector_id: collector_id.to_string(),
            collector_type: collector_id.to_string(),
            config_overrides: None,
            environment: None,
            working_directory: None,
            resource_limits: None,
            startup_timeout_ms: None,
        }
    }

    /// Create a restart collector request
    pub fn restart(
        collector_id: &str,
        config_overrides: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            collector_id: collector_id.to_string(),
            collector_type: collector_id.to_string(),
            config_overrides,
            environment: None,
            working_directory: None,
            resource_limits: None,
            startup_timeout_ms: Some(30000),
        }
    }
}

impl RpcRequest {
    /// Create a new RPC request
    pub fn new(
        client_id: String,
        target: String,
        operation: CollectorOperation,
        payload: RpcPayload,
        timeout: Duration,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            client_id,
            target,
            operation,
            payload,
            timestamp: SystemTime::now(),
            timeout_ms: timeout.as_millis() as u64,
            correlation_id: Uuid::new_v4().to_string(),
        }
    }

    /// Create a lifecycle request
    pub fn lifecycle(
        client_id: String,
        target: String,
        operation: CollectorOperation,
        lifecycle_request: CollectorLifecycleRequest,
        timeout: Duration,
    ) -> Self {
        Self::new(
            client_id,
            target,
            operation,
            RpcPayload::Lifecycle(lifecycle_request),
            timeout,
        )
    }

    /// Create a health check request
    pub fn health_check(client_id: String, target: String, timeout: Duration) -> Self {
        Self::new(
            client_id,
            target,
            CollectorOperation::HealthCheck,
            RpcPayload::Empty,
            timeout,
        )
    }

    /// Create a configuration update request
    pub fn config_update(
        client_id: String,
        target: String,
        config_request: ConfigUpdateRequest,
        timeout: Duration,
    ) -> Self {
        Self::new(
            client_id,
            target,
            CollectorOperation::UpdateConfig,
            RpcPayload::ConfigUpdate(config_request),
            timeout,
        )
    }

    /// Create a capabilities request
    pub fn capabilities(client_id: String, target: String, timeout: Duration) -> Self {
        Self::new(
            client_id,
            target,
            CollectorOperation::GetCapabilities,
            RpcPayload::Empty,
            timeout,
        )
    }

    /// Create a shutdown request
    pub fn shutdown(
        client_id: String,
        target: String,
        shutdown_request: ShutdownRequest,
        timeout: Duration,
    ) -> Self {
        let operation = match shutdown_request.shutdown_type {
            ShutdownType::Graceful => CollectorOperation::GracefulShutdown,
            ShutdownType::Immediate | ShutdownType::Emergency => CollectorOperation::ForceShutdown,
        };

        Self::new(
            client_id,
            target,
            operation,
            RpcPayload::Shutdown(shutdown_request),
            timeout,
        )
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

impl Default for RpcStatus {
    fn default() -> Self {
        Self::Success
    }
}

impl Default for ShutdownType {
    fn default() -> Self {
        Self::Graceful
    }
}

impl Default for ErrorCategory {
    fn default() -> Self {
        Self::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_creation() {
        let request = RpcRequest::health_check(
            "test-client".to_string(),
            "control.collector.procmond".to_string(),
            Duration::from_secs(10),
        );

        assert_eq!(request.client_id, "test-client");
        assert_eq!(request.target, "control.collector.procmond");
        assert_eq!(request.operation, CollectorOperation::HealthCheck);
        assert_eq!(request.timeout_ms, 10000);
    }

    #[test]
    fn test_lifecycle_request_creation() {
        let lifecycle_req = CollectorLifecycleRequest::start("procmond", None);
        assert_eq!(lifecycle_req.collector_id, "procmond");
        assert_eq!(lifecycle_req.collector_type, "procmond");
        assert_eq!(lifecycle_req.startup_timeout_ms, Some(30000));
    }

    #[test]
    fn test_rpc_response_serialization() {
        let response = RpcResponse {
            request_id: "test-request".to_string(),
            service_id: "test-service".to_string(),
            operation: CollectorOperation::HealthCheck,
            status: RpcStatus::Success,
            payload: None,
            timestamp: SystemTime::now(),
            execution_time_ms: 100,
            error_details: None,
        };

        let serialized = bincode::serde::encode_to_vec(&response, bincode::config::standard());
        assert!(serialized.is_ok());

        let deserialized =
            bincode::serde::decode_from_slice(&serialized.unwrap(), bincode::config::standard());
        assert!(deserialized.is_ok());
        let (deserialized_response, _): (RpcResponse, _) = deserialized.unwrap();
        assert_eq!(deserialized_response.service_id, "test-service");
        assert_eq!(deserialized_response.status, RpcStatus::Success);
    }

    #[tokio::test]
    async fn test_rpc_service_creation() {
        let capabilities = ServiceCapabilities {
            operations: vec![
                CollectorOperation::Start,
                CollectorOperation::Stop,
                CollectorOperation::HealthCheck,
            ],
            max_concurrent_requests: 10,
            timeout_limits: TimeoutLimits {
                min_timeout_ms: 1000,
                max_timeout_ms: 300000,
                default_timeout_ms: 30000,
            },
            supported_collectors: vec!["procmond".to_string()],
        };

        let service = CollectorRpcService::new("test-service".to_string(), capabilities);
        assert_eq!(service.service_id, "test-service");
        assert_eq!(service.supported_operations.len(), 3);
    }
}
