// Busrt message broker types and traits for DaemonEye
// This module defines the core types and interfaces for busrt integration

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Topic hierarchy constants for busrt message routing
pub mod topics {
    // Event topics for collector data distribution
    pub const EVENTS_PROCESS_ENUMERATION: &str = "events/process/enumeration";
    pub const EVENTS_PROCESS_LIFECYCLE: &str = "events/process/lifecycle";
    pub const EVENTS_PROCESS_METADATA: &str = "events/process/metadata";
    pub const EVENTS_PROCESS_HASH: &str = "events/process/hash";
    pub const EVENTS_PROCESS_ANOMALY: &str = "events/process/anomaly";

    // Future network monitoring topics
    pub const EVENTS_NETWORK_CONNECTIONS: &str = "events/network/connections";
    pub const EVENTS_NETWORK_TRAFFIC: &str = "events/network/traffic";
    pub const EVENTS_NETWORK_DNS: &str = "events/network/dns";
    pub const EVENTS_NETWORK_ANOMALY: &str = "events/network/anomaly";

    // Future filesystem monitoring topics
    pub const EVENTS_FILESYSTEM_OPERATIONS: &str = "events/filesystem/operations";
    pub const EVENTS_FILESYSTEM_ACCESS: &str = "events/filesystem/access";
    pub const EVENTS_FILESYSTEM_METADATA: &str = "events/filesystem/metadata";
    pub const EVENTS_FILESYSTEM_ANOMALY: &str = "events/filesystem/anomaly";

    // Future performance monitoring topics
    pub const EVENTS_PERFORMANCE_METRICS: &str = "events/performance/metrics";
    pub const EVENTS_PERFORMANCE_RESOURCES: &str = "events/performance/resources";
    pub const EVENTS_PERFORMANCE_THRESHOLDS: &str = "events/performance/thresholds";
    pub const EVENTS_PERFORMANCE_ANOMALY: &str = "events/performance/anomaly";

    // Control topics for collector management
    pub const CONTROL_COLLECTOR_LIFECYCLE: &str = "control/collector/lifecycle";
    pub const CONTROL_COLLECTOR_CONFIG: &str = "control/collector/config";
    pub const CONTROL_COLLECTOR_HEALTH: &str = "control/collector/health";
    pub const CONTROL_COLLECTOR_CAPABILITIES: &str = "control/collector/capabilities";

    // Agent coordination topics
    pub const CONTROL_AGENT_TASKS: &str = "control/agent/tasks";
    pub const CONTROL_AGENT_COORDINATION: &str = "control/agent/coordination";
    pub const CONTROL_AGENT_STATUS: &str = "control/agent/status";
    pub const CONTROL_AGENT_SHUTDOWN: &str = "control/agent/shutdown";

    // Broker administrative topics
    pub const CONTROL_BROKER_STATS: &str = "control/broker/stats";
    pub const CONTROL_BROKER_ADMIN: &str = "control/broker/admin";
    pub const CONTROL_BROKER_DIAGNOSTICS: &str = "control/broker/diagnostics";
}

/// Busrt event message with correlation metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusrtEvent {
    /// Unique event identifier
    pub event_id: String,
    /// Correlation ID for related events
    pub correlation_id: Option<String>,
    /// Event timestamp in milliseconds since epoch
    pub timestamp_ms: i64,
    /// Originating collector identifier
    pub source_collector: String,
    /// Busrt topic for routing
    pub topic: String,
    /// Event payload
    pub payload: EventPayload,
    /// Event correlation metadata
    pub correlation: Option<EventCorrelation>,
}

/// Event payload variants for different collector types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventPayload {
    Process(ProcessEventData),
    Network(NetworkEventData),
    Filesystem(FilesystemEventData),
    Performance(PerformanceEventData),
}

/// Process event data with busrt metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEventData {
    /// Process record from existing protobuf definitions
    pub process: ProcessRecord,
    /// Collection cycle identifier
    pub collection_cycle_id: String,
    /// Event type classification
    pub event_type: ProcessEventType,
}

/// Process event type classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessEventType {
    Discovery,   // Process discovered
    Update,      // Process metadata updated
    Termination, // Process terminated
    Anomaly,     // Anomalous behavior detected
}

/// Network event data (future implementation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEventData {
    pub connection_id: String,
    pub event_type: NetworkEventType,
    // Additional network-specific fields
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEventType {
    ConnectionEstablished,
    ConnectionClosed,
    TrafficAnomaly,
    DnsQuery,
}

/// Filesystem event data (future implementation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemEventData {
    pub file_path: String,
    pub event_type: FilesystemEventType,
    // Additional filesystem-specific fields
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesystemEventType {
    FileCreated,
    FileModified,
    FileDeleted,
    AccessAnomaly,
}

