# Collector-Core Framework

The collector-core framework provides a unified collection infrastructure that enables multiple monitoring components while maintaining shared operational foundation.

---

## Table of Contents

[TOC]

---

## Overview

The collector-core framework is the foundation of DaemonEye's extensible monitoring architecture. It provides:

- Universal `EventSource` trait for pluggable collection implementations
- `Collector` runtime for event source management and aggregation
- Extensible `CollectionEvent` enum for unified event handling
- Capability negotiation through `SourceCaps` bitflags
- Shared infrastructure for configuration, logging, and health checks

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                    Collector Runtime                        │
├─────────────────────────────────────────────────────────────┤
│  EventSource    EventSource    EventSource    EventSource   │
│  (Process)      (Network)      (Filesystem)   (Performance) │
└─────────────────────────────────────────────────────────────┘
```

The framework separates collection methodology from operational infrastructure, allowing different collection strategies to share the same runtime foundation.

## Core Components

### EventSource Trait

The `EventSource` trait abstracts collection methodology from operational infrastructure:

```rust
#[async_trait]
pub trait EventSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> SourceCaps;

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()>;

    async fn stop(&self) -> anyhow::Result<()>;
    async fn health_check(&self) -> anyhow::Result<()>;
}
```

### Capability System

The `SourceCaps` bitflags enable capability negotiation between components:

```rust
bitflags! {
    pub struct SourceCaps: u32 {
        const PROCESS = 1 << 0;        // Process monitoring
        const NETWORK = 1 << 1;        // Network monitoring
        const FILESYSTEM = 1 << 2;     // Filesystem monitoring
        const PERFORMANCE = 1 << 3;    // Performance monitoring
        const REALTIME = 1 << 4;       // Real-time event streaming
        const KERNEL_LEVEL = 1 << 5;   // Kernel-level access
        const SYSTEM_WIDE = 1 << 6;    // System-wide monitoring
    }
}
```

### Collection Events

The `CollectionEvent` enum provides unified event handling:

```rust
pub enum CollectionEvent {
    Process(ProcessEvent),
    Network(NetworkEvent),
    Filesystem(FilesystemEvent),
    Performance(PerformanceEvent),
    TriggerRequest(TriggerRequest),
}
```

### Collector Runtime

The `Collector` provides unified runtime for multiple event sources:

```rust
pub struct Collector {
    config: CollectorConfig,
    sources: Vec<Box<dyn EventSource>>,
}

impl Collector {
    pub fn new(config: CollectorConfig) -> Self;
    pub fn register(&mut self, source: Box<dyn EventSource>) -> anyhow::Result<()>;
    pub fn capabilities(&self) -> SourceCaps;
    pub async fn run(self) -> Result<()>;
}
```

## Event Processing Pipeline

### Event Flow

1. **Event Sources** generate events based on their collection methodology
2. **Event Channel** receives events via `mpsc::Sender<CollectionEvent>`
3. **Event Processor** handles batching, backpressure, and processing
4. **Event Bus** (optional) provides pub/sub event distribution
5. **Storage/Forwarding** processes events according to configuration

### Batching and Backpressure

The framework includes sophisticated event processing with:

- **Configurable Batching**: Events are batched for efficient processing
- **Backpressure Handling**: Semaphore-based flow control prevents memory exhaustion
- **Timeout Management**: Batch timeouts ensure timely processing
- **Graceful Degradation**: System continues operation under load

```rust
// Batch configuration
fn create_batch_config() -> CollectorConfig {
    CollectorConfig::new()
        .with_max_batch_size(1000)
        .with_batch_timeout(Duration::from_secs(5))
        .with_backpressure_threshold(800)
}
```

## Configuration System

### Hierarchical Configuration

The framework supports hierarchical configuration loading:

1. Command-line flags (highest precedence)
2. Environment variables
3. User configuration files
4. System configuration files
5. Embedded defaults (lowest precedence)

### Configuration Structure

```rust
pub struct CollectorConfig {
    pub component_name: String,
    pub max_event_sources: usize,
    pub event_buffer_size: usize,
    pub max_batch_size: usize,
    pub batch_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub health_check_interval: Duration,
    pub enable_telemetry: bool,
    pub enable_debug_logging: bool,
    // ... additional configuration options
}
```

## Health Monitoring

### Health Check System

The framework provides comprehensive health monitoring:

- **Source Health Checks**: Individual event source health monitoring
- **System Resource Monitoring**: CPU, memory, and performance tracking
- **Error Rate Monitoring**: Automatic error rate calculation and alerting
- **Telemetry Collection**: Performance metrics and operational statistics

### Health Status Types

```rust
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}
```

## IPC Integration

### Collector IPC Server

The framework includes IPC server capabilities for external communication:

```rust
pub struct CollectorIpcServer {
    // IPC server implementation
}

impl CollectorIpcServer {
    pub async fn start(&mut self) -> Result<()>;
    pub async fn handle_request(&self, request: IpcRequest) -> IpcResponse;
    pub fn get_capabilities(&self) -> SourceCaps;
}
```

### Protocol Support

- **Unix Domain Sockets** (Linux/macOS)
- **Named Pipes** (Windows)
- **Protobuf Serialization** for efficient communication
- **Automatic Reconnection** with exponential backoff

## Event Source Implementation

### Process Event Source Example

```rust
use async_trait::async_trait;
use collector_core::{CollectionEvent, EventSource, ProcessEvent, SourceCaps};

