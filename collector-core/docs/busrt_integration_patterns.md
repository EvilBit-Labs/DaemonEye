# Busrt Integration Patterns for DaemonEye

This document outlines the key API patterns and configuration options for integrating busrt message broker into the DaemonEye architecture.

## Overview

Busrt provides industrial-grade IPC capabilities that will replace the current crossbeam-based event bus in DaemonEye. The integration supports:

- **Embedded Broker Mode**: Broker runs within daemoneye-agent process
- **Standalone Server Mode**: Separate broker process for advanced deployments
- **Multi-Transport Support**: In-process channels, Unix sockets, TCP sockets
- **Pub/Sub Patterns**: Event distribution between components
- **RPC Patterns**: Request/response communication for control operations

## Core API Patterns

### 1. Embedded Broker Configuration

```rust
use busrt::broker::Broker;
use std::time::Duration;

// Create and configure embedded broker
let mut broker = Broker::new();

// Configure IPC server for local communication
broker.spawn_ipc_server("daemoneye.sock", 100)?;

// Set appropriate timeouts for security monitoring
broker.set_timeout(Duration::from_secs(30));

// Start the broker
broker.spawn().await?;
```

**Key Configuration Options:**

- **Socket Path**: Use descriptive names like "daemoneye.sock"
- **Connection Limit**: 100 concurrent connections (suitable for multi-collector setup)
- **Timeout**: 30 seconds (balance between responsiveness and reliability)
- **Transport**: IPC sockets for local communication, TCP for network (Enterprise)

### 2. Client Connection Patterns

```rust
use busrt::client::AsyncClient;

// Connect to embedded broker
let client = AsyncClient::connect("ipc://daemoneye.sock").await?;

// Graceful disconnect with timeout
let disconnect_result = timeout(
    Duration::from_secs(5),
    client.disconnect()
).await;
```

**Connection Best Practices:**

- **Connection String Format**: `ipc://socket_name` for local IPC
- **Timeout Handling**: Always wrap operations in timeouts
- **Graceful Shutdown**: Implement proper disconnect sequences
- **Reconnection Logic**: Handle connection failures with exponential backoff

### 3. Pub/Sub Event Distribution

```rust
use busrt::QoS;

// Subscribe to event topics with appropriate QoS
client.subscribe("events.process.*", QoS::Processed).await?;
client.subscribe("events.network.*", QoS::Processed).await?;
client.subscribe("control.*", QoS::Processed).await?;

// Publish events to domain-specific topics
let event_data = serde_json::to_vec(&process_event)?;
client.publish("events.process.new", event_data, QoS::Processed).await?;
```

**Topic Hierarchy for DaemonEye:**

- `events.process.*` - Process monitoring events from procmond
- `events.network.*` - Network events from netmond (future)
- `events.filesystem.*` - Filesystem events from fsmond (future)
- `events.performance.*` - Performance events from perfmond (future)
- `control.*` - Control messages for collector lifecycle management
- `tasks.*` - Task distribution to collectors
- `results.*` - Task results from collectors
- `health.*` - Health check and status messages

### 4. RPC Patterns for Control Operations

```rust
// Register RPC handler (collector side)
client.rpc_register("health_check", |request: HealthCheckRequest| async move {
    let response = HealthCheckResponse {
        status: "healthy".to_string(),
        uptime_seconds: get_uptime(),
    };
    Ok(serde_json::to_vec(&response)?)
}).await?;

// Make RPC call (agent side)
let request_data = serde_json::to_vec(&health_request)?;
let response_data = client.rpc_call("health_check", request_data).await?;
```

**RPC Operations for DaemonEye:**

- `health_check` - Collector health monitoring
- `start_collection` - Start/resume data collection
- `stop_collection` - Stop/pause data collection
- `update_config` - Dynamic configuration updates
- `get_capabilities` - Capability negotiation
- `shutdown` - Graceful collector shutdown

## Transport Layer Options

### 1. In-Process Channels (Default)

- **Use Case**: Internal daemoneye-agent communication
- **Performance**: Highest throughput, lowest latency
- **Configuration**: Automatic, no external setup required

### 2. Unix Domain Sockets (Linux/macOS)

- **Use Case**: Inter-process collector communication
- **Security**: File system permissions, owner-only access
- **Configuration**: `ipc://socket_path`

### 3. Named Pipes (Windows)

- **Use Case**: Inter-process collector communication on Windows
- **Security**: Windows security descriptors
- **Configuration**: `ipc://pipe_name`

### 4. TCP Sockets (Enterprise)

- **Use Case**: Network-distributed collectors
- **Security**: mTLS authentication required
- **Configuration**: `tcp://host:port`

## Quality of Service (QoS) Levels

### QoS::Processed

- **Use Case**: Critical events and control messages
- **Guarantee**: At-least-once delivery with acknowledgment
- **Performance**: Higher overhead, stronger reliability

### QoS::Realtime

- **Use Case**: High-frequency monitoring data
- **Guarantee**: Best-effort delivery, no acknowledgment
- **Performance**: Lower overhead, higher throughput

## Error Handling Patterns

### Connection Failures