/// Performance event data (future implementation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceEventData {
    pub metric_name: String,
    pub event_type: PerformanceEventType,
    // Additional performance-specific fields
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PerformanceEventType {
    MetricUpdate,
    ThresholdViolation,
    ResourceExhaustion,
    PerformanceAnomaly,
}

/// Event correlation metadata for multi-collector workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCorrelation {
    /// Multi-step workflow identifier
    pub workflow_id: String,
    /// Related event IDs
    pub related_events: Vec<String>,
    /// Workflow context data
    pub context: HashMap<String, String>,
}

/// RPC service trait for collector lifecycle management
#[async_trait]
pub trait CollectorLifecycleService: Send + Sync {
    /// Start a collector with specified configuration
    async fn start_collector(
        &self,
        request: StartCollectorRequest,
    ) -> Result<StartCollectorResponse, BusrtError>;

    /// Stop a running collector gracefully or forcefully
    async fn stop_collector(
        &self,
        request: StopCollectorRequest,
    ) -> Result<StopCollectorResponse, BusrtError>;

    /// Restart a collector with optional new configuration
    async fn restart_collector(
        &self,
        request: RestartCollectorRequest,
    ) -> Result<RestartCollectorResponse, BusrtError>;

    /// Get current status of a collector
    async fn get_collector_status(
        &self,
        request: StatusRequest,
    ) -> Result<StatusResponse, BusrtError>;

    /// List all managed collectors
    async fn list_collectors(
        &self,
        request: ListCollectorsRequest,
    ) -> Result<ListCollectorsResponse, BusrtError>;

    /// Get collector capabilities and metadata
    async fn get_collector_capabilities(
        &self,
        request: CapabilitiesRequest,
    ) -> Result<CapabilitiesResponse, BusrtError>;

    /// Pause a collector temporarily
    async fn pause_collector(
        &self,
        request: PauseCollectorRequest,
    ) -> Result<PauseCollectorResponse, BusrtError>;

    /// Resume a paused collector
    async fn resume_collector(
        &self,
        request: ResumeCollectorRequest,
    ) -> Result<ResumeCollectorResponse, BusrtError>;
}

/// RPC service trait for health monitoring
#[async_trait]
pub trait HealthCheckService: Send + Sync {
    async fn health_check(
        &self,
        request: HealthCheckRequest,
    ) -> Result<HealthCheckResponse, BusrtError>;
    async fn heartbeat(&self, request: HeartbeatRequest) -> Result<HeartbeatResponse, BusrtError>;
    async fn get_metrics(&self, request: MetricsRequest) -> Result<MetricsResponse, BusrtError>;
}

/// RPC service trait for configuration management
#[async_trait]
pub trait ConfigurationService: Send + Sync {
    async fn update_config(
        &self,
        request: UpdateConfigRequest,
    ) -> Result<UpdateConfigResponse, BusrtError>;
    async fn get_config(&self, request: GetConfigRequest) -> Result<GetConfigResponse, BusrtError>;
    async fn validate_config(
        &self,
        request: ValidateConfigRequest,
    ) -> Result<ValidateConfigResponse, BusrtError>;
}

/// Start collector RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartCollectorRequest {
    pub collector_type: String,
    pub collector_id: String,
    pub config: CollectorConfig,
    pub environment: HashMap<String, String>,
}

/// Start collector RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartCollectorResponse {
    pub success: bool,
    pub collector_id: String,
    pub error_message: Option<String>,
    pub status: CollectorStatus,
}

/// Stop collector RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopCollectorRequest {
    pub collector_id: String,
    pub force: bool,
    pub timeout_seconds: u32,
}

/// Stop collector RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopCollectorResponse {
    pub success: bool,
    pub error_message: Option<String>,
    pub shutdown_duration_ms: u32,
}

/// Restart collector RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartCollectorRequest {
    pub collector_id: String,
    pub new_config: Option<CollectorConfig>,
    pub timeout_seconds: u32,
}

/// Restart collector RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartCollectorResponse {
    pub success: bool,
    pub error_message: Option<String>,
    pub new_collector_id: String,
}

/// Status request for collector information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {
    pub collector_id: String,
}

/// Status response with collector information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub collector_id: String,
    pub status: CollectorStatus,
    pub uptime_seconds: u64,
    pub last_activity: i64,
    pub error_message: Option<String>,
    pub resource_usage: ResourceUsage,
}

/// List collectors RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCollectorsRequest {
    pub filter_status: Option<CollectorStatus>,
    pub filter_type: Option<String>,
}

/// List collectors RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCollectorsResponse {
    pub collectors: Vec<CollectorInfo>,
    pub total_count: usize,
}

