# Topic hierarchy

This document describes the topic structure and wildcard semantics used by the DaemonEye event bus.

Wildcard semantics (as enforced by the current matcher):

- Literal segments must match exactly.
- "+" matches exactly one segment.
- "#" matches the remaining segments, and must appear only as the final segment in the pattern.
  - Note: In the current implementation, "#" only matches when there is at least one remaining segment. Patterns like "events.#" will not match the single-segment topic "events"; likewise, "... .#." does not match zero additional segments. The truth-table test below codifies this behavior.

Reference tests

- The matcher’s behavior is locked in by a truth-table snapshot test: daemoneye-eventbus/tests/pattern_truth_table.rs
- Snapshot file: daemoneye-eventbus/tests/snapshots/pattern_truth_table\_\_topic_pattern_matrix_v1.snap
- To run the test and review/update snapshots:
  - cargo test -p daemoneye-eventbus -- tests::pattern_truth_table
  - cargo insta test
  - cargo insta review

Example patterns

- events.process.+ matches events.process.new
- control.trigger.+ matches control.trigger.request
- events.+.# matches any topic that begins with two segments after "events" and then any tail

# DaemonEye Topic Hierarchy Design

## Overview

This document defines the topic hierarchy for multi-collector communication in the DaemonEye system using the daemoneye-eventbus message broker. The design enables flexible pub/sub patterns while maintaining security boundaries and operational clarity.

## Topic Structure Design

### Event Topics (Data Flow)

Event topics follow the pattern: `events.<domain>.<type>[.<subtype>]`

#### Process Events

```text
events.process.lifecycle     # Process start/stop/exit events
events.process.metadata      # Process metadata updates (CPU, memory, etc.)
events.process.tree          # Parent-child relationship changes
events.process.integrity     # Hash verification and integrity checks
events.process.anomaly       # Behavioral anomalies and suspicious patterns
events.process.batch         # Bulk process enumeration results
```

#### Network Events (Future)

```text
events.network.connections   # TCP/UDP connection events
events.network.dns           # DNS query/response events
events.network.traffic       # Network traffic analysis
events.network.anomaly       # Network behavioral anomalies
events.network.security      # Security-relevant network events
```

#### Filesystem Events (Future)

```text
events.filesystem.operations # File create/modify/delete operations
events.filesystem.access     # File access patterns
events.filesystem.integrity  # File integrity monitoring
events.filesystem.anomaly    # Filesystem behavioral anomalies
events.filesystem.bulk       # Bulk filesystem operations
```

#### Performance Events (Future)

```text
events.performance.system    # System-wide performance metrics
events.performance.process   # Per-process performance data
events.performance.resource  # Resource utilization events
events.performance.anomaly   # Performance anomalies
events.performance.threshold # Threshold breach events
```

### Control Topics (Management Flow)

Control topics follow the pattern: `control.<component>.<operation>[.<target>]`

#### Collector Control

```text
control.collector.lifecycle  # Start/stop/restart collector processes
control.collector.config     # Configuration updates and reloads
control.collector.task       # Task assignment and distribution
control.collector.capability # Capability advertisement and discovery
control.collector.status     # Status reporting and heartbeat
```

#### Agent Control

```text
control.agent.orchestration  # Multi-collector orchestration commands
control.agent.detection      # Detection rule management
control.agent.alert          # Alert generation and delivery control
control.agent.correlation    # Cross-domain event correlation
control.agent.policy         # Policy enforcement and updates
```

#### Health Monitoring

```text
control.health.heartbeat     # Component heartbeat messages
control.health.status        # Component status updates
control.health.diagnostics   # Diagnostic information exchange
control.health.metrics       # Performance and operational metrics
control.health.alerts        # Health-related alerts and warnings
```

## Topic Naming Conventions

### Hierarchical Structure Rules

1. **Domain Separation**: First level separates event data (`events`) from control messages (`control`)
2. **Component Identification**: Second level identifies the source/target component
3. **Operation Classification**: Third level specifies the operation or event type
4. **Granular Targeting**: Optional fourth level for specific subtypes or targets

### Naming Standards

- Use lowercase with dot (`.`) separators
- Use descriptive, consistent terminology
- Avoid abbreviations unless widely understood
- Maximum 4 levels deep for readability
- Use plural nouns for collections (e.g., `events`, `operations`)
- Use singular nouns for specific items (e.g., `heartbeat`, `status`)

### Reserved Keywords

- `events.*` - Reserved for data flow topics
- `control.*` - Reserved for management flow topics
- `system.*` - Reserved for system-level topics (future use)
- `debug.*` - Reserved for debugging and development topics

## Wildcarding Patterns

### Subscription Flexibility

The topic hierarchy supports flexible subscription patterns using wildcards:

#### Single-Level Wildcard (`+`)

```text
events.process.+             # All process event types
control.collector.+          # All collector control operations
control.health.+             # All health monitoring topics
```

#### Multi-Level Wildcard (`#`)

```text
events.#                     # All event topics
control.#                    # All control topics
events.process.#             # All process-related events
control.collector.#          # All collector-related control messages
```

#### Specific Domain Patterns

```text
events.+.lifecycle           # Lifecycle events from all domains
events.+.anomaly             # Anomaly events from all domains
control.+.status             # Status messages from all components
control.+.config             # Configuration updates for all components
```

### Common Subscription Patterns

#### For daemoneye-agent (Orchestrator)

```text
events.#                     # Subscribe to all events for correlation
control.agent.#              # Subscribe to agent-specific control messages
control.health.#             # Monitor health of all components
```

#### For Collectors (procmond, netmond, etc.)