pub struct ProcessEventSource {
    config: ProcessSourceConfig,
    db_manager: Arc<Mutex<DatabaseManager>>,
}

#[async_trait]
impl EventSource for ProcessEventSource {
    fn name(&self) -> &'static str {
        "process-collector"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(self.config.collection_interval);

        while !shutdown_signal.load(Ordering::Relaxed) {
            interval.tick().await;

            // Collect process information
            let processes = self.collect_processes().await?;

            // Send events
            for process in processes {
                let event = CollectionEvent::Process(ProcessEvent {
                    pid: process.pid,
                    name: process.name,
                    timestamp: SystemTime::now(),
                    // ... additional fields
                });

                if tx.send(event).await.is_err() {
                    warn!("Event channel closed, stopping collection");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // Cleanup resources
        Ok(())
    }
}
```

## Advanced Features

### Analysis Chain Coordination

The framework includes analysis chain coordination for complex workflows:

```rust
pub struct AnalysisChainCoordinator {
    // Coordinates multi-stage analysis workflows
}

pub struct AnalysisWorkflowDefinition {
    pub stages: Vec<AnalysisStage>,
    pub dependencies: HashMap<String, Vec<String>>,
    pub timeout: Duration,
}
```

### Trigger Management

Sophisticated trigger management for event-driven analysis:

```rust
pub struct TriggerManager {
    // Manages trigger conditions and priorities
}

pub struct TriggerCondition {
    pub sql_condition: String,
    pub priority: TriggerPriority,
    pub resource_limits: TriggerResourceLimits,
}
```

### Event Bus System

Optional pub/sub event distribution:

```rust
pub struct EventBus {
    // Pub/sub event distribution system
}

pub struct EventSubscription {
    pub filter: EventFilter,
    pub correlation_filter: Option<CorrelationFilter>,
    pub subscriber_id: String,
}
```

## Performance Characteristics

### Throughput Targets

- **Event Processing**: >1,000 events/second
- **CPU Overhead**: \<5% sustained usage
- **Memory Usage**: \<100MB resident under normal operation
- **Latency**: \<100ms per event processing

### Scalability

- **Event Sources**: Support for 16+ concurrent event sources
- **Event Buffer**: Configurable buffer sizes (1,000-10,000 events)
- **Batch Processing**: Configurable batch sizes (100-1,000 events)
- **Backpressure**: Automatic flow control and graceful degradation

## Testing Strategy

### Test Coverage

The framework includes comprehensive testing:

- **Unit Tests**: Individual component functionality
- **Integration Tests**: Cross-component interaction
- **Performance Tests**: Throughput and latency benchmarks
- **Property Tests**: Generative testing for edge cases

### Test Utilities

```rust
// Test utilities for event source testing
pub mod test_utils {
    pub struct MockEventSource;
    pub struct TestCollector;
    pub fn create_test_config() -> CollectorConfig;
}
```

## Usage Examples

### Basic Collector Setup

```rust
use collector_core::{Collector, CollectorConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = CollectorConfig::new()
        .with_component_name("my-collector".to_string())
        .with_max_event_sources(4)
        .with_event_buffer_size(2000);

    let mut collector = Collector::new(config);

    // Register event sources
    collector.register(Box::new(ProcessEventSource::new()))?;
    collector.register(Box::new(NetworkEventSource::new()))?;

    // Run the collector
    collector.run().await
}
```

### Custom Event Source

```rust
use async_trait::async_trait;
use collector_core::{CollectionEvent, EventSource, SourceCaps};

struct CustomEventSource;

#[async_trait]
impl EventSource for CustomEventSource {
    fn name(&self) -> &'static str {
        "custom-source"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PERFORMANCE | SourceCaps::REALTIME
    }

    async fn start(
        &self,
        tx: mpsc::Sender<CollectionEvent>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        // Custom collection logic
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

## Best Practices

### Event Source Development

1. **Implement Graceful Shutdown**: Always check the shutdown signal
2. **Handle Errors Gracefully**: Continue operation when possible
3. **Use Structured Logging**: Provide detailed operational information
4. **Monitor Performance**: Track collection rates and resource usage
5. **Test Thoroughly**: Include unit, integration, and performance tests

### Configuration Management

1. **Use Builder Pattern**: Provide fluent configuration APIs
2. **Validate Configuration**: Check configuration at startup
3. **Support Hierarchical Loading**: Allow multiple configuration sources
4. **Document Defaults**: Clearly document default values
5. **Environment Variable Support**: Enable container-friendly configuration

### Performance Optimization

1. **Batch Events**: Use batching for efficient processing
2. **Monitor Backpressure**: Implement flow control mechanisms
3. **Optimize Hot Paths**: Profile and optimize critical code paths
4. **Use Async I/O**: Leverage Tokio for concurrent operations
5. **Resource Limits**: Implement resource budgets and limits

---

*The collector-core framework provides the foundation for DaemonEye's extensible monitoring architecture, enabling multiple collection strategies while maintaining shared operational infrastructure.*
