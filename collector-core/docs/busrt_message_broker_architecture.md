# Busrt Message Broker Architecture for DaemonEye

## Overview

This document defines the busrt message broker architecture for DaemonEye, replacing the current crossbeam-based event bus with an industrial-grade message broker that supports multi-process communication and future scalability.

## Topic Hierarchy Design

### Event Topics

The event topic hierarchy follows a domain-based structure that enables flexible subscription patterns and clear separation of concerns:

```
events/
├── process/
│   ├── enumeration      # Process discovery events
│   ├── lifecycle        # Process start/stop events
│   ├── metadata         # Process metadata updates
│   ├── hash             # Executable hash computation results
│   └── anomaly          # Process anomaly detection events
├── network/             # Future: Network monitoring events
│   ├── connections      # Network connection events
│   ├── traffic          # Network traffic analysis
│   ├── dns              # DNS query monitoring
│   └── anomaly          # Network anomaly detection
├── filesystem/          # Future: Filesystem monitoring events
│   ├── operations       # File system operations
│   ├── access           # File access patterns
│   ├── metadata         # File metadata changes
│   └── anomaly          # Filesystem anomaly detection
└── performance/         # Future: Performance monitoring events
    ├── metrics          # System performance metrics
    ├── resources        # Resource utilization
    ├── thresholds       # Threshold violations
    └── anomaly          # Performance anomaly detection
```

### Control Topics

Control topics manage collector lifecycle, configuration, and coordination:

```
control/
├── collector/
│   ├── lifecycle        # Start/stop/restart commands
│   ├── config           # Configuration updates
│   ├── health           # Health check requests
│   └── capabilities     # Capability advertisement
├── agent/
│   ├── tasks            # Detection task distribution
│   ├── coordination     # Multi-collector coordination
│   ├── status           # Agent status updates
│   └── shutdown         # Graceful shutdown coordination
└── broker/
    ├── stats            # Broker statistics and metrics
    ├── admin            # Administrative commands
    └── diagnostics      # Diagnostic information
```

### Topic Naming Conventions

- **Hierarchical Structure**: Use forward slashes (`/`) for topic hierarchy
- **Lowercase**: All topic names use lowercase with underscores for readability
- **Descriptive Names**: Topic names clearly indicate the type of data or operation
- **Wildcarding Support**: Topics support busrt wildcarding patterns:
  - `events/process/*` - Subscribe to all process events
  - `events/*/anomaly` - Subscribe to all anomaly events across domains
  - `control/collector/+` - Subscribe to direct collector control messages

### Topic Access Patterns

#### Publisher Patterns

- **Collectors**: Publish to `events/{domain}/*` topics
- **daemoneye-agent**: Publishes to `control/collector/*` and `control/agent/*`
- **Broker**: Publishes to `control/broker/*` for administrative messages

#### Subscriber Patterns

- **daemoneye-agent**: Subscribes to `events/*/*` for all collector events
- **Collectors**: Subscribe to `control/collector/*` for lifecycle management
- **Monitoring Systems**: Subscribe to `events/*/anomaly` for security events

### Security Boundaries

- **Topic Isolation**: Each collector type has dedicated event topics
- **Control Separation**: Control topics are separate from event topics
- **Access Control**: Future enterprise features will implement topic-based ACLs
- **Audit Trail**: All control messages are logged for security auditing

## RPC Call Patterns

### Collector Lifecycle Management

#### Service Definitions

