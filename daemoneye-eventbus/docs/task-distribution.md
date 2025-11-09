# Task Distribution and Capability-Based Routing

## Overview

This document describes the task distribution and capability-based routing system implemented for the daemoneye-eventbus. This system enables intelligent routing of tasks to appropriate collectors based on their advertised capabilities, with support for priority queuing, load balancing, and dynamic routing updates.

## Architecture

The task distribution system consists of the following components:

### TaskDistributor

The main coordinator that routes tasks to collectors. It provides:

- Task queuing with priority handling
- Capability-based routing to appropriate collector types
- Dynamic routing updates when collectors join/leave
- Fallback routing for unavailable collectors
- Load balancing across multiple collector instances

### CapabilityRegistry

Tracks collector capabilities and availability, including:

- Collector type and supported operations
- Maximum concurrent task capacity
- Supported priority levels
- Current task count and status
- Last heartbeat timestamp

### TaskQueue

Priority-based queue for pending tasks that:

- Orders tasks by priority (higher priority first)
- Uses FIFO ordering for same-priority tasks
- Supports automatic retry with configurable limits
- Handles task expiration based on deadlines

### RoutingStrategy

Pluggable routing algorithms:

- **RoundRobin**: Distributes tasks evenly across collectors
- **LeastLoaded**: Routes to collector with fewest active tasks
- **FirstAvailable**: Uses first available collector
- **Random**: Random selection for load distribution

## Key Features

### 1. Task Distribution Logic

Tasks are distributed using daemoneye-eventbus topic publishing:

```rust
// Task is published to collector-specific topic
let task_topic = format!("control.tasks.{}.{}", collector_type, collector_id);
broker.publish(&task_topic, &correlation_id, payload).await?;
```

### 2. Collector Type Routing

Tasks are routed based on collector capabilities:

```rust
// Find collectors that support the operation
let suitable_collectors = registry
    .values()
    .filter(|reg| {
        reg.capability.supported_operations.contains(&task.operation)
            && reg.capability.priority_levels.contains(&task.priority)
            && reg.status == CollectorStatus::Available
            && reg.current_tasks < reg.capability.max_concurrent_tasks
    })
    .collect();
```

### 3. Task Queuing and Priority Handling

Tasks are queued when no suitable collectors are available:

```rust
// Priority queue with custom ordering
impl Ord for QueuedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority tasks come first
        match self.task.priority.cmp(&other.task.priority) {
            Ordering::Equal => {
                // For same priority, older tasks come first (FIFO)
                other.queued_at.cmp(&self.queued_at)
            }
            other => other,
        }
    }
}
```

### 4. Capability Advertisement and Discovery

Collectors register their capabilities:

```rust
let capability = CollectorCapability {
    collector_id: "procmond-1".to_string(),
    collector_type: "procmond".to_string(),
    supported_operations: vec!["enumerate_processes".to_string()],
    max_concurrent_tasks: 10,
    priority_levels: vec![1, 2, 3, 4, 5],
    metadata: HashMap::new(),
};

distributor.register_collector(capability).await?;
```

### 5. Dynamic Routing Updates

The system automatically updates routing when collectors join or leave:

```rust
// Register new collector
distributor.register_collector(capability).await?;

// Deregister collector
distributor.deregister_collector("procmond-1").await?;

// Update heartbeat to maintain availability
distributor.update_heartbeat("procmond-1").await?;
```

### 6. Fallback Routing

When collectors are unavailable:

- Tasks are queued with priority ordering
- Automatic retry with exponential backoff
- Health checks detect and recover unhealthy collectors
- Queue processing resumes when collectors become available

## Usage Example

```rust
use daemoneye_eventbus::{CollectorCapability, RoutingStrategy, TaskDistributor, TaskRequest};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create broker and distributor
    let broker = Arc::new(DaemoneyeBroker::new("/tmp/eventbus.sock").await?);
    let distributor = TaskDistributor::new(broker).await?;

    // Register collector capabilities
    let capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec!["enumerate_processes".to_string()],
        max_concurrent_tasks: 10,
        priority_levels: vec![1, 2, 3, 4, 5],
        metadata: HashMap::new(),
    };
    distributor.register_collector(capability).await?;

    // Set routing strategy
    distributor
        .set_routing_strategy(RoutingStrategy::LeastLoaded)
        .await;

    // Distribute a task
    let task = TaskRequest {
        task_id: "task-1".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 3,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let collector_id = distributor.distribute_task(task).await?;
    println!("Task distributed to: {}", collector_id);

    // Start background tasks for queue processing and health checks
    distributor.start_background_tasks().await?;

    Ok(())
}
```

## Configuration

The TaskDistributor can be configured with:

```rust
let distributor = TaskDistributor::with_config(
    broker,
    max_queue_size: 10000,        // Maximum queued tasks
    max_retries: 3,                // Maximum retry attempts
    heartbeat_timeout: Duration::from_secs(30),  // Heartbeat timeout
    routing_strategy: RoutingStrategy::LeastLoaded,
).await?;
```

## Monitoring and Statistics

The system provides comprehensive statistics:

```rust
let stats = distributor.get_stats().await;
println!("Tasks distributed: {}", stats.tasks_distributed);
println!("Tasks queued: {}", stats.tasks_queued);
println!("Tasks failed: {}", stats.tasks_failed);
println!("Active collectors: {}", stats.active_collectors);
println!("Tasks by type: {:?}", stats.tasks_by_type);
```

## Integration with Requirements

This implementation satisfies the following requirements from the specification:

- **Requirement 15.1**: Task distribution using daemoneye-eventbus topic publishing
- **Requirement 15.3**: Capability-based routing and task distribution
- **Requirement 16.1**: Dynamic collector registration and capability advertisement

The system provides:

1. ✅ Task distribution logic using daemoneye-eventbus topic publishing
2. ✅ Collector type routing based on capabilities
3. ✅ Task queuing and priority handling
4. ✅ Capability advertisement and discovery system
5. ✅ Routing logic based on collector capabilities
6. ✅ Dynamic routing updates when collectors join/leave
7. ✅ Fallback routing for unavailable collectors

## Testing

Comprehensive integration tests verify:

- Task distribution with multiple collectors
- Capability-based routing to appropriate collector types
- Priority queue ordering
- Different routing strategies (RoundRobin, LeastLoaded, FirstAvailable, Random)
- Collector registration and deregistration
- Task completion tracking and capacity management
- Heartbeat and health check mechanisms

All tests pass successfully, demonstrating the robustness of the implementation.
