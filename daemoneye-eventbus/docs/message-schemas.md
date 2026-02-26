# DaemonEye EventBus Message Schemas

This document describes the message schemas used by the daemoneye-eventbus for pub/sub event distribution and RPC request/response patterns for collector management.

## Overview

The daemoneye-eventbus uses a comprehensive message schema system that supports:

- **Pub/Sub Event Distribution**: Collection events from multiple collector types
- **RPC Request/Response**: Collector lifecycle management and control operations
- **Event Correlation**: Multi-collector workflow coordination
- **Version Negotiation**: Backward compatibility and protocol evolution

## Message Schema Architecture

### Core Message Structure

All messages in the EventBus follow a common envelope structure defined in `eventbus.proto`:

```protobuf
message EventBusMessage {
  MessageMetadata metadata = 1;
  CorrelationMetadata correlation = 2;
  oneof payload {
    CollectionEventPayload collection_event = 3;
    RpcRequestPayload rpc_request = 4;
    RpcResponsePayload rpc_response = 5;
    ControlMessagePayload control_message = 6;
    HeartbeatPayload heartbeat = 7;
    AlertPayload alert = 8;
  }
}
```

### Message Types

The EventBus supports several message types for different use cases:

1. **COLLECTION_EVENT**: Events from monitoring collectors (process, network, filesystem, performance)
2. **RPC_REQUEST/RPC_RESPONSE**: Request/response patterns for collector management
3. **CONTROL_MESSAGE**: System coordination and configuration updates
4. **HEARTBEAT**: Health monitoring and keepalive messages
5. **ALERT**: Notification delivery for security events

## Event Correlation Metadata

Multi-collector workflows require sophisticated correlation tracking:

```protobuf
message CorrelationMetadata {
  string correlation_id = 1;                    // Primary correlation ID
  optional string parent_correlation_id = 2;    // Hierarchical relationships
  string root_correlation_id = 3;               // Workflow root
  optional string trace_id = 4;                 // Distributed tracing
  optional string span_id = 5;                  // Tracing span
  uint64 sequence_number = 6;                   // Event ordering
  optional string workflow_stage = 7;           // Pipeline stage
  map<string, string> correlation_tags = 8;     // Custom grouping
}
```

### Correlation Patterns

1. **Linear Workflow**: Events with same `root_correlation_id` and incrementing `sequence_number`
2. **Hierarchical Analysis**: Parent-child relationships using `parent_correlation_id`
3. **Cross-Collector Correlation**: Events from different collectors sharing `correlation_tags`
4. **Distributed Tracing**: Integration with OpenTelemetry via `trace_id` and `span_id`

## Collection Event Schemas

### Process Events

Process monitoring events from procmond:

```protobuf
message ProcessEventData {
  ProcessRecord process = 1;                    // Core process information
  ProcessEventSubtype subtype = 2;             // Event type (created, updated, terminated)
  optional ProcessTreeContext tree_context = 3; // Parent/child relationships
  optional SecurityContext security_context = 4; // User, group, privileges
  optional ProcessPerformanceMetrics performance = 5; // CPU, memory, I/O
}
```

### Network Events (Future: netmond)

Network monitoring events with connection tracking:

```protobuf
message NetworkEventData {
  NetworkRecord network = 1;                   // Connection information
  NetworkEventSubtype subtype = 2;            // Connection state changes
  optional NetworkConnectionMetadata connection_metadata = 3; // Duration, flags
  optional TrafficAnalysisResults traffic_analysis = 4; // Threat detection
}
```

### Filesystem Events (Future: fsmond)

Filesystem monitoring with access pattern analysis:

```protobuf
message FilesystemEventData {
  FilesystemRecord filesystem = 1;             // File operation details
  FilesystemEventSubtype subtype = 2;         // Operation type
  optional FileMetadata file_metadata = 3;    // File type, attributes
  optional AccessPatternAnalysis access_analysis = 4; // Anomaly detection
}
```

### Performance Events (Future: perfmond)

System performance monitoring with trend analysis:

```protobuf
message PerformanceEventData {
  PerformanceRecord performance = 1;          // Metric information
  PerformanceEventSubtype subtype = 2;       // Threshold, anomaly events
  optional ThresholdInfo threshold_info = 3;  // Threshold violations
  optional TrendAnalysis trend_analysis = 4;  // Trend detection
}
```

## RPC Request/Response Schemas

### Collector Lifecycle Management

RPC patterns for managing collector processes:

```protobuf
message RpcRequestPayload {
  RpcRequest request = 1;                     // Core request information
  RpcRoutingInfo routing = 2;                 // Load balancing, circuit breaker
  optional RpcAuthInfo auth_info = 3;         // Authentication/authorization
}

message RpcResponsePayload {
  RpcResponse response = 1;                   // Core response information
  RpcRoutingInfo routing = 2;                 // Response routing
  optional RpcPerformanceMetrics performance_metrics = 3; // Timing metrics
}
```

### Supported Operations

- **START/STOP/RESTART**: Collector lifecycle management
- **HEALTH_CHECK**: Status monitoring and diagnostics
- **UPDATE_CONFIG**: Dynamic configuration updates
- **GET_CAPABILITIES**: Feature discovery and negotiation
- **GRACEFUL_SHUTDOWN**: Coordinated shutdown sequences

## Topic Hierarchy Design

The EventBus uses a hierarchical topic structure for efficient routing:

### Topic Patterns

- **events.process.**\*: Process monitoring events from procmond
- **events.network.**\*: Network monitoring events from netmond (future)
- **events.filesystem.**\*: Filesystem monitoring events from fsmond (future)
- **events.performance.**\*: Performance monitoring events from perfmond (future)
- **control.collector.**\*: Collector lifecycle management
- **control.agent.**\*: Agent coordination and configuration
- **control.health.**\*: Health monitoring and heartbeat messages

