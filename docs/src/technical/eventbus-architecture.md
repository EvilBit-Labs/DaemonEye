# EventBus Architecture

## Overview

The daemoneye-eventbus provides a local cross-platform IPC pub/sub messaging system designed specifically for multi-collector coordination within a single DaemonEye system. It enables collectors to communicate with the daemoneye-agent on the same host through a hierarchical topic structure with support for workflow tracking and forensic analysis.

**Scope**: The EventBus is designed for local IPC communication only. All collectors and the agent must run on the same system. For multi-system deployments, each system runs its own independent EventBus broker within its daemoneye-agent instance.

## Key Features

- **Cross-platform transport**: Unix domain sockets (Linux/macOS) and named pipes (Windows)
- **Hierarchical topics**: Up to 4-level topic structure with wildcard matching
- **Correlation metadata**: Track events across multi-collector workflows
- **Embedded broker**: Runs within daemoneye-agent orchestrator
- **Access control**: Three-level access control (Public, Restricted, Privileged)
- **High performance**: 10,000+ messages/second throughput, sub-millisecond latency

## Topic Hierarchy

### Event Topics (Data Flow)

Event topics carry data from monitoring collectors to the daemoneye-agent for analysis and detection.

#### Process Events (`events.process.*`)

| Topic                      | Description                                  | Publisher | Access Level |
| -------------------------- | -------------------------------------------- | --------- | ------------ |
| `events.process.lifecycle` | Process start, stop, exit events             | procmond  | Restricted   |
| `events.process.metadata`  | Process metadata updates (CPU, memory)       | procmond  | Restricted   |
| `events.process.tree`      | Parent-child relationship changes            | procmond  | Restricted   |
| `events.process.integrity` | Hash verification and integrity checks       | procmond  | Restricted   |
| `events.process.anomaly`   | Behavioral anomalies and suspicious patterns | procmond  | Public       |
| `events.process.batch`     | Bulk process enumeration results             | procmond  | Restricted   |

**Wildcard Pattern**: `events.process.#` - Subscribe to all process events

#### Future Extensions

- `events.network.*` - Network monitoring events (netmond)
- `events.filesystem.*` - Filesystem monitoring events (fsmond)
- `events.performance.*` - Performance monitoring events (perfmond)

### Control Topics (Management Flow)

Control topics manage collector lifecycle, configuration, and health monitoring.

#### Collector Management (`control.collector.*`)

| Topic                            | Description                             | Publisher       | Access Level |
| -------------------------------- | --------------------------------------- | --------------- | ------------ |
| `control.collector.lifecycle`    | Start, stop, restart operations         | daemoneye-agent | Privileged   |
| `control.collector.config`       | Configuration updates and reloads       | daemoneye-agent | Privileged   |
| `control.collector.task`         | Task assignment and distribution        | daemoneye-agent | Restricted   |
| `control.collector.registration` | Collector registration and capabilities | collectors      | Restricted   |

**Wildcard Pattern**: `control.collector.#` - Subscribe to all collector control messages

#### Agent Orchestration (`control.agent.*`)

| Topic                         | Description                    | Publisher       | Access Level |
| ----------------------------- | ------------------------------ | --------------- | ------------ |
| `control.agent.orchestration` | Agent coordination messages    | daemoneye-agent | Restricted   |
| `control.agent.policy`        | Policy updates and enforcement | daemoneye-agent | Privileged   |

#### Health Monitoring (`control.health.*`)

| Topic                        | Description                     | Publisher      | Access Level |
| ---------------------------- | ------------------------------- | -------------- | ------------ |
| `control.health.heartbeat`   | Liveness check heartbeats       | All components | Public       |
| `control.health.status`      | Component status updates        | All components | Public       |
| `control.health.diagnostics` | Diagnostic information exchange | All components | Public       |

**Wildcard Pattern**: `control.health.#` - Subscribe to all health messages

## Wildcard Matching

The EventBus supports two types of wildcards for flexible subscriptions:

### Single-level Wildcard (`+`)

Matches exactly one segment:

```rust,ignore
// Subscribe to lifecycle events from all collector types
let pattern = "events.+.lifecycle";
// Matches: events.process.lifecycle, events.network.lifecycle
// Does not match: events.process.metadata
```

### Multi-level Wildcard (`#`)

Matches zero or more segments (must be at the end):

```rust,ignore
// Subscribe to all process events
let pattern = "events.process.#";
// Matches: events.process.lifecycle, events.process.metadata, events.process.tree

// Subscribe to all control messages
let pattern = "control.#";
// Matches: control.collector.lifecycle, control.health.heartbeat
```

