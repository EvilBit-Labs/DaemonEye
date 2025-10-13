# Topic Hierarchy Integration Guide

## Overview

This guide explains how to integrate the new topic hierarchy design with existing DaemonEye components and the daemoneye-eventbus message broker.

## Integration Points

### 1. Collector-Core Framework Integration

The topic hierarchy is designed to work seamlessly with the collector-core framework:

```rust
use daemoneye_eventbus::{DaemoneyeEventBus, TopicRegistry, Topic};
use collector_core::{Collector, EventSource, CollectionEvent};

// Register collector topics during startup
let mut topic_registry = TopicRegistry::new();

// procmond registration
topic_registry.register_publisher("procmond", "events.process.lifecycle")?;
topic_registry.register_publisher("procmond", "events.process.metadata")?;
topic_registry.register_subscriber("procmond", "control.collector.#")?;

// daemoneye-agent registration
topic_registry.register_subscriber("daemoneye-agent", "events.#")?;
topic_registry.register_publisher("daemoneye-agent", "control.collector.lifecycle")?;
```

### 2. Event Publishing Pattern

Components should publish events using structured topic names:

```rust
// procmond publishing process events
let event = CollectionEvent::Process(ProcessEvent {
    pid: 1234,
    name: "example_process".to_string(),
    // ... other fields
});

// Publish to structured topic
event_bus.publish_to_topic(event, "events.process.lifecycle", correlation_id).await?;
```

### 3. Subscription Patterns

Components can subscribe using wildcard patterns for flexible event consumption:

```rust
// daemoneye-agent subscribing to all events
let subscription = EventSubscription {
    subscriber_id: "daemoneye-agent".to_string(),
    topic_patterns: Some(vec!["events.#".to_string()]),
    enable_wildcards: true,
    // ... other fields
};

let receiver = event_bus.subscribe(subscription).await?;
```

## Migration Strategy

### Phase 1: Core Process Events (Current)

- Implement `events.process.*` hierarchy
- Migrate procmond to use structured topics
- Update daemoneye-agent to subscribe to process event topics

### Phase 2: Control Topics

- Add `control.collector.*` and `control.health.*` topics
- Implement collector lifecycle management via topics
- Add health monitoring topic patterns

### Phase 3: Future Domains

- Add `events.network.*`, `events.filesystem.*`, `events.performance.*`
- Implement cross-domain correlation patterns
- Support external system integration topics

## Topic Naming Best Practices

### Event Topics

```
events.process.lifecycle     # Process start/stop/exit events
events.process.metadata      # Process metadata updates
events.process.tree          # Parent-child relationship changes
events.process.integrity     # Hash verification results
events.process.anomaly       # Behavioral anomalies
events.process.batch         # Bulk enumeration results
```

### Control Topics

```
control.collector.lifecycle  # Start/stop/restart commands
control.collector.config     # Configuration updates
control.collector.task       # Task assignment
control.health.heartbeat     # Health check requests/responses
control.health.status        # Component status updates
```

## Security Boundaries

### Access Control Implementation

```rust
// Check publishing permissions
if !topic_registry.can_publish("procmond", "events.process.lifecycle") {
    return Err(EventBusError::topic("Publishing not allowed"));
}

// Check subscription permissions
let topic = Topic::new("events.process.lifecycle")?;
if !topic_registry.can_subscribe("siem-connector", &topic) {
    return Err(EventBusError::topic("Subscription not allowed"));
}

// Validate access level
match topic_registry.get_access_level("control.collector.lifecycle") {
    TopicAccessLevel::Privileged => {
        // Require authentication
        validate_authentication()?;
    }
    _ => {}
}
```

### Component Boundaries

- **procmond**: Publishes to `events.process.*`, subscribes to `control.collector.*`
- **daemoneye-agent**: Subscribes to `events.#`, publishes to `control.*`
- **External Systems**: Limited to security-relevant event topics only

## Performance Considerations

### Topic Routing Optimization

The TopicMatcher provides efficient routing:

```rust
// O(1) exact match lookup
let subscribers = topic_matcher.find_subscribers("events.process.lifecycle")?;

// Efficient wildcard matching with early termination
let pattern = TopicPattern::new("events.process.+")?;
let matches = pattern.matches(&topic);
```

### Statistics and Monitoring

```rust
// Track topic usage
topic_matcher.record_publication("events.process.lifecycle");
topic_matcher.record_delivery("events.process.lifecycle");

// Get performance metrics
let stats = topic_matcher.get_topic_stats("events.process.lifecycle");
println!("Published: {}, Delivered: {}",
         stats.messages_published, stats.messages_delivered);
```

## Error Handling

### Topic Validation

```rust
// Validate topic format during registration
match Topic::new("invalid..topic") {
    Ok(topic) => { /* use topic */ }
    Err(TopicError::EmptySegment(pos)) => {
        eprintln!("Empty segment at position {}", pos);
    }
    Err(e) => {
        eprintln!("Topic validation error: {}", e);
    }
}
```

### Pattern Matching Errors

```rust
// Handle pattern creation errors
match TopicPattern::new("invalid.#.pattern") {
    Ok(pattern) => { /* use pattern */ }
    Err(TopicError::InvalidFormat(msg)) => {
        eprintln!("Invalid pattern format: {}", msg);
    }
    Err(e) => {
        eprintln!("Pattern error: {}", e);
    }
}
```

## Testing Integration

### Unit Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_topic_registration() {
        let mut registry = TopicRegistry::new();

        // Test procmond registration
        registry
            .register_publisher("procmond", "events.process.lifecycle")
            .unwrap();
        assert!(registry.can_publish("procmond", "events.process.lifecycle"));

        // Test access control
        assert_eq!(
            registry.get_access_level("control.health.status"),
            TopicAccessLevel::Public
        );
    }
}
```

### Integration Testing

```rust
#[tokio::test]
async fn test_end_to_end_topic_flow() {
    let broker = DaemoneyeBroker::new("/tmp/test.sock").await?;
    let mut event_bus = DaemoneyeEventBus::from_broker(broker).await?;

    // Subscribe to process events
    let subscription = EventSubscription {
        subscriber_id: "test-subscriber".to_string(),
        topic_patterns: Some(vec!["events.process.+".to_string()]),
        enable_wildcards: true,
        // ... other fields
    };
    let mut receiver = event_bus.subscribe(subscription).await?;

    // Publish event
    let event = CollectionEvent::Process(/* ... */);
    event_bus
        .publish_to_topic(event, "events.process.lifecycle", "test-correlation")
        .await?;

    // Verify delivery
    let received = receiver.recv().await.unwrap();
    assert_eq!(received.topic, "events.process.lifecycle");
}
```

## Future Extensions

### Multi-Collector Coordination

```rust
// Future: Network collector integration
topic_registry.register_publisher("netmond", "events.network.connections")?;
topic_registry.register_subscriber("daemoneye-agent", "events.network.#")?;

// Cross-domain correlation
let correlation_pattern = TopicPattern::new("events.+.anomaly")?;
```

### External System Integration

```rust
// SIEM connector with limited access
topic_registry.register_subscriber("siem-connector", "events.+.security")?;
topic_registry.register_subscriber("siem-connector", "events.+.anomaly")?;

// Prevent unauthorized publishing
assert!(!topic_registry.can_publish("siem-connector", "control.collector.config"));
```

This integration guide provides a comprehensive roadmap for implementing the topic hierarchy design within the existing DaemonEye architecture while maintaining security boundaries and performance requirements.