### Wildcard Matching

- **Single-level wildcard** (`+`): Matches one topic level
- **Multi-level wildcard** (`#`): Matches multiple topic levels
- **Pattern priority**: More specific patterns take precedence

## Message Versioning and Backward Compatibility

### Versioning Strategy

The EventBus uses semantic versioning for protocol evolution:

```protobuf
message MessageVersion {
  uint32 major = 1;                          // Breaking changes
  uint32 minor = 2;                          // Backward compatible additions
  uint32 patch = 3;                          // Bug fixes
  string protocol_version = 4;               // Human-readable version
}
```

### Compatibility Rules

1. **Major Version**: Breaking changes, no backward compatibility
2. **Minor Version**: Backward compatible additions, new optional features
3. **Patch Version**: Bug fixes, no protocol changes

### Version Negotiation

Clients and servers negotiate compatible versions during connection:

```protobuf
message VersionNegotiationRequest {
  repeated MessageVersion supported_versions = 1; // Client capabilities
  MessageVersion preferred_version = 2;           // Client preference
  repeated string client_capabilities = 3;        // Feature requirements
}

message VersionNegotiationResponse {
  MessageVersion negotiated_version = 1;          // Agreed version
  repeated string server_capabilities = 2;        // Available features
  repeated string compatibility_warnings = 3;     // Deprecation notices
  bool negotiation_success = 4;                   // Success status
}
```

### Migration Strategy

1. **Graceful Degradation**: Older clients work with newer servers
2. **Feature Detection**: Capability-based feature availability
3. **Deprecation Warnings**: Clear migration guidance
4. **Compatibility Matrix**: Version support documentation

## Quality of Service (QoS)

### Delivery Guarantees

```protobuf
enum DeliveryGuarantee {
  AT_MOST_ONCE = 0;    // Fire and forget
  AT_LEAST_ONCE = 1;   // With retries
  EXACTLY_ONCE = 2;    // With deduplication
}
```

### Message Ordering

- **Ordered Delivery**: Maintains message sequence within correlation groups
- **Priority Handling**: High-priority messages processed first
- **Queue Management**: Bounded queues with backpressure handling

### Retention Policies

```protobuf
message RetentionPolicy {
  uint64 retention_duration_ms = 1;           // Time-based retention
  optional uint32 max_retained_messages = 2;  // Count-based retention
  optional uint64 max_retained_size_bytes = 3; // Size-based retention
}
```

## Security Considerations

### Authentication and Authorization

```protobuf
message RpcAuthInfo {
  optional string client_cert_fingerprint = 1; // mTLS authentication
  optional string auth_token = 2;              // Bearer token
  repeated string permissions = 3;             // Access control
  string auth_method = 4;                      // Authentication method
}
```

### Message Integrity

- **CRC32 Checksums**: Message corruption detection
- **Sequence Numbers**: Message ordering and duplicate detection
- **Correlation Validation**: Cross-reference correlation metadata

## Performance Characteristics

### Throughput Targets

- **Message Processing**: >10,000 messages/second
- **Latency**: \<1ms median message delivery
- **Memory Usage**: \<100MB for 10,000 active subscriptions
- **CPU Overhead**: \<5% sustained usage

### Optimization Strategies

1. **Zero-Copy Serialization**: Efficient protobuf handling
2. **Batch Processing**: Group message operations
3. **Connection Pooling**: Reuse transport connections
4. **Circuit Breakers**: Prevent cascade failures

## Integration Examples

### Basic Pub/Sub

```rust,ignore
use daemoneye_eventbus::{EventBus, CollectionEvent, CorrelationMetadata};

// Create correlation metadata
let correlation = CorrelationMetadata::new()
    .with_stage("process_analysis".to_string())
    .with_tag("collector".to_string(), "procmond".to_string());

// Publish event with correlation
let event = CollectionEvent::Process(process_event);
event_bus.publish_with_correlation(event, correlation).await?;
```

### RPC Call Pattern

```rust,ignore
use daemoneye_eventbus::{CollectorRpcClient, CollectorOperation, RpcRequest};

// Create RPC client
let mut rpc_client = CollectorRpcClient::new("control.collector.procmond").await?;

// Make lifecycle request
let request = RpcRequest::lifecycle(
    "agent-id".to_string(),
    "control.collector.procmond".to_string(),
    CollectorOperation::Start,
    lifecycle_request,
    Duration::from_secs(30)
);

let response = rpc_client.call(request, Duration::from_secs(30)).await?;
```

### Version Negotiation

```rust,ignore
use daemoneye_eventbus::{VersionNegotiator, VersionNegotiationRequest};

// Negotiate version with server
let negotiator = VersionNegotiator::new();
let request = VersionNegotiationRequest::for_current_version("client-id".to_string());
let response = negotiator.negotiate_version(&request)?;

// Check negotiated capabilities
if response.server_capabilities.contains(&"advanced_routing".to_string()) {
    // Use advanced routing features
}
```

## Future Extensions

### Planned Enhancements

1. **Stream Processing**: Real-time event stream analytics
2. **Message Compression**: Reduce bandwidth usage
3. **Encryption**: End-to-end message encryption
4. **Federation**: Multi-broker message routing
5. **Metrics Integration**: Prometheus metrics export

### Extensibility Points

- **Custom Event Types**: Domain-specific event schemas
- **Plugin Architecture**: Custom message processors
- **Transport Layers**: Additional transport protocols
- **Serialization Formats**: Alternative to protobuf