## Correlation Metadata

The EventBus supports comprehensive correlation tracking for multi-collector workflows and forensic analysis.

### Basic Correlation

```rust,ignore
use daemoneye_eventbus::CorrelationMetadata;

// Create correlation metadata
let metadata = CorrelationMetadata::new("workflow-123".to_string())
    .with_stage("collection".to_string())
    .with_tag("source".to_string(), "procmond".to_string());
```

### Hierarchical Correlation

Track events across multiple stages of analysis:

```rust,ignore
// Root correlation for entire workflow
let root_metadata = CorrelationMetadata::new("threat-analysis-001".to_string())
    .with_stage("detection".to_string())
    .with_tag("workflow".to_string(), "suspicious_process_analysis".to_string());

// Collection stage
let collection_metadata = root_metadata.create_child("collection-001".to_string())
    .with_stage("collection".to_string());

// Analysis stage
let analysis_metadata = collection_metadata.create_child("analysis-001".to_string())
    .with_stage("analysis".to_string());

// Alert generation stage
let alert_metadata = analysis_metadata.create_child("alert-001".to_string())
    .with_stage("alerting".to_string());
```

### Correlation Filtering

Filter events based on correlation metadata:

```rust,ignore
use daemoneye_eventbus::{CorrelationFilter, EventSubscription};

// Filter by root correlation ID (entire workflow)
let filter = CorrelationFilter::new()
    .with_root_id("threat-analysis-001".to_string())
    .with_stage("analysis".to_string())
    .with_required_tag("workflow".to_string(), "suspicious_process_analysis".to_string());

// Use in subscription
let subscription = EventSubscription {
    subscriber_id: "forensic-analyzer".to_string(),
    correlation_filter: Some(filter),
    topic_patterns: Some(vec!["events.process.#".to_string()]),
    enable_wildcards: true,
    // ... other fields
};
```

### Use Cases

#### Multi-Collector Workflows (Local System)

Track events across multiple collectors in a cascading analysis workflow on the same system:

```rust,ignore
// Root correlation for entire local workflow
let root_correlation = CorrelationMetadata::new("workflow-id".to_string())
    .with_stage("detection".to_string())
    .with_tag("workflow".to_string(), "suspicious_process_analysis".to_string())
    .with_tag("host".to_string(), "localhost".to_string());

// Process collection stage (procmond)
let collection_correlation = root_correlation.create_child("collection-id".to_string())
    .with_stage("collection".to_string());

// Binary analysis stage (local analyzer)
let analysis_correlation = collection_correlation.create_child("analysis-id".to_string())
    .with_stage("analysis".to_string());

// Alert generation stage (daemoneye-agent)
let alert_correlation = analysis_correlation.create_child("alert-id".to_string())
    .with_stage("alerting".to_string());
```

#### Forensic Investigation Tracking (Local System)

Track all events related to a security investigation on a single host:

```rust,ignore
let investigation_id = "incident-2024-001";
let forensic_metadata = CorrelationMetadata::new(investigation_id.to_string())
    .with_stage("forensic_analysis".to_string())
    .with_tag("investigation_id".to_string(), investigation_id.to_string())
    .with_tag("analyst".to_string(), "security_team".to_string())
    .with_tag("severity".to_string(), "critical".to_string())
    .with_tag("incident_type".to_string(), "malware_detection".to_string())
    .with_tag("host".to_string(), "compromised-server-01".to_string());

// Filter for all local events in this investigation
let forensic_filter = CorrelationFilter::new()
    .with_required_tag("investigation_id".to_string(), investigation_id.to_string())
    .with_required_tag("severity".to_string(), "critical".to_string());
```

#### Local Workflow Tracing

Track events across local collector components on a single system:

```rust,ignore
let trace_metadata = CorrelationMetadata::new("trace-id".to_string())
    .with_stage("local_collection".to_string())
    .with_tag("component".to_string(), "procmond".to_string())
    .with_tag("host".to_string(), "server-01".to_string())
    .with_tag("collector_type".to_string(), "process_monitor".to_string());
```

## Access Control

Topics have three access levels that control who can publish and subscribe:

### Public Topics

- Accessible to all components
- Examples: `control.health.*`, `events.*.anomaly`
- No authentication required

### Restricted Topics

- Component-specific access
- Examples: `events.process.*`, `control.collector.task`
- Requires component registration

### Privileged Topics

- Requires authentication
- Examples: `control.collector.lifecycle`, `control.agent.policy`
- Only daemoneye-agent can publish

## Embedded Broker