```rust
// Collector lifecycle RPC service
pub trait CollectorLifecycleService {
    async fn start_collector(
        &self,
        request: StartCollectorRequest,
    ) -> Result<StartCollectorResponse>;
    async fn stop_collector(&self, request: StopCollectorRequest) -> Result<StopCollectorResponse>;
    async fn restart_collector(
        &self,
        request: RestartCollectorRequest,
    ) -> Result<RestartCollectorResponse>;
    async fn get_collector_status(&self, request: StatusRequest) -> Result<StatusResponse>;
}

// Health check RPC service
pub trait HealthCheckService {
    async fn health_check(&self, request: HealthCheckRequest) -> Result<HealthCheckResponse>;
    async fn heartbeat(&self, request: HeartbeatRequest) -> Result<HeartbeatResponse>;
    async fn get_metrics(&self, request: MetricsRequest) -> Result<MetricsResponse>;
}

// Configuration management RPC service
pub trait ConfigurationService {
    async fn update_config(&self, request: UpdateConfigRequest) -> Result<UpdateConfigResponse>;
    async fn get_config(&self, request: GetConfigRequest) -> Result<GetConfigResponse>;
    async fn validate_config(
        &self,
        request: ValidateConfigRequest,
    ) -> Result<ValidateConfigResponse>;
}
```

#### RPC Call Patterns

1. **Synchronous Lifecycle Operations**

   - `start_collector()` - Start a collector with configuration
   - `stop_collector()` - Gracefully stop a collector
   - `restart_collector()` - Restart collector with new configuration

2. **Asynchronous Health Monitoring**

   - `heartbeat()` - Periodic health check with minimal overhead
   - `health_check()` - Comprehensive health assessment
   - `get_metrics()` - Performance and operational metrics

3. **Configuration Management**

   - `update_config()` - Dynamic configuration updates
   - `validate_config()` - Configuration validation before application
   - `get_config()` - Current configuration retrieval

### RPC Timeout and Retry Policies

- **Lifecycle Operations**: 30-second timeout with no retries (idempotent)
- **Health Checks**: 5-second timeout with 3 retries
- **Configuration Updates**: 15-second timeout with exponential backoff
- **Heartbeats**: 2-second timeout with immediate retry

## Message Schemas

### Event Message Schema

Extending existing protobuf definitions for busrt pub/sub:

```protobuf
syntax = "proto3";

// Enhanced event message for busrt distribution
message BusrtEvent {
    // Event metadata
    string event_id = 1;           // Unique event identifier
    string correlation_id = 2;     // Correlation across related events
    int64 timestamp_ms = 3;        // Event timestamp in milliseconds
    string source_collector = 4;   // Originating collector identifier
    string topic = 5;              // Busrt topic for routing

    // Event payload (one of)
    oneof payload {
        ProcessEvent process_event = 10;
        NetworkEvent network_event = 11;      // Future
        FilesystemEvent filesystem_event = 12; // Future
        PerformanceEvent performance_event = 13; // Future
    }

    // Event correlation metadata
    EventCorrelation correlation = 20;
}

// Event correlation for multi-collector workflows
message EventCorrelation {
    string workflow_id = 1;        // Multi-step workflow identifier
    repeated string related_events = 2; // Related event IDs
    map<string, string> context = 3;    // Workflow context data
}

// Enhanced process event with busrt metadata
message ProcessEvent {
    // Existing ProcessRecord fields
    ProcessRecord process = 1;

    // Busrt-specific metadata
    string collection_cycle_id = 2; // Collection cycle identifier
    EventType event_type = 3;       // Event classification

    enum EventType {
        DISCOVERY = 0;     // Process discovered
        UPDATE = 1;        // Process metadata updated
        TERMINATION = 2;   // Process terminated
        ANOMALY = 3;       // Anomalous behavior detected
    }
}
```

### RPC Message Schema

