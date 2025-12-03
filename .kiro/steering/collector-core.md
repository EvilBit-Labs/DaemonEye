---
inclusion: fileMatch
fileMatchPattern:
  - collector-core/**/*
  - '**/collector*'
---

# Collector-Core Framework Standards

## Core Architecture Patterns

The `collector-core` framework provides unified collection infrastructure with these mandatory patterns:

- **EventSource Trait**: All collection components MUST implement `EventSource` with `async_trait`
- **Capability-Based Design**: Use `SourceCaps` bitflags for feature discovery and task routing
- **Event-Driven Architecture**: All data flows through `CollectionEvent` enum variants
- **Graceful Shutdown**: Implement cooperative cancellation with timeout enforcement

## EventSource Implementation Requirements

When implementing EventSource, follow this exact pattern:

```rust,ignore
use async_trait::async_trait;
use collector_core::{CollectionEvent, EventSource, SourceCaps};
use tokio::sync::mpsc;

#[async_trait]
impl EventSource for MySource {
    fn name(&self) -> &'static str {
        // Use kebab-case naming convention
        "my-event-source"
    }

    fn capabilities(&self) -> SourceCaps {
        // Declare capabilities using bitflags OR operations
        SourceCaps::PROCESS | SourceCaps::REALTIME
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        // MUST handle tx.send() failures gracefully
        // MUST respect cancellation tokens
        // MUST use structured logging with tracing
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // MUST complete within graceful shutdown timeout (default 60s) for resource cleanup
        // MUST clean up resources deterministically
        Ok(())
    }
}
```

## Event Type Conventions

Use strongly-typed events with consistent field naming:

```rust,ignore
use collector_core::{CollectionEvent, ProcessEvent};
use std::time::SystemTime;

// Always include timestamp and use SystemTime
let event = CollectionEvent::Process(ProcessEvent {
    pid: 1234,
    name: "process_name".to_string(),
    timestamp: SystemTime::now(),
    // Use Option<T> for nullable fields
    executable_path: Some("/usr/bin/example".to_string()),
});
```

## Capability Management Rules

- Use bitflags for all capability declarations
- Combine capabilities with `|` operator
- Check capabilities with `.contains()` method
- Never assume capabilities without verification

```rust,ignore
use collector_core::SourceCaps;

// Correct capability combination
let caps = SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE;

// Always verify before using features
if caps.contains(SourceCaps::KERNEL_LEVEL) {
    // Use kernel-level features
}
```

## Configuration Builder Pattern

MUST use builder pattern for all configuration:

```rust,ignore
use collector_core::CollectorConfig;
use std::time::Duration;

let config = CollectorConfig::new()
    .with_max_event_sources(32)           // Default: 16
    .with_event_buffer_size(2000)         // Default: 1000
    .with_shutdown_timeout(Duration::from_secs(60))  // Required
    .with_health_check_interval(Duration::from_secs(120));
```

## Error Handling Requirements

- Use `anyhow::Result<T>` for all fallible operations
- Provide context with `.with_context()` for debugging
- Log errors at appropriate levels (error, warn, debug)
- Never panic in production code paths

## Testing Mandates

All collector-core components MUST include:

- **Unit Tests**: EventSource trait implementation, capability validation
- **Integration Tests**: Multi-source coordination, event flow validation
- **Performance Tests**: Throughput benchmarks, memory usage validation
- **Property Tests**: Event serialization, configuration validation

## Performance Requirements

- **Event Throughput**: Minimum 1000 events/second
- **CPU Overhead**: Maximum 5% sustained usage
- **Memory**: Bounded growth under continuous load
- **Shutdown**: Complete within forced/emergency shutdown timeout (500ms) for termination

## Code Style Enforcement

- Use `#[async_trait]` for all async traits
- Prefer `tokio::sync::mpsc` for event channels
- Use `tracing` for structured logging, never `println!`
- Follow Rust naming conventions (snake_case, kebab-case for names)