The daemoneye-agent runs an embedded EventBus broker that:

### Broker Responsibilities

- **Topic Management**: Manages topic subscriptions and message routing
- **Access Control**: Enforces topic-level access control policies
- **Correlation Tracking**: Tracks correlation metadata for workflow coordination
- **Statistics**: Provides message statistics and health monitoring
- **Connection Management**: Manages client connections and reconnection

### Broker Configuration

The broker is configured through the daemoneye-agent configuration:

```yaml
eventbus:
  socket_path: /tmp/daemoneye-broker.sock  # Unix socket path
  max_connections: 100                      # Maximum concurrent connections
  message_buffer_size: 10000                # Message buffer size per subscriber
  enable_statistics: true                   # Enable statistics collection
```

### Broker Statistics

The broker tracks the following statistics:

- Messages published per topic
- Messages delivered per subscriber
- Active subscribers per topic
- Message delivery latency
- Connection count and status

## Performance Characteristics

### Throughput

- **Message Rate**: 10,000+ messages per second
- **Concurrent Subscribers**: 100+ subscribers per topic
- **Message Size**: Up to 1MB per message

### Latency

- **Local IPC**: Sub-millisecond latency
- **Message Routing**: < 100μs per message
- **Correlation Lookup**: < 10μs per filter

### Resource Usage

- **Memory**: < 10MB per 1000 subscribers
- **CPU**: < 1% for message routing
- **Disk**: No persistent storage (in-memory only)

## Security Considerations

### Transport Security

- **Unix Sockets**: File permissions 0600 (owner read/write only)
- **Named Pipes**: Appropriate Windows security descriptors
- **Process Isolation**: Each collector runs in separate process

### Message Security

- **Serialization**: Uses bincode for efficient binary protocol
- **Validation**: All incoming messages validated
- **Size Limits**: Enforce maximum message sizes (1MB default)

### Access Control

- **Topic-level**: Enforced by broker based on component identity
- **Component Registration**: Required for restricted topics
- **Authentication**: Required for privileged topics

## Integration Examples

### Publishing Events

```rust,ignore
use daemoneye_eventbus::{
    CollectionEvent, DaemoneyeEventBus, ProcessEvent, CorrelationMetadata,
};

// Create correlation metadata
let metadata = CorrelationMetadata::new("workflow-123".to_string())
    .with_stage("collection".to_string())
    .with_tag("source".to_string(), "procmond".to_string());

// Create process event
let event = CollectionEvent::Process(ProcessEvent {
    pid: 1234,
    name: "suspicious_process".to_string(),
    // ... other fields
});

// Publish with correlation metadata
event_bus.publish(event, metadata.correlation_id.clone()).await?;
```

### Subscribing to Events

```rust,ignore
use daemoneye_eventbus::{EventSubscription, SourceCaps, CorrelationFilter};

// Create subscription with correlation filter
let subscription = EventSubscription {
    subscriber_id: "forensic-analyzer".to_string(),
    capabilities: SourceCaps {
        event_types: vec!["process".to_string()],
        collectors: vec!["procmond".to_string()],
        max_priority: 5,
    },
    event_filter: None,
    correlation_filter: Some(
        CorrelationFilter::new()
            .with_root_id("workflow-123".to_string())
            .with_required_tag("workflow".to_string(), "threat_analysis".to_string())
    ),
    topic_patterns: Some(vec!["events.process.#".to_string()]),
    enable_wildcards: true,
};

let mut receiver = event_bus.subscribe(subscription).await?;

// Receive events
while let Some(bus_event) = receiver.recv().await {
    println!("Received event: {:?}", bus_event);
}
```

## Troubleshooting

### Common Issues

#### Connection Refused

**Symptom**: Cannot connect to EventBus broker

**Solution**:

1. Verify daemoneye-agent is running
2. Check socket path configuration
3. Verify file permissions on Unix socket

#### No Messages Received

**Symptom**: Subscriber not receiving messages

**Solution**:

1. Verify topic pattern matches published topics
2. Check correlation filter criteria
3. Verify access control permissions

#### High Latency

**Symptom**: Message delivery is slow

**Solution**:

1. Check message buffer size configuration
2. Verify subscriber is processing messages quickly
3. Monitor broker statistics for bottlenecks

## Further Reading

- [Topic Hierarchy Documentation](../../daemoneye-eventbus/docs/topic-hierarchy.md)
- [Correlation Metadata Documentation](../../daemoneye-eventbus/docs/correlation-metadata.md)
- [EventBus API Reference](../../daemoneye-eventbus/README.md)
