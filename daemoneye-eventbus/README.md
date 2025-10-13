# DaemonEye EventBus

A cross-platform IPC event bus designed specifically for the DaemonEye monitoring system. Provides pub/sub messaging with wildcard topic matching and embedded broker functionality.

## Features

- **Cross-platform**: Windows (named pipes) and Unix (domain sockets)
- **Embedded broker**: Runs within daemoneye-agent orchestrator
- **Topic routing**: Wildcard matching for flexible event distribution
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

## Topic Naming Conventions

The event bus uses a hierarchical topic structure:

- `events.process.*` - Process monitoring events
- `events.network.*` - Network events (future)
- `events.filesystem.*` - Filesystem events (future)
- `events.performance.*` - Performance events (future)
- `control.collector.*` - Collector lifecycle control
- `health.*` - Health check messages

### Wildcard Matching

- `events.process.*` matches `events.process.new`, `events.process.old`, etc.
- `events.*` matches all event topics
- Exact matches take precedence over wildcards

## Performance Characteristics

- **Message Throughput**: 10,000+ messages per second
- **Connection Overhead**: Minimal with connection pooling
- **Memory Usage**: Bounded with configurable limits
- **Latency**: Sub-millisecond for local IPC

## Cross-Platform Support

### Unix Systems (Linux, macOS)

- Uses Unix domain sockets
- Socket files created in `/tmp/daemoneye-{instance}.sock`
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

## Integration with DaemonEye

This crate is designed to replace the busrt dependency in the DaemonEye monitoring system. It provides the same EventBus trait interface while ensuring cross-platform compatibility.

### Migration from busrt

1. Replace `busrt` dependency with `daemoneye-eventbus`
2. Update imports to use `daemoneye_eventbus::*`
3. The EventBus trait API remains the same
4. Topic patterns and subscription logic work identically

## License

MIT License - see LICENSE file for details.
