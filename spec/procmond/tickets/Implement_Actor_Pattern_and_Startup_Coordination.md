# Implement Actor Pattern and Startup Coordination

## Overview

Refactor ProcmondMonitorCollector to use actor pattern for coordinated state management and implement startup coordination with daemoneye-agent. This ticket replaces LocalEventBus with DaemoneyeEventBus (via EventBusConnector) and establishes the message-passing architecture for RPC coordination.

## Scope

**In Scope:**

- Actor pattern implementation in ProcmondMonitorCollector
- ActorMessage enum for message-based coordination
- Bounded mpsc channel (capacity: 100) for actor messages
- Replace LocalEventBus with EventBusConnector
- Startup coordination: wait for "begin monitoring" command
- Dynamic interval adjustment from backpressure
- Configuration hot-reload at cycle boundaries
- Enhanced health check with event bus connectivity
- Update main.rs for actor initialization

**Out of Scope:**

- RPC service handler implementation (Ticket 3)
- Registration and heartbeat (Ticket 3)
- Agent-side loading state (Ticket 4)
- Comprehensive testing (Ticket 5)

## Technical Details

### Actor Pattern Architecture

**Modified Component:** `file:procmond/src/monitor_collector.rs`

**Key Changes:**

- Run in dedicated task with message processing loop
- Receive messages via bounded mpsc channel (capacity: 100)
- Process messages sequentially (no concurrent state mutations)
- Respond via oneshot channels for request/response patterns
- Maintain collection state without complex locking

**ActorMessage Enum:**

```rust
enum ActorMessage {
    HealthCheck {
        respond_to: oneshot::Sender<HealthCheckData>,
    },
    UpdateConfig {
        config: Config,
        respond_to: oneshot::Sender<Result<()>>,
    },
    GracefulShutdown {
        respond_to: oneshot::Sender<Result<()>>,
    },
    BeginMonitoring, // From agent after loading state
    AdjustInterval {
        new_interval: Duration,
    }, // From EventBusConnector backpressure
}
```

### Startup Coordination

**Flow:**

1. procmond starts and connects to broker
2. procmond subscribes to `control.collector.lifecycle` topic
3. procmond waits for "begin monitoring" broadcast from agent
4. Upon receiving command, procmond starts collection loop

**Why:** Ensures agent has completed loading state (all collectors ready, privileges dropped) before procmond begins monitoring.

### Configuration Hot-Reload

**Strategy:** Apply configuration changes at cycle boundaries (atomic)

**Implementation:**

- Config update message queued in actor's channel
- Actor processes message at start of next collection cycle
- Ensures no mid-cycle configuration changes
- Some configs may require restart (documented)

```mermaid
sequenceDiagram
    participant Main as main.rs
    participant Actor as ProcmondMonitorCollector (Actor)
    participant EventBus as EventBusConnector
    participant Collector as ProcessCollector
    participant Lifecycle as LifecycleTracker
    
    Note over Main,Lifecycle: Initialization
    Main->>Main: Create bounded mpsc channel (capacity: 100)
    Main->>EventBus: Initialize with WAL
    Main->>Actor: Create with channel receiver
    Main->>Actor: Pass EventBusConnector
    
    Note over Main,Lifecycle: Startup Coordination
    Main->>EventBus: Subscribe to control.collector.lifecycle
    EventBus->>Main: Receive "begin monitoring" broadcast
    Main->>Actor: Send BeginMonitoring message
    Actor->>Actor: Start collection loop
    
    Note over Main,Lifecycle: Collection Loop
    loop Every collection interval
        Actor->>Collector: Collect processes
        Collector-->>Actor: ProcessEvent list
        Actor->>Lifecycle: Update and detect changes
        Lifecycle-->>Actor: ProcessLifecycleEvent list
        Actor->>EventBus: Publish events
        EventBus->>EventBus: Write to WAL, buffer, publish
    end
    
    Note over Main,Lifecycle: Backpressure
    EventBus->>EventBus: Buffer reaches 70%
    EventBus->>Actor: Send AdjustInterval message
    Actor->>Actor: Increase collection interval (1.5x)
    Note over Actor: Collection slows down
    EventBus->>EventBus: Buffer drops to 50%
    EventBus->>Actor: Send AdjustInterval message (restore)
    Actor->>Actor: Restore original interval
    
    Note over Main,Lifecycle: Configuration Update
    Main->>Actor: Send UpdateConfig message
    Actor->>Actor: Queue config update
    Note over Actor: Wait for cycle boundary
    Actor->>Actor: Apply config at start of next cycle
    Actor->>Main: Send success response via oneshot
    
    Note over Main,Lifecycle: Graceful Shutdown
    Main->>Actor: Send GracefulShutdown message
    Actor->>Actor: Complete current cycle
    Actor->>EventBus: Flush buffered events + WAL
    Actor->>Main: Send ready response via oneshot
    Main->>Main: Exit
```