/// Collector information summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorInfo {
    pub collector_id: String,
    pub collector_type: String,
    pub status: CollectorStatus,
    pub uptime_seconds: u64,
    pub last_activity: i64,
    pub capabilities: Vec<String>,
    pub resource_usage: ResourceUsage,
}

/// Capabilities request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesRequest {
    pub collector_id: String,
}

/// Capabilities response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub collector_id: String,
    pub capabilities: Vec<String>,
    pub supported_operations: Vec<String>,
    pub resource_limits: ResourceLimits,
    pub metadata: HashMap<String, String>,
}

/// Pause collector RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseCollectorRequest {
    pub collector_id: String,
    pub reason: String,
}

/// Pause collector RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseCollectorResponse {
    pub success: bool,
    pub error_message: Option<String>,
    pub paused_at: i64,
}

/// Resume collector RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeCollectorRequest {
    pub collector_id: String,
}

/// Resume collector RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeCollectorResponse {
    pub success: bool,
    pub error_message: Option<String>,
    pub resumed_at: i64,
}

/// Resource usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub disk_io_bytes: u64,
    pub network_io_bytes: u64,
    pub open_files: u32,
}

/// Resource limits configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_cpu_percent: Option<f64>,
    pub max_memory_bytes: Option<u64>,
    pub max_disk_io_bytes_per_sec: Option<u64>,
    pub max_network_io_bytes_per_sec: Option<u64>,
    pub max_open_files: Option<u32>,
}

/// Health check RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckRequest {
    pub collector_id: String,
    pub include_metrics: bool,
}

/// Health check RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    pub status: HealthStatus,
    pub message: String,
    pub metrics: HashMap<String, f64>,
    pub uptime_seconds: u64,
}

/// Heartbeat RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub collector_id: String,
}

/// Heartbeat RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub timestamp: i64,
    pub status: HealthStatus,
}

/// Metrics request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsRequest {
    pub collector_id: String,
    pub metric_names: Option<Vec<String>>,
}

/// Metrics response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub metrics: HashMap<String, f64>,
    pub timestamp: i64,
}

/// Configuration update RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigRequest {
    pub collector_id: String,
    pub new_config: CollectorConfig,
    pub validate_only: bool,
}

/// Configuration update RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigResponse {
    pub success: bool,
    pub error_message: Option<String>,
    pub validation_errors: Vec<ConfigValidationError>,
}

/// Get configuration RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetConfigRequest {
    pub collector_id: String,
}

/// Get configuration RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetConfigResponse {
    pub config: CollectorConfig,
}

/// Validate configuration RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateConfigRequest {
    pub config: CollectorConfig,
}

/// Validate configuration RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateConfigResponse {
    pub valid: bool,
    pub validation_errors: Vec<ConfigValidationError>,
}

/// Collector configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    pub collector_type: String,
    pub scan_interval_ms: u64,
    pub batch_size: usize,
    pub timeout_seconds: u32,
    pub capabilities: Vec<String>,
    pub settings: HashMap<String, serde_json::Value>,
}

/// Collector status enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CollectorStatus {
    /// Collector is initializing
    Starting,
    /// Collector is running normally
    Running,
    /// Collector is paused temporarily
    Paused,
    /// Collector is shutting down gracefully
    Stopping,
    /// Collector has stopped
    Stopped,
    /// Collector is in error state
    Error,
    /// Collector is restarting
    Restarting,
    /// Collector status is unknown
    Unknown,
}

/// Health status enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Configuration validation error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValidationError {
    pub field: String,
    pub message: String,
    pub error_code: String,
}

/// Busrt-specific error types
#[derive(Debug, thiserror::Error)]
pub enum BusrtError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Message serialization failed: {0}")]
    SerializationFailed(String),

    #[error("RPC timeout: {0}")]
    RpcTimeout(String),

    #[error("Topic not found: {0}")]
    TopicNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Broker unavailable: {0}")]
    BrokerUnavailable(String),
}

/// Busrt client trait for message broker interaction
#[async_trait]
pub trait BusrtClient: Send + Sync {
    /// Publish a message to a topic
    async fn publish(&self, topic: &str, message: BusrtEvent) -> Result<(), BusrtError>;

    /// Subscribe to a topic pattern
    async fn subscribe(&self, pattern: &str) -> Result<mpsc::Receiver<BusrtEvent>, BusrtError>;

    /// Unsubscribe from a topic pattern
    async fn unsubscribe(&self, pattern: &str) -> Result<(), BusrtError>;

