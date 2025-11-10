# DaemonEye EventBus Topic Hierarchy

This document describes the complete topic hierarchy for the DaemonEye EventBus, including event topics, control topics, and health monitoring topics.

## Overview

The DaemonEye EventBus uses a hierarchical topic structure to organize messages between components. Topics are organized into two main domains:

- **Event Topics** (`events.*`) - Data flow from collectors to the agent
- **Control Topics** (`control.*`) - Management and coordination messages

## Topic Structure

Topics follow a hierarchical naming convention with up to 4 levels:

```
<domain>.<subdomain>.<type>[.<subtype>]
```

For example:

- `events.process.lifecycle` - Process lifecycle events
- `control.collector.config` - Collector configuration updates
- `control.health.heartbeat` - Health monitoring heartbeats

## Wildcard Support

The EventBus supports two types of wildcards for flexible subscriptions:

- **Single-level wildcard (`+`)**: Matches exactly one segment

  - Example: `events.+.lifecycle` matches `events.process.lifecycle` and `events.network.lifecycle`

- **Multi-level wildcard (`#`)**: Matches zero or more segments (must be at the end)

  - Example: `events.process.#` matches all process events
  - Example: `control.#` matches all control messages

## Event Topics

Event topics carry data from monitoring collectors to the daemoneye-agent for analysis and detection.

### Process Events (`events.process.*`)

Process monitoring events from procmond:

| Topic                      | Description                                  | Publisher | Access Level |
| -------------------------- | -------------------------------------------- | --------- | ------------ |
| `events.process.lifecycle` | Process start, stop, exit events             | procmond  | Restricted   |
| `events.process.metadata`  | Process metadata updates (CPU, memory, etc.) | procmond  | Restricted   |
| `events.process.tree`      | Parent-child relationship changes            | procmond  | Restricted   |
| `events.process.integrity` | Hash verification and integrity checks       | procmond  | Restricted   |
| `events.process.anomaly`   | Behavioral anomalies and suspicious patterns | procmond  | Public       |
| `events.process.batch`     | Bulk process enumeration results             | procmond  | Restricted   |

**Wildcard Pattern**: `events.process.#` - Subscribe to all process events

### Network Events (`events.network.*`)

Network monitoring events from netmond (future extension):

| Topic                        | Description               | Publisher | Access Level |
| ---------------------------- | ------------------------- | --------- | ------------ |
| `events.network.connections` | Network connection events | netmond   | Restricted   |
| `events.network.dns`         | DNS query events          | netmond   | Restricted   |
| `events.network.traffic`     | Network traffic analysis  | netmond   | Restricted   |
| `events.network.anomaly`     | Network anomaly detection | netmond   | Public       |

**Wildcard Pattern**: `events.network.#` - Subscribe to all network events

### Filesystem Events (`events.filesystem.*`)

Filesystem monitoring events from fsmond (future extension):

| Topic                          | Description                           | Publisher | Access Level |
| ------------------------------ | ------------------------------------- | --------- | ------------ |
| `events.filesystem.operations` | File creation, modification, deletion | fsmond    | Restricted   |
| `events.filesystem.access`     | File access pattern tracking          | fsmond    | Restricted   |
| `events.filesystem.bulk`       | Bulk file operation detection         | fsmond    | Restricted   |
| `events.filesystem.anomaly`    | Filesystem anomaly detection          | fsmond    | Public       |

**Wildcard Pattern**: `events.filesystem.#` - Subscribe to all filesystem events

### Performance Events (`events.performance.*`)

Performance monitoring events from perfmond (future extension):

| Topic                            | Description                     | Publisher | Access Level |
| -------------------------------- | ------------------------------- | --------- | ------------ |
| `events.performance.utilization` | Resource utilization metrics    | perfmond  | Restricted   |
| `events.performance.system`      | System-wide performance metrics | perfmond  | Restricted   |
| `events.performance.anomaly`     | Performance anomaly detection   | perfmond  | Public       |

**Wildcard Pattern**: `events.performance.#` - Subscribe to all performance events

## Control Topics

Control topics manage collector lifecycle, configuration, and health monitoring.

### Collector Management (`control.collector.*`)

Collector lifecycle and configuration management:

| Topic                                | Description                             | Publisher       | Access Level |
| ------------------------------------ | --------------------------------------- | --------------- | ------------ |
| `control.collector.lifecycle`        | Start, stop, restart operations         | daemoneye-agent | Privileged   |
| `control.collector.config`           | Configuration updates and reloads       | daemoneye-agent | Privileged   |
| `control.collector.task`             | Base topic for task assignment          | daemoneye-agent | Restricted   |
| `control.collector.task.{type}.{id}` | Collector-specific task distribution    | daemoneye-agent | Restricted   |
| `control.collector.registration`     | Collector registration and capabilities | collectors      | Restricted   |

**Wildcard Pattern**: `control.collector.#` - Subscribe to all collector control messages

### Agent Orchestration (`control.agent.*`)

Agent-level orchestration and policy management:

