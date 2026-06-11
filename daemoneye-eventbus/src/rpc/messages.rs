//! Request/response and data types for collector RPC operations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// Result of a configuration update operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdateResult {
    pub version: u64,
    pub changed_fields: Vec<String>,
    pub restart_performed: bool,
    pub timestamp: SystemTime,
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
    /// Absolute deadline for this request to be completed
    pub deadline: SystemTime,
    /// Rich correlation metadata for distributed tracing and workflow tracking
    pub correlation_metadata: RpcCorrelationMetadata,
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
    /// Time the request spent queued before processing in milliseconds
    pub queue_time_ms: Option<u64>,
    /// Total end-to-end time from request to response in milliseconds
    pub total_time_ms: u64,
    /// Error details if status is Error
    pub error_details: Option<RpcError>,
    /// Rich correlation metadata echoed/updated from the request
    pub correlation_metadata: RpcCorrelationMetadata,
}

/// Correlation metadata carried with every RPC to enable distributed tracing and
/// hierarchical workflow tracking.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RpcCorrelationMetadata {
    /// Primary correlation ID for this RPC call
    pub correlation_id: String,
    /// Parent RPC call ID for hierarchical tracking
    pub parent_correlation_id: Option<String>,
    /// Root workflow correlation ID
    pub root_correlation_id: String,
    /// Distributed tracing trace ID (OpenTelemetry compatible)
    pub trace_id: Option<String>,
    /// Distributed tracing span ID
    pub span_id: Option<String>,
    /// Sequence number within correlation group
    pub sequence_number: u64,
    /// Current workflow stage (e.g., "collection", "analysis", "alerting")
    pub workflow_stage: Option<String>,
    /// Workflow identifier for multi-step processes
    pub workflow_id: Option<String>,
    /// Number of RPC hops in the call chain
    pub hop_count: u32,
    /// Custom tags for flexible grouping
    pub correlation_tags: HashMap<String, String>,
}

impl RpcCorrelationMetadata {
    /// Create new correlation metadata with required `correlation_id`; sets `root_correlation_id` to the same value.
    pub fn new(correlation_id: String) -> Self {
        Self {
            root_correlation_id: correlation_id.clone(),
            correlation_id,
            parent_correlation_id: None,
            trace_id: None,
            span_id: None,
            sequence_number: 0,
            workflow_stage: None,
            workflow_id: None,
            hop_count: 0,
            correlation_tags: HashMap::new(),
        }
    }

    /// Attach a parent correlation id.
    #[must_use]
    pub fn with_parent(mut self, parent_id: String) -> Self {
        self.parent_correlation_id = Some(parent_id);
        self
    }

    /// Set tracing IDs.
    #[must_use]
    pub fn with_trace(mut self, trace_id: String, span_id: String) -> Self {
        self.trace_id = Some(trace_id);
        self.span_id = Some(span_id);
        self
    }

    /// Set workflow identifiers.
    #[must_use]
    pub fn with_workflow(mut self, workflow_id: String, stage: String) -> Self {
        self.workflow_id = Some(workflow_id);
        self.workflow_stage = Some(stage);
        self
    }

    /// Add a tag.
    #[must_use]
    pub fn with_tag(mut self, key: String, value: String) -> Self {
        self.correlation_tags.insert(key, value);
        self
    }

    /// Increment hop count for cascading calls.
    #[must_use]
    pub const fn increment_hop(mut self) -> Self {
        self.hop_count = self.hop_count.saturating_add(1);
        self
    }
}

/// RPC operation types for collector lifecycle management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CollectorOperation {
    /// Register a collector with the broker
    Register,
    /// Deregister a collector from the broker
    Deregister,
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
    /// Execute a specific task (e.g. detection)
    ExecuteTask,
}

/// RPC payload containing operation-specific data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RpcPayload {
    /// Collector lifecycle request
    Lifecycle(CollectorLifecycleRequest),
    /// Collector registration request
    Registration(RegistrationRequest),
    /// Collector registration acknowledgment/response
    RegistrationResponse(RegistrationResponse),
    /// Collector deregistration request
    Deregistration(DeregistrationRequest),
    /// Health check request/response
    HealthCheck(HealthCheckData),
    /// Configuration update request
    ConfigUpdate(ConfigUpdateRequest),
    /// Capabilities request/response
    Capabilities(CapabilitiesData),
    /// Shutdown coordination request
    Shutdown(ShutdownRequest),
    /// Generic task execution request
    Task(serde_json::Value),
    /// Generic task execution result
    TaskResult(serde_json::Value),
    /// Generic key-value payload
    Generic(HashMap<String, serde_json::Value>),
    /// Empty payload
    Empty,
}