    /// Make an RPC call with JSON serialization
    async fn rpc_call_json(
        &self,
        service: &str,
        method: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, BusrtError>;

    /// Get broker statistics
    async fn get_broker_stats(&self) -> Result<BrokerStats, BusrtError>;
}

/// Helper trait for typed RPC calls (separate from dyn-compatible trait)
pub trait BusrtClientExt: BusrtClient {
    /// Make a typed RPC call
    fn rpc_call<T, R>(
        &self,
        service: &str,
        method: &str,
        request: T,
    ) -> impl std::future::Future<Output = Result<R, BusrtError>> + Send
    where
        T: Serialize + Send,
        R: for<'de> Deserialize<'de>,
    {
        async move {
            let request_json = serde_json::to_value(request)
                .map_err(|e| BusrtError::SerializationFailed(e.to_string()))?;

            let response_json = self.rpc_call_json(service, method, request_json).await?;

            serde_json::from_value(response_json)
                .map_err(|e| BusrtError::SerializationFailed(e.to_string()))
        }
    }
}

// Blanket implementation for all BusrtClient implementors
impl<T: BusrtClient> BusrtClientExt for T {}

/// Broker statistics for monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerStats {
    pub uptime_seconds: u64,
    pub total_connections: u64,
    pub active_connections: u64,
    pub messages_published: u64,
    pub messages_delivered: u64,
    pub topics_count: u64,
    pub memory_usage_bytes: u64,
}

/// Broker configuration for embedded and standalone modes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerConfig {
    pub mode: BrokerMode,
    pub embedded: Option<EmbeddedBrokerConfig>,
    pub standalone: Option<StandaloneBrokerConfig>,
}

/// Broker deployment mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrokerMode {
    Embedded,
    Standalone,
}

/// Embedded broker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedBrokerConfig {
    pub max_connections: usize,
    pub message_buffer_size: usize,
    pub transport: TransportConfig,
    pub security: SecurityConfig,
}

/// Standalone broker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandaloneBrokerConfig {
    pub bind_address: String,
    pub max_connections: usize,
    pub persistence: PersistenceConfig,
    pub clustering: ClusterConfig,
    pub monitoring: MonitoringConfig,
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub transport_type: TransportType,
    pub path: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
}

/// Transport type enumeration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransportType {
    UnixSocket,
    NamedPipe,
    Tcp,
    InProcess,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityConfig {
    pub authentication_enabled: bool,
    pub tls_enabled: bool,
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    pub ca_file: Option<String>,
}

/// Persistence configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceConfig {
    pub enabled: bool,
    pub storage_path: String,
    pub max_message_size: usize,
    pub retention_hours: u32,
}

/// Clustering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub enabled: bool,
    pub node_id: String,
    pub cluster_nodes: Vec<String>,
    pub election_timeout_ms: u32,
}

/// Monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub metrics_enabled: bool,
    pub metrics_port: u16,
    pub health_check_port: u16,
    pub log_level: String,
}

/// Compatibility layer for crossbeam migration
pub struct CrossbeamCompatibilityLayer {
    busrt_client: Box<dyn BusrtClient>,
    topic_mapping: HashMap<String, String>,
}

impl CrossbeamCompatibilityLayer {
    /// Create new compatibility layer
    pub fn new(busrt_client: Box<dyn BusrtClient>) -> Self {
        let mut topic_mapping = HashMap::new();

        // Map existing event types to busrt topics
        topic_mapping.insert(
            "process".to_string(),
            topics::EVENTS_PROCESS_ENUMERATION.to_string(),
        );
        topic_mapping.insert(
            "network".to_string(),
            topics::EVENTS_NETWORK_CONNECTIONS.to_string(),
        );
        topic_mapping.insert(
            "filesystem".to_string(),
            topics::EVENTS_FILESYSTEM_OPERATIONS.to_string(),
        );
        topic_mapping.insert(
            "performance".to_string(),
            topics::EVENTS_PERFORMANCE_METRICS.to_string(),
        );

        Self {
            busrt_client,
            topic_mapping,
        }
    }

    /// Map event to appropriate busrt topic
    pub fn map_event_to_topic(&self, event_type: &str) -> String {
        self.topic_mapping
            .get(event_type)
            .cloned()
            .unwrap_or_else(|| format!("events/{}/default", event_type))
    }

    /// Send event with crossbeam semantics
    pub async fn send(&self, event_type: &str, event: BusrtEvent) -> Result<(), BusrtError> {
        let topic = self.map_event_to_topic(event_type);
        self.busrt_client.publish(&topic, event).await
    }

    /// Receive events with crossbeam semantics
    pub async fn receive(
        &self,
        event_type: &str,
    ) -> Result<mpsc::Receiver<BusrtEvent>, BusrtError> {
        let topic = self.map_event_to_topic(event_type);
        self.busrt_client.subscribe(&topic).await
    }
}

// Re-export ProcessRecord from existing crate structure
// This will be replaced with actual import once integrated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: u32,
    pub name: String,
    pub executable_path: Option<String>,
    // Additional fields from existing protobuf definitions
}