```rust
use anyhow::{Context, Result};

async fn connect_with_retry(address: &str, max_retries: u32) -> Result<AsyncClient> {
    let mut retry_count = 0;
    let mut delay = Duration::from_millis(100);

    loop {
        match AsyncClient::connect(address).await {
            Ok(client) => return Ok(client),
            Err(e) if retry_count < max_retries => {
                warn!("Connection failed (attempt {}): {}", retry_count + 1, e);
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay * 2, Duration::from_secs(30));
                retry_count += 1;
            }
            Err(e) => return Err(e).context("Failed to connect after retries"),
        }
    }
}
```

### Message Handling Failures

```rust
async fn handle_message_with_recovery(client: &mut AsyncClient) -> Result<()> {
    loop {
        match timeout(Duration::from_secs(30), client.recv()).await {
            Ok(Ok(message)) => {
                if let Err(e) = process_message(message).await {
                    error!("Failed to process message: {}", e);
                    // Continue processing other messages
                }
            }
            Ok(Err(e)) => {
                error!("Message receive error: {}", e);
                // Attempt reconnection
                break;
            }
            Err(_) => {
                debug!("Message receive timeout (normal for low-traffic periods)");
            }
        }
    }
    Ok(())
}
```

## Performance Considerations

### Message Batching

- **Batch Size**: 100-1000 messages per batch for optimal throughput
- **Flush Interval**: 100ms maximum for real-time requirements
- **Memory Usage**: Monitor queue depths to prevent memory exhaustion

### Connection Pooling

- **Pool Size**: 1 connection per collector type (process, network, filesystem)
- **Lifecycle**: Long-lived connections with automatic reconnection
- **Resource Limits**: Configure max connections per broker instance

### Topic Design

- **Granularity**: Balance between specificity and subscription overhead
- **Wildcards**: Use sparingly to avoid performance impact
- **Retention**: Configure appropriate message retention policies

## Security Considerations

### Local IPC Security

- **File Permissions**: 0600 for socket files, 0700 for directories
- **Process Isolation**: Each collector runs in separate process
- **Privilege Separation**: Broker runs with minimal privileges

### Network Security (Enterprise)

- **mTLS**: Mandatory for TCP transport
- **Certificate Validation**: Full chain validation required
- **Cipher Suites**: TLS 1.3 preferred, TLS 1.2 minimum

### Message Security

- **Serialization**: Use serde_json for structured data
- **Validation**: Validate all incoming messages
- **Size Limits**: Enforce maximum message sizes

## Integration with Existing DaemonEye Components

### Replacing Crossbeam Event Bus

1. **Phase 1**: Implement BusrtEventBus struct with EventBus trait compatibility
2. **Phase 2**: Migrate event routing from channels to pub/sub topics
3. **Phase 3**: Add RPC patterns for collector lifecycle management
4. **Phase 4**: Remove crossbeam dependencies

### IPC Protocol Migration

1. **Maintain Compatibility**: Keep existing protobuf message formats
2. **Add Capabilities**: Extend with busrt-specific features
3. **Gradual Migration**: Support both protocols during transition
4. **Testing**: Comprehensive integration tests for both protocols

### Configuration Integration

```rust
#[derive(serde::Deserialize)]
pub struct BusrtConfig {
    pub transport: TransportConfig,
    pub broker: BrokerConfig,
    pub client: ClientConfig,
}

#[derive(serde::Deserialize)]
pub struct TransportConfig {
    pub socket_path: String,
    pub connection_timeout: Duration,
    pub max_connections: u32,
}

#[derive(serde::Deserialize)]
pub struct BrokerConfig {
    pub embedded: bool,
    pub timeout: Duration,
    pub max_message_size: usize,
}
```

## Testing Strategies

### Unit Tests

- **Broker Lifecycle**: Start/stop operations
- **Client Lifecycle**: Connect/disconnect patterns
- **Message Serialization**: Serde compatibility
- **Error Handling**: Failure scenarios

### Integration Tests

- **Multi-Client**: Multiple collectors connecting simultaneously
- **Message Flow**: End-to-end pub/sub and RPC patterns
- **Performance**: Throughput and latency benchmarks
- **Reliability**: Network partition and recovery scenarios

### Load Testing

- **High Frequency**: 10,000+ messages per second
- **Many Clients**: 100+ concurrent connections
- **Memory Usage**: Long-running stability tests
- **Resource Limits**: Behavior under resource constraints

## Migration Timeline

### Phase 1: Foundation (Current Task)

- ✅ Add busrt dependency
- ✅ Create basic examples and documentation
- ✅ Implement proof-of-concept patterns

### Phase 2: Core Integration

- Implement BusrtEventBus with EventBus trait compatibility
- Create embedded broker startup in collector-core
- Add client connection management

### Phase 3: Event Migration

- Replace crossbeam channels with busrt pub/sub
- Migrate event routing and filtering
- Update event statistics and monitoring

### Phase 4: RPC Implementation

- Add RPC patterns for collector lifecycle
- Implement health checks and configuration updates
- Create graceful shutdown coordination

### Phase 5: Multi-Process Support

- Enable task distribution across collector types
- Add capability-based routing
- Implement result aggregation and correlation

This integration will provide DaemonEye with industrial-grade IPC capabilities while maintaining the existing security and performance characteristics.