```protobuf
// Collector lifecycle RPC messages
message StartCollectorRequest {
    string collector_type = 1;     // "process", "network", etc.
    string collector_id = 2;       // Unique collector instance ID
    CollectorConfig config = 3;    // Collector configuration
    map<string, string> environment = 4; // Environment variables
}

message StartCollectorResponse {
    bool success = 1;
    string collector_id = 2;
    string error_message = 3;
    CollectorStatus status = 4;
}

message StopCollectorRequest {
    string collector_id = 1;
    bool force = 2;                // Force immediate shutdown
    int32 timeout_seconds = 3;     // Graceful shutdown timeout
}

message StopCollectorResponse {
    bool success = 1;
    string error_message = 2;
    int32 shutdown_duration_ms = 3; // Actual shutdown time
}

// Health check RPC messages
message HealthCheckRequest {
    string collector_id = 1;
    bool include_metrics = 2;      // Include performance metrics
}

message HealthCheckResponse {
    HealthStatus status = 1;
    string message = 2;
    map<string, double> metrics = 3; // Performance metrics
    int64 uptime_seconds = 4;

    enum HealthStatus {
        HEALTHY = 0;
        DEGRADED = 1;
        UNHEALTHY = 2;
        UNKNOWN = 3;
    }
}

// Configuration management RPC messages
message UpdateConfigRequest {
    string collector_id = 1;
    CollectorConfig new_config = 2;
    bool validate_only = 3;        // Validate without applying
}

message UpdateConfigResponse {
    bool success = 1;
    string error_message = 2;
    repeated ConfigValidationError validation_errors = 3;
}
```

### Message Versioning Strategy

- **Protobuf Evolution**: Use protobuf field numbers for backward compatibility
- **Version Headers**: Include version information in message metadata
- **Graceful Degradation**: Handle unknown fields gracefully
- **Migration Support**: Support multiple message versions during transitions

## Deployment Architecture

### Embedded Broker (Default)

The embedded broker runs within the daemoneye-agent process:

```rust
// Embedded broker configuration
pub struct EmbeddedBrokerConfig {
    pub max_connections: usize,     // Default: 64
    pub message_buffer_size: usize, // Default: 10000
    pub transport: TransportConfig, // Unix sockets/named pipes
    pub security: SecurityConfig,   // Authentication settings
}

// Embedded broker lifecycle
impl DaemoneyeAgent {
    async fn start_embedded_broker(&mut self) -> Result<()> {
        let broker = BusrtBroker::embedded(self.config.broker.clone()).await?;
        self.broker = Some(broker);

        // Start collector management services
        self.start_lifecycle_service().await?;
        self.start_health_service().await?;
        self.start_config_service().await?;

        Ok(())
    }
}
```

**Advantages:**

- Simplified deployment (single process)
- Lower resource overhead
- Easier configuration management
- Suitable for standalone deployments

**Resource Characteristics:**

- Memory: ~10-20MB additional overhead
- CPU: \<1% additional usage
- File Descriptors: ~10-20 additional FDs

### Standalone Broker (Enterprise)

For enterprise deployments requiring high availability:

```rust
// Standalone broker configuration
pub struct StandaloneBrokerConfig {
    pub bind_address: SocketAddr,       // Network binding
    pub max_connections: usize,         // Default: 1000
    pub persistence: PersistenceConfig, // Message persistence
    pub clustering: ClusterConfig,      // Multi-broker clustering
    pub monitoring: MonitoringConfig,   // Metrics and observability
}

// Standalone broker deployment
pub struct StandaloneBroker {
    config: StandaloneBrokerConfig,
    runtime: BusrtRuntime,
    services: Vec<Box<dyn RpcService>>,
}
```

**Advantages:**

- High availability and clustering
- Better resource isolation
- Centralized message routing
- Advanced monitoring capabilities

**Resource Characteristics:**

- Memory: ~100-500MB depending on load
- CPU: 2-5% under normal load
- Network: Dedicated network ports

### Configuration Selection

```yaml
# daemoneye-agent configuration
broker:
  mode: embedded    # or "standalone"

  # Embedded broker settings
  embedded:
    max_connections: 64
    buffer_size: 10000
    transport:
      type: unix_socket    # or "named_pipe" on Windows
      path: /tmp/daemoneye-broker.sock

  # Standalone broker settings (Enterprise)
  standalone:
    address: 127.0.0.1:9090
    tls:
      enabled: true
      cert_file: /etc/daemoneye/broker.crt
      key_file: /etc/daemoneye/broker.key
```

## Migration Strategy

### Phase 1: Compatibility Layer

Create a compatibility layer that maintains existing crossbeam semantics:

