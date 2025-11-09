# Multi-Collector Coordination System

## Overview

The multi-collector coordination system enables distributed task distribution, capability-based routing, result aggregation, load balancing, and failover across multiple collector instances in the DaemonEye monitoring infrastructure.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                   Multi-Collector Coordination                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │
│  │ Task         │  │ Capability   │  │ Result       │        │
│  │ Distributor  │  │ Router       │  │ Aggregator   │        │
│  └──────────────┘  └──────────────┘  └──────────────┘        │
│         │                  │                  │                │
│         └──────────────────┼──────────────────┘                │
│                            │                                   │
│                   ┌────────┴────────┐                          │
│                   │ Load Balancer   │                          │
│                   │ with Failover   │                          │
│                   └─────────────────┘                          │
│                            │                                   │
├────────────────────────────┼───────────────────────────────────┤
│                            │                                   │
│  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌──────────┐  │
│  │procmond-1│   │procmond-2│   │netmond-1 │   │fsmond-1  │  │
│  └──────────┘   └──────────┘   └──────────┘   └──────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Components

### 1. Task Distributor (`task_distributor.rs`)

Handles topic-based task distribution with priority queuing.

**Features:**

- Priority-based task queuing (Critical, High, Normal, Low)
- Topic-based routing to collector domains
- Task timeout tracking
- Distribution statistics

**Usage:**

```rust
use collector_core::{TaskDistributor, TaskPriority, DistributionTask};
use daemoneye_eventbus::DaemoneyeBroker;
use std::sync::Arc;

let broker = Arc::new(DaemoneyeBroker::new("/tmp/eventbus.sock").await?);
let distributor = TaskDistributor::new(broker);

// Create and distribute a task
let task = distributor.create_task_from_event(
    &event,
    TaskPriority::High,
    Duration::from_secs(30)
)?;

distributor.distribute_task(task).await?;
```

### 2. Capability Router (`capability_router.rs`)

Provides capability-based routing and dynamic collector discovery.

**Features:**

- Collector capability registration and discovery
- Health-based routing decisions
- Heartbeat monitoring and stale collector detection
- Routing confidence scoring

**Usage:**

```rust
use collector_core::{CapabilityRouter, CollectorCapability, SourceCaps};

let router = CapabilityRouter::new(Duration::from_secs(30));

// Register a collector
let capability = CollectorCapability {
    collector_id: "procmond-1".to_string(),
    capabilities: SourceCaps::PROCESS | SourceCaps::REALTIME,
    // ... other fields
};
router.register_collector(capability).await?;

// Route a task
let decision = router.route_task(SourceCaps::PROCESS).await?;
println!("Selected collector: {}", decision.collector_id);
```

### 3. Result Aggregator (`result_aggregator.rs`)

Aggregates results from multiple collectors with correlation tracking.

**Features:**

- Correlation-based result aggregation
- Result deduplication and ordering
- Timeout handling for incomplete aggregations
- Streaming result collection

**Usage:**

```rust
use collector_core::{ResultAggregator, AggregationConfig, CollectorResult};

let config = AggregationConfig::default();
let aggregator = ResultAggregator::new(config);

// Start aggregation
let correlation_id = "task-123".to_string();
aggregator.start_aggregation(correlation_id.clone(), Some(3)).await?;

// Add results as they arrive
let result = CollectorResult { /* ... */ };
if let Some(completed) = aggregator.add_result(&correlation_id, result).await? {
    println!("Aggregation complete with {} results", completed.results.len());
}
```

### 4. Load Balancer (`load_balancer.rs`)

Provides load balancing and failover for collector instances.

**Features:**

- Multiple load balancing strategies (Round-robin, Least connections, Weighted, Random)
- Automatic failover on collector failure
- Task redistribution
- Load statistics and monitoring

**Usage:**

```rust
use collector_core::{LoadBalancer, LoadBalancerConfig, LoadBalancingStrategy};

let config = LoadBalancerConfig {
    strategy: LoadBalancingStrategy::LeastConnections,
    failover_threshold: 3,
    ..Default::default()
};
let balancer = LoadBalancer::new(config);

// Register collectors
balancer.register_collector("collector-1".to_string(), 1.0).await?;

// Select a collector for task
let selected = balancer.select_collector(
    SourceCaps::PROCESS,
    &available_collectors
).await?;

// Record failures and trigger failover if needed
if balancer.record_failure("collector-1").await? {
    let event = balancer.trigger_failover(
        "collector-1",
        &available_collectors,
        SourceCaps::PROCESS
    ).await?;
}
```

## Workflow Example

Complete multi-collector coordination workflow:

```rust
use collector_core::*;
use daemoneye_eventbus::DaemoneyeBroker;
use std::sync::Arc;

async fn coordinate_collectors() -> Result<()> {
    // 1. Initialize components
    let broker = Arc::new(DaemoneyeBroker::new("/tmp/eventbus.sock").await?);
    let distributor = TaskDistributor::new(Arc::clone(&broker));
    let router = CapabilityRouter::new(Duration::from_secs(30));
    let aggregator = ResultAggregator::new(AggregationConfig::default());
    let balancer = LoadBalancer::new(LoadBalancerConfig::default());

    // 2. Register collectors
    let collector = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        capabilities: SourceCaps::PROCESS,
        // ... other fields
    };
    router.register_collector(collector).await?;
    balancer
        .register_collector("procmond-1".to_string(), 1.0)
        .await?;

    // 3. Create and route task
    let event = CollectionEvent::Process(/* ... */);
    let task =
        distributor.create_task_from_event(&event, TaskPriority::High, Duration::from_secs(30))?;

    let routing_decision = router.route_task(SourceCaps::PROCESS).await?;

    // 4. Distribute task with load balancing
    let selected = balancer
        .select_collector(SourceCaps::PROCESS, &available_collectors)
        .await?;

    distributor.distribute_task(task).await?;

    // 5. Aggregate results
    let correlation_id = task.correlation_id.clone();
    aggregator
        .start_aggregation(correlation_id.clone(), Some(1))
        .await?;

    // Results arrive asynchronously...
    let result = CollectorResult { /* ... */ };
    if let Some(completed) = aggregator.add_result(&correlation_id, result).await? {
        println!("Task completed with {} results", completed.results.len());
    }

    Ok(())
}
```

## Testing

Comprehensive integration tests are available in `tests/multi_collector_coordination.rs`:

```bash
cargo test --test multi_collector_coordination
```

Tests cover:

- Multi-collector task distribution
- Capability-based routing
- Result aggregation
- Load balancing strategies
- Failover mechanisms
- Complete coordination workflows
- Timeout handling
- Stale collector detection

## Performance Considerations

- **Task Distribution**: Sub-millisecond latency for task routing
- **Result Aggregation**: Configurable timeout and buffer sizes
- **Load Balancing**: Minimal overhead with efficient selection algorithms
- **Failover**: Automatic detection and recovery within seconds

## Requirements Satisfied

This implementation satisfies the following requirements:

- **15.1**: Topic-based task distribution for multiple collector types
- **15.3**: Capability-based routing with dynamic discovery
- **15.4**: Result aggregation from multiple collectors
- **16.1**: Load balancing across collector instances
- **16.3**: Failover detection and automatic recovery

## Future Enhancements

- Advanced load balancing algorithms (predictive, ML-based)
- Cross-datacenter collector coordination
- Enhanced result streaming with backpressure
- Collector affinity and locality-aware routing
- Real-time performance metrics and dashboards