| Topic                         | Description                    | Publisher       | Access Level |
| ----------------------------- | ------------------------------ | --------------- | ------------ |
| `control.agent.orchestration` | Agent coordination messages    | daemoneye-agent | Restricted   |
| `control.agent.policy`        | Policy updates and enforcement | daemoneye-agent | Privileged   |

**Wildcard Pattern**: `control.agent.#` - Subscribe to all agent control messages

### Health Monitoring (`control.health.*`)

Component health monitoring and diagnostics:

| Topic                        | Description                     | Publisher      | Access Level |
| ---------------------------- | ------------------------------- | -------------- | ------------ |
| `control.health.heartbeat`   | Liveness check heartbeats       | All components | Public       |
| `control.health.status`      | Component status updates        | All components | Public       |
| `control.health.diagnostics` | Diagnostic information exchange | All components | Public       |

**Wildcard Pattern**: `control.health.#` - Subscribe to all health messages

## Access Control

Topics have three access levels that control who can publish and subscribe:

### Public Topics

- Accessible to all components
- Examples: `control.health.*`, `events.*.anomaly`
- No authentication required

### Restricted Topics

- Component-specific access
- Examples: `events.process.*`, `control.collector.task`, `control.collector.task.*.*`
- Requires component registration

### Privileged Topics

- Requires authentication
- Examples: `control.collector.lifecycle`, `control.agent.policy`
- Only daemoneye-agent can publish

## Usage Examples

### Subscribing to Process Events

```rust
use daemoneye_eventbus::{EventBus, EventSubscription, SourceCaps, process};

// Subscribe to all process events
let subscription = EventSubscription {
    subscriber_id: "my-subscriber".to_string(),
    capabilities: SourceCaps {
        event_types: vec!["process".to_string()],
        collectors: vec!["procmond".to_string()],
        max_priority: 5,
    },
    event_filter: None,
    correlation_filter: None,
    topic_patterns: Some(vec![process::ALL.to_string()]),
    enable_wildcards: true,
};

let mut receiver = event_bus.subscribe(subscription).await?;
```

### Publishing to a Specific Topic

```rust
use daemoneye_eventbus::{EventBus, CollectionEvent, ProcessEvent, process};
use std::collections::HashMap;

// Publish a process lifecycle event
let event = CollectionEvent::Process(ProcessEvent {
    pid: 1234,
    name: "example".to_string(),
    command_line: Some("example --arg".to_string()),
    executable_path: Some("/bin/example".to_string()),
    ppid: Some(1),
    start_time: Some(std::time::SystemTime::now()),
    metadata: HashMap::new(),
});

// The broker will route this to events.process.new
event_bus.publish(event, "correlation-id".to_string()).await?;
```

### Using the Topic Builder

```rust
use daemoneye_eventbus::TopicBuilder;

// Build a topic programmatically
let topic = TopicBuilder::events()
    .process()
    .lifecycle()
    .build();

assert_eq!(topic, "events.process.lifecycle");

// Build and validate
let validated_topic = TopicBuilder::control()
    .collector()
    .config()
    .build_validated()?;
```

### Checking Access Levels

```rust
use daemoneye_eventbus::{TopicHierarchy, TopicAccessLevel, process, collector};

// Check access level for a topic
let level = TopicHierarchy::get_access_level(process::LIFECYCLE);
assert_eq!(level, TopicAccessLevel::Restricted);

let level = TopicHierarchy::get_access_level(collector::LIFECYCLE);
assert_eq!(level, TopicAccessLevel::Privileged);
```

## Topic Naming Conventions

When creating new topics, follow these conventions:

1. **Use lowercase**: All topic segments must be lowercase
2. **Use hyphens for multi-word segments**: `events.process.parent-child` (if needed)
3. **Keep it hierarchical**: Organize from general to specific
4. **Be descriptive**: Topic names should clearly indicate their purpose
5. **Avoid deep nesting**: Maximum 4 levels (domain.subdomain.type.subtype)

## Security Boundaries

The topic hierarchy enforces security boundaries:

1. **Collectors** can only publish to their domain-specific event topics

   - procmond → `events.process.*`
   - netmond → `events.network.*`
   - fsmond → `events.filesystem.*`
   - perfmond → `events.performance.*`

2. **daemoneye-agent** can publish to all control topics

   - `control.collector.*`
   - `control.agent.*`

3. **All components** can publish health messages

   - `control.health.*`

4. **Subscription filtering** ensures components only receive relevant events

   - Based on capabilities and topic patterns
   - Enforced by the broker

## Future Extensions

The topic hierarchy is designed to be extensible. Future additions may include:

- **Trigger Topics** (`control.trigger.*`) - For triggerable collectors
- **Analysis Topics** (`events.analysis.*`) - For analysis results
- **Correlation Topics** (`events.correlation.*`) - For multi-domain correlation
- **Security Topics** (`events.security.*`) - For security-specific events

## References

- [Topic Implementation](../src/topic.rs) - Core topic matching and validation
- [Topics Module](../src/topics.rs) - Complete topic hierarchy definitions
- [Broker Implementation](../src/broker.rs) - Message routing and delivery
- [Design Document](../../.kiro/specs/daemoneye-core-monitoring/design.md) - Architecture overview