```rust
// Compatibility wrapper for existing event bus
pub struct CrossbeamCompatibilityLayer {
    busrt_client: BusrtClient,
    topic_mapping: HashMap<EventType, String>,
}

impl CrossbeamCompatibilityLayer {
    // Map existing event types to busrt topics
    fn map_event_to_topic(&self, event: &CollectionEvent) -> String {
        match event {
            CollectionEvent::Process(_) => "events/process/enumeration".to_string(),
            CollectionEvent::Network(_) => "events/network/connections".to_string(),
            // ... other mappings
        }
    }

    // Maintain existing send/receive semantics
    pub async fn send(&self, event: CollectionEvent) -> Result<()> {
        let topic = self.map_event_to_topic(&event);
        let message = BusrtEvent::from(event);
        self.busrt_client.publish(&topic, message).await
    }
}
```

### Phase 2: Gradual Migration

1. **Replace Event Bus Infrastructure**

   - Swap crossbeam channels with busrt topics
   - Maintain identical API surface
   - Add busrt-specific configuration options

2. **Update Event Sources**

   - Modify EventSource implementations to use busrt
   - Add topic-based event publishing
   - Implement capability-based topic selection

3. **Enhance Agent Coordination**

   - Add RPC-based collector management
   - Implement health check protocols
   - Enable dynamic configuration updates

### Phase 3: Advanced Features

1. **Multi-Collector Coordination**

   - Enable cross-collector event correlation
   - Implement workflow orchestration
   - Add advanced routing capabilities

2. **Enterprise Features**

   - Standalone broker deployment
   - Message persistence and replay
   - Advanced monitoring and alerting

### Testing Strategy

#### Behavioral Equivalence Testing

```rust
#[cfg(test)]
mod migration_tests {
    use super::*;

    #[tokio::test]
    async fn test_crossbeam_busrt_equivalence() {
        // Test that busrt implementation produces identical behavior
        let crossbeam_events = run_crossbeam_scenario().await;
        let busrt_events = run_busrt_scenario().await;

        assert_eq!(crossbeam_events, busrt_events);
    }

    #[tokio::test]
    async fn test_event_ordering_preservation() {
        // Ensure event ordering is preserved during migration
        // Critical for process lifecycle tracking
    }

    #[tokio::test]
    async fn test_performance_regression() {
        // Verify busrt doesn't introduce performance regressions
        let crossbeam_perf = benchmark_crossbeam().await;
        let busrt_perf = benchmark_busrt().await;

        assert!(busrt_perf.throughput >= crossbeam_perf.throughput * 0.95);
    }
}
```

#### Integration Testing

- **Multi-Process Testing**: Validate collector coordination across processes
- **Failure Scenarios**: Test broker failures and recovery
- **Load Testing**: Verify performance under high message volumes
- **Security Testing**: Validate topic isolation and access controls

### Migration Checklist

- [ ] Implement busrt compatibility layer
- [ ] Create topic mapping configuration
- [ ] Update collector-core EventSource implementations
- [ ] Add RPC service implementations
- [ ] Implement health check protocols
- [ ] Create configuration migration tools
- [ ] Develop comprehensive test suite
- [ ] Document deployment procedures
- [ ] Create rollback procedures

## Performance Characteristics

### Throughput Targets

- **Event Publishing**: >10,000 events/second per collector
- **RPC Calls**: \<10ms latency for health checks
- **Message Routing**: \<1ms broker routing latency
- **Memory Usage**: \<50MB additional overhead for embedded broker

### Scalability Limits

- **Embedded Broker**: Up to 64 concurrent collectors
- **Standalone Broker**: Up to 1000 concurrent collectors (Enterprise)
- **Message Buffering**: 10,000 messages per topic (configurable)
- **Connection Limits**: Platform-dependent (typically 1024+ file descriptors)

### Monitoring and Observability

- **Broker Metrics**: Message throughput, connection count, memory usage
- **Topic Statistics**: Message rates per topic, subscriber counts
- **RPC Metrics**: Call latency, success rates, error rates
- **Health Indicators**: Collector status, broker health, system resources

This architecture provides a robust foundation for multi-collector coordination while maintaining backward compatibility and enabling future enterprise features.