/// RPC response status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum RpcStatus {
    /// Operation completed successfully
    #[default]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
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
    #[default]
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

/// Collector registration request data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationRequest {
    /// Collector identifier requested by the collector
    pub collector_id: String,
    /// Collector implementation type (procmond, netmond, etc.)
    pub collector_type: String,
    /// Hostname of the system running the collector
    pub hostname: String,
    /// Optional software version reported by the collector
    pub version: Option<String>,
    /// Process ID of the collector if available
    pub pid: Option<u32>,
    /// Declared capabilities supported by the collector
    pub capabilities: Vec<String>,
    /// Arbitrary metadata associated with the collector
    pub attributes: HashMap<String, serde_json::Value>,
    /// Requested heartbeat interval in milliseconds
    pub heartbeat_interval_ms: Option<u64>,
}

/// Collector registration response payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationResponse {
    /// Collector identifier that was registered
    pub collector_id: String,
    /// Whether the registration request was accepted
    pub accepted: bool,
    /// Heartbeat interval assigned by the registry in milliseconds
    pub heartbeat_interval_ms: u64,
    /// Topics or channels assigned to the collector
    pub assigned_topics: Vec<String>,
    /// Optional message from the registry (error or informational)
    pub message: Option<String>,
}

/// Collector deregistration request payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeregistrationRequest {
    /// Collector identifier
    pub collector_id: String,
    /// Optional reason for deregistration
    pub reason: Option<String>,
    /// Whether the deregistration is forced (e.g., shutdown)
    pub force: bool,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[non_exhaustive]
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
    #[default]
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

/// Configuration change notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeNotification {
    /// Collector identifier
    pub collector_id: String,
    /// List of changed field names
    pub changed_fields: Vec<String>,
    /// Configuration version number
    pub version: u64,
    /// Timestamp when change occurred
    pub timestamp: u64,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum ShutdownType {
    /// Graceful shutdown with cleanup
    #[default]
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

impl CollectorLifecycleRequest {
    /// Create a start collector request
    pub fn start(
        collector_id: &str,
        config_overrides: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            collector_id: collector_id.to_owned(),
            collector_type: collector_id.to_owned(), // Default to same as ID
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
            collector_id: collector_id.to_owned(),
            collector_type: collector_id.to_owned(),
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
            collector_id: collector_id.to_owned(),
            collector_type: collector_id.to_owned(),
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
        let now = SystemTime::now();
        let deadline = now.checked_add(timeout).unwrap_or(now);
        let correlation_metadata = RpcCorrelationMetadata::new(Uuid::new_v4().to_string());

        Self {
            request_id: Uuid::new_v4().to_string(),
            client_id,
            target,
            operation,
            payload,
            timestamp: now,
            deadline,
            correlation_metadata,
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

    /// Create a registration request
    pub fn register(
        client_id: String,
        target: String,
        registration_request: RegistrationRequest,
        timeout: Duration,
    ) -> Self {
        Self::new(
            client_id,
            target,
            CollectorOperation::Register,
            RpcPayload::Registration(registration_request),
            timeout,
        )
    }

    /// Create a deregistration request
    pub fn deregister(
        client_id: String,
        target: String,
        deregistration_request: DeregistrationRequest,
        timeout: Duration,
    ) -> Self {
        Self::new(
            client_id,
            target,
            CollectorOperation::Deregister,
            RpcPayload::Deregistration(deregistration_request),
            timeout,
        )
    }

    /// Create a health check request
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client identifier making the request
    /// * `target` - The target topic (e.g., "control.collector.test-collector")
    /// * `timeout` - Request timeout duration
    ///
    /// Note: The `collector_id` is inferred from the service handling the request.
    /// When sending to a specific collector's topic, the collector will use its own ID.
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