## Dependencies

**Requires:**

- ticket:54226c8a-719a-479a-863b-9c91f43717a9/[Ticket 1] - EventBusConnector and WAL must exist

**Blocks:**

- ticket:54226c8a-719a-479a-863b-9c91f43717a9/[Ticket 3] - RPC service needs actor pattern
- ticket:54226c8a-719a-479a-863b-9c91f43717a9/[Ticket 4] - Agent needs "begin monitoring" subscription

## Acceptance Criteria

### Actor Pattern

- [ ] ProcmondMonitorCollector runs in dedicated task with message loop
- [ ] Bounded mpsc channel (capacity: 100) created for actor messages
- [ ] ActorMessage enum defined with all message types
- [ ] Messages processed sequentially (no concurrent state mutations)
- [ ] Oneshot channels used for request/response patterns
- [ ] Channel full errors handled gracefully (log warning, return error)

### Event Bus Integration

- [ ] LocalEventBus completely replaced with EventBusConnector
- [ ] Events published via EventBusConnector to `events.process.*` topics
- [ ] EventBusConnector integrated with actor pattern
- [ ] No compilation errors or warnings

### Startup Coordination

- [ ] procmond subscribes to `control.collector.lifecycle` topic
- [ ] procmond waits for "begin monitoring" broadcast before starting collection
- [ ] BeginMonitoring message triggers collection loop start
- [ ] Startup sequence documented in code comments

### Dynamic Interval Adjustment

- [ ] Actor receives AdjustInterval messages from EventBusConnector
- [ ] Collection interval increases by 50% (1.5x) when backpressure triggered
- [ ] Collection interval restored to original when backpressure released
- [ ] Interval adjustment logged at INFO level

### Configuration Hot-Reload

- [ ] UpdateConfig message queued in actor channel
- [ ] Config applied at start of next collection cycle (atomic)
- [ ] Config validation performed before application
- [ ] Success/failure response sent via oneshot channel
- [ ] Documentation lists which configs are hot-reloadable vs. require restart

### Health Check Enhancement

- [ ] HealthCheck message returns event bus connectivity status
- [ ] Health data includes: collection state, buffer level, connection status
- [ ] Response sent via oneshot channel

### main.rs Updates

- [ ] Bounded mpsc channel created (capacity: 100)
- [ ] EventBusConnector initialized with WAL
- [ ] ProcmondMonitorCollector initialized as actor with channel receiver
- [ ] Graceful shutdown coordination implemented
- [ ] `DAEMONEYE_BROKER_SOCKET` environment variable read

## References

- **Epic Brief:** spec:54226c8a-719a-479a-863b-9c91f43717a9/0fc3298b-37df-4722-a761-66a5a0da16b3
- **Core Flows:** spec:54226c8a-719a-479a-863b-9c91f43717a9/f086f464-1e81-42e8-89f5-74a8638360d1 (Flow 2: System Startup)
- **Tech Plan:** spec:54226c8a-719a-479a-863b-9c91f43717a9/f70103e2-e7ef-494f-8638-5a7324565f28 (Phase 1, Actor Pattern)
- **Monitor Collector:** file:procmond/src/monitor_collector.rs
- **Main Entry Point:** file:procmond/src/main.rs