```text
control.collector.#          # Subscribe to collector control messages
control.collector.task       # Subscribe to task assignments
control.health.heartbeat     # Participate in health monitoring
```

#### For Monitoring Systems

```text
events.+.anomaly             # Subscribe to anomalies from all domains
control.health.#             # Monitor system health
events.+.security            # Security-relevant events from all domains
```

## Topic Access Patterns

### Publisher Patterns

#### Collectors → Agent (Event Flow)

```text
procmond publishes to:       events.process.*
netmond publishes to:        events.network.*
fsmond publishes to:         events.filesystem.*
perfmond publishes to:       events.performance.*
```

#### Agent → Collectors (Control Flow)

```text
daemoneye-agent publishes to: control.collector.*
                              control.health.heartbeat (response)
```

#### Bidirectional Health Monitoring

```text
All components publish to:    control.health.status
                              control.health.heartbeat
                              control.health.metrics
```

### Subscriber Patterns

#### Agent Subscriptions (Data Aggregation)

```text
daemoneye-agent subscribes to: events.#                    # All events
                               control.agent.#             # Agent control
                               control.health.#            # Health monitoring
```

#### Collector Subscriptions (Task Reception)

```text
procmond subscribes to:        control.collector.#         # Collector control
                               control.collector.task      # Task assignments
                               control.health.heartbeat    # Health checks
```

#### External Monitoring Subscriptions

```text
SIEM systems subscribe to:     events.+.security          # Security events
                               events.+.anomaly           # Anomaly events
                               control.health.alerts      # Health alerts
```

## Security Boundaries

### Access Control Principles

1. **Principle of Least Privilege**: Components only subscribe to topics they need
2. **Data Flow Isolation**: Event topics are read-only for most components
3. **Control Message Authentication**: Control topics require authenticated publishers
4. **Health Monitoring Transparency**: Health topics are broadly accessible for monitoring

### Component Security Boundaries

#### procmond (Privileged Collector)

```text
PUBLISH:   events.process.*           # Process events only
           control.health.status      # Health status reporting
           control.health.heartbeat   # Heartbeat responses

SUBSCRIBE: control.collector.#        # Collector control messages
           control.health.heartbeat   # Health check requests
```

#### daemoneye-agent (Orchestrator)

```text
PUBLISH:   control.collector.*        # Collector management
           control.agent.*            # Agent coordination
           control.health.heartbeat   # Health check requests

SUBSCRIBE: events.#                   # All events for correlation
           control.agent.#            # Agent-specific control
           control.health.#           # System health monitoring
```

#### External Systems (SIEM, Monitoring)

```text
PUBLISH:   control.agent.policy       # Policy updates (authenticated)
           control.agent.detection    # Detection rule updates

SUBSCRIBE: events.+.security          # Security events only
           events.+.anomaly           # Anomaly events only
           control.health.alerts      # Health alerts only
```

### Topic-Level Security

#### Public Topics (Broad Access)

```text
control.health.*                      # Health monitoring
events.*.anomaly                      # Anomaly detection
events.*.security                     # Security events
```

#### Restricted Topics (Component-Specific)

```text
control.collector.config              # Configuration management
control.agent.orchestration           # Internal orchestration
events.*.metadata                     # Detailed metadata
```

#### Privileged Topics (Authenticated Only)

```text
control.collector.lifecycle           # Collector lifecycle management
control.agent.policy                  # Policy enforcement
control.*.config                      # Configuration updates
```

## Implementation Guidelines

### Topic Registration

Components should register their topic usage during startup:

```rust,ignore
// Example topic registration for procmond
let topic_registry = TopicRegistry::new();
topic_registry.register_publisher("events.process.lifecycle");
topic_registry.register_publisher("events.process.metadata");
topic_registry.register_subscriber("control.collector.#");
```

### Message Routing

The daemoneye-eventbus broker should implement efficient routing based on:

1. **Exact Match**: Direct topic name matching for performance
2. **Wildcard Expansion**: Pattern matching for flexible subscriptions
3. **Security Filtering**: Access control validation before message delivery
4. **Load Balancing**: Distribution across multiple subscribers when appropriate

### Error Handling

Topic-related errors should be handled gracefully:

1. **Unknown Topics**: Log warnings but don't reject messages
2. **Access Denied**: Log security violations and reject messages
3. **Subscription Failures**: Retry with exponential backoff
4. **Publisher Failures**: Circuit breaker pattern for reliability

## Migration Strategy

### Phase 1: Core Event Topics

- Implement `events.process.*` hierarchy
- Establish `control.collector.*` and `control.health.*` topics
- Migrate procmond to use new topic structure

### Phase 2: Extended Control Topics

- Add `control.agent.*` hierarchy
- Implement health monitoring topics
- Integrate with daemoneye-agent orchestration

### Phase 3: Future Domain Topics

- Add `events.network.*`, `events.filesystem.*`, `events.performance.*`
- Implement cross-domain correlation patterns
- Support external system integration topics

## Monitoring and Observability

### Topic Metrics

The broker should expose metrics for:

```text
daemoneye_eventbus_messages_published_total{topic}
daemoneye_eventbus_messages_delivered_total{topic}
daemoneye_eventbus_subscribers_active{topic}
daemoneye_eventbus_topic_throughput_messages_per_second{topic}
```

### Topic Health Monitoring

Regular health checks should validate:

1. **Topic Accessibility**: All registered topics are reachable
2. **Subscription Health**: Subscribers are actively consuming messages
3. **Publisher Health**: Publishers are successfully sending messages
4. **Message Flow**: End-to-end message delivery verification

This topic hierarchy design provides a scalable, secure, and flexible foundation for multi-collector communication in the DaemonEye system while maintaining clear operational boundaries and supporting future extensibility.
