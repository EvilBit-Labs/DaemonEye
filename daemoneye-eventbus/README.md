# DaemonEye EventBus

A cross-platform local IPC event bus designed specifically for the DaemonEye monitoring system. Provides pub/sub messaging between collectors and the agent on the same system with wildcard topic matching and embedded broker functionality.

## Features

- **Cross-platform**: Windows (named pipes) and Unix (domain sockets)
- **Embedded broker**: Runs within daemoneye-agent orchestrator
- **Topic routing**: Hierarchical topic structure with wildcard matching (+ and #)
- **Correlation metadata**: Track events across multi-collector workflows with hierarchical correlation IDs
- **Workflow tracking**: Support for multi-stage analysis pipelines with stage tracking and tagging
- **At-most-once delivery**: Simple, reliable messaging semantics
- **Async/await**: Built on tokio for high-performance concurrent operations

## Basic Usage

> [!NOTE]
> This example requires adding `tokio` to your `Cargo.toml`:
>
> ```toml
> tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
> ```

```rust
use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeBroker, DaemoneyeEventBus, EventBus, ProcessEvent,
};
use std::collections::HashMap;
use std::time::SystemTime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Start embedded broker
    let broker = DaemoneyeBroker::new("/tmp/daemoneye.sock").await?;
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await?;

    // Create a process event
    let process_event = ProcessEvent {
        pid: 1234,
        name: "example_process".to_string(),
        command_line: Some("example_process --arg".to_string()),
        executable_path: Some("/usr/bin/example_process".to_string()),
        ppid: Some(1000),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    };

    // Publish the event - returns Result and uses ? operator for error propagation
    event_bus
        .publish(
            CollectionEvent::Process(process_event),
            "correlation-123".to_string(),
        )
        .await?;

    // Alternative: Explicit error handling
    // match event_bus.publish(CollectionEvent::Process(process_event), "correlation-123".to_string()).await {
    //     Ok(_) => println!("Event published successfully"),
    //     Err(e) => eprintln!("Failed to publish event: {}", e),
    // }

    Ok(())
}
```

## Topic Hierarchy

The event bus uses a hierarchical topic structure with up to 4 levels:

### Event Topics (Data Flow)

- `events.process.lifecycle` - Process start, stop, exit events
- `events.process.metadata` - Process metadata updates (CPU, memory)
- `events.process.tree` - Parent-child relationship changes
- `events.process.integrity` - Hash verification and integrity checks
- `events.process.anomaly` - Behavioral anomalies and suspicious patterns
- `events.process.batch` - Bulk process enumeration results
- `events.network.*` - Network events (future extension)
- `events.filesystem.*` - Filesystem events (future extension)
- `events.performance.*` - Performance events (future extension)

### Control Topics (Management Flow)

- `control.collector.lifecycle` - Start, stop, restart operations
- `control.collector.config` - Configuration updates and reloads
- `control.collector.task` - Task assignment and distribution
- `control.collector.registration` - Collector registration and capabilities
- `control.agent.orchestration` - Agent coordination messages
- `control.agent.policy` - Policy updates and enforcement
- `control.health.heartbeat` - Liveness check heartbeats
- `control.health.status` - Component status updates
- `control.health.diagnostics` - Diagnostic information exchange

### Wildcard Matching

The event bus supports two types of wildcards:

- **Single-level wildcard (`+`)**: Matches exactly one segment
  - Example: `events.+.lifecycle` matches `events.process.lifecycle` and `events.network.lifecycle`
- **Multi-level wildcard (`#`)**: Matches zero or more segments (must be at the end)
  - Example: `events.process.#` matches all process events
  - Example: `control.#` matches all control messages

### Access Control

Topics have three access levels:

- **Public**: Accessible to all components (e.g., `control.health.*`, `events.*.anomaly`)
- **Restricted**: Component-specific access (e.g., `events.process.*` for procmond)
- **Privileged**: Requires authentication (e.g., `control.collector.lifecycle`, `control.agent.policy`)

For complete topic hierarchy documentation, see [docs/topic-hierarchy.md](docs/topic-hierarchy.md).

## Performance Characteristics

- **Message Throughput**: 10,000+ messages per second
- **Connection Overhead**: Minimal with connection pooling
- **Memory Usage**: Bounded with configurable limits
- **Latency**: Sub-millisecond for local IPC

## Cross-Platform Support

### Unix Systems (Linux, macOS)

- Uses Unix domain sockets
- Socket files created in `/tmp/daemoneye.sock` (default) or `/tmp/daemoneye-{instance}.sock` for multiple instances
- File permissions: 0600 (owner read/write only)

### Windows

- Uses named pipes
- Pipe names: `\\.\pipe\DaemonEye-{instance}`
- Appropriate Windows security descriptors

## Security Considerations

### Local IPC Security

- File permissions: 0600 for socket files, 0700 for directories
- Process isolation: Each collector runs in separate process
- Privilege separation: Broker runs with minimal privileges

### Message Security

- Serialization: Uses bincode for efficient binary protocol
- Validation: All incoming messages validated
- Size limits: Enforce maximum message sizes

## Development

### Running Tests

```bash
cargo test
```

### Running Benchmarks

```bash
cargo bench
```

### Cross-Platform Testing

The event bus is tested on:

- Linux (Ubuntu 20.04+)
- macOS (14.0+)
- Windows (10+, 11, Server 2019+, Server 2022)

## Correlation Metadata

The event bus supports comprehensive correlation tracking for multi-collector workflows:

### Basic Correlation

```rust
use daemoneye_eventbus::CorrelationMetadata;

// Create correlation metadata
let metadata = CorrelationMetadata::new("workflow-123".to_string())
    .with_stage("collection".to_string())
    .with_tag("source".to_string(), "procmond".to_string());
```

### Hierarchical Correlation

```rust
// Create parent correlation
let parent_metadata = CorrelationMetadata::new("parent-id".to_string())
    .with_stage("detection".to_string())
    .with_tag("workflow".to_string(), "threat_analysis".to_string());

// Create child correlation that inherits properties
let child_metadata = parent_metadata.create_child("child-id".to_string());
// Child automatically inherits workflow stage and tags
```

### Correlation Filtering

```rust
use daemoneye_eventbus::{CorrelationFilter, EventSubscription};

// Filter by root correlation ID (entire workflow)
let filter = CorrelationFilter::new()
    .with_root_id("root-workflow-id".to_string())
    .with_stage("analysis".to_string())
    .with_required_tag("severity".to_string(), "critical".to_string());

// Use in subscription
let subscription = EventSubscription {
    subscriber_id: "forensic-analyzer".to_string(),
    correlation_filter: Some(filter),
    // ... other fields
};
```

### Use Cases

- **Multi-Collector Workflows**: Track events across process, network, and filesystem collectors on the same system
- **Forensic Investigation**: Group all events related to a security investigation on a single host
- **Local Workflow Tracing**: Track events across local collector components with correlation IDs
- **Performance Analysis**: Track metrics across workflow stages within a single system

For complete correlation metadata documentation, see [docs/correlation-metadata.md](docs/correlation-metadata.md).

## Integration with DaemonEye

This crate is designed to replace the busrt dependency in the DaemonEye monitoring system. It provides local IPC pub/sub messaging between collectors and the daemoneye-agent on the same system, ensuring cross-platform compatibility.

### Migration from busrt

1. Replace `busrt` dependency with `daemoneye-eventbus`
2. Update imports to use `daemoneye_eventbus::*`
3. The EventBus trait API remains the same
4. Topic patterns and subscription logic work identically
5. New: Correlation metadata support for workflow tracking

## License

Apache 2.0 License - see LICENSE file for details.
