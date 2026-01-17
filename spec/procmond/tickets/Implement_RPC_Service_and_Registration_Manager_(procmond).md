# Implement RPC Service and Registration Manager (procmond)

## Overview

Implement RPC service handling and collector registration for procmond. This ticket enables lifecycle management via RPC (health checks, config updates, graceful shutdown) and establishes registration/heartbeat communication with daemoneye-agent.

## Scope

**In Scope:**
- RpcServiceHandler component with actor coordination
- RegistrationManager component for registration and heartbeat
- RPC operation handling: HealthCheck, UpdateConfig, GracefulShutdown
- Subscription to `control.collector.procmond` topic
- Registration via RPC on startup
- "Ready" status reporting after registration
- Periodic heartbeat publishing (every 30 seconds)
- Deregistration on graceful shutdown
- Unit tests for RPC and registration

**Out of Scope:**
- Agent-side loading state management (Ticket 4)
- Agent-side heartbeat detection (Ticket 4)
- Comprehensive integration testing (Ticket 5)
- Security hardening (Ticket 6)

## Technical Details

### RpcServiceHandler Component

**Location:** `file:procmond/src/rpc_service.rs`

**Key Responsibilities:**
- Subscribe to `control.collector.procmond` topic for RPC requests
- Parse incoming RPC requests
- Send ActorMessage to ProcmondMonitorCollector via mpsc channel
- Wait for responses via oneshot channels
- Publish RPC responses with appropriate status codes
- Handle channel full errors gracefully
- Serialize concurrent RPC requests (process one at a time)

**Supported Operations:**
- **HealthCheck**: Query collector health and status
- **UpdateConfig**: Apply configuration changes at cycle boundary
- **GracefulShutdown**: Initiate clean shutdown with event flush

### RegistrationManager Component

**Location:** `file:procmond/src/registration.rs`

**Key Responsibilities:**
- Register with daemoneye-agent on startup via RPC
- Report "ready" status after successful registration
- Publish periodic heartbeats to `control.health.heartbeat.procmond` (every 30 seconds)
- Include health status in heartbeat: Healthy/Degraded/Unhealthy
- Track registration state and heartbeat sequence number
- Deregister on graceful shutdown

**Registration Message Schema:**
```rust
struct RegistrationRequest {
    collector_id: String,  // "procmond"
    collector_type: String,  // "process-monitor"
    version: String,
    capabilities: Vec<String>,
    pid: u32,
}

struct RegistrationResponse {
    status: RegistrationStatus,  // Accepted/Rejected
    message: Option<String>,
}
```

**Heartbeat Message Schema:**
```rust
struct HeartbeatMessage {
    collector_id: String,
    sequence: u64,
    timestamp: DateTime<Utc>,
    health_status: HealthStatus,  // Healthy/Degraded/Unhealthy
    metrics: HeartbeatMetrics,
}

struct HeartbeatMetrics {
    processes_collected: u64,
    events_published: u64,
    buffer_level_percent: f64,
    connection_status: ConnectionStatus,
}
```

```mermaid
sequenceDiagram
    participant Main as main.rs
    participant Reg as RegistrationManager
    participant RPC as RpcServiceHandler
    participant Actor as ProcmondMonitorCollector
    participant EventBus as EventBusConnector
    participant Agent as daemoneye-agent
    
    Note over Main,Agent: Startup Registration
    Main->>Reg: Initialize
    Reg->>EventBus: Publish registration request (RPC)
    EventBus->>Agent: Forward registration
    Agent-->>EventBus: Registration accepted
    EventBus->>Reg: Registration response
    Reg->>EventBus: Publish "ready" status
    Reg->>Reg: Start heartbeat task
    
    Note over Main,Agent: Heartbeat Loop
    loop Every 30 seconds
        Reg->>Actor: Query health metrics
        Actor-->>Reg: Health data
        Reg->>EventBus: Publish heartbeat
    end
    
    Note over Main,Agent: RPC Request Handling
    Agent->>EventBus: Send health check RPC
    EventBus->>RPC: Receive request
    RPC->>RPC: Parse request
    RPC->>Actor: Send HealthCheck message (actor)
    Actor-->>RPC: Health data via oneshot
    RPC->>EventBus: Publish RPC response
    EventBus->>Agent: Forward response
    
    Note over Main,Agent: Configuration Update
    Agent->>EventBus: Send config update RPC
    EventBus->>RPC: Receive request
    RPC->>RPC: Validate config
    RPC->>Actor: Send UpdateConfig message (actor)
    Note over Actor: Config applied at cycle boundary
    Actor-->>RPC: Update result via oneshot
    RPC->>EventBus: Publish RPC response
    
    Note over Main,Agent: Graceful Shutdown
    Agent->>EventBus: Send graceful shutdown RPC
    EventBus->>RPC: Receive request
    RPC->>Actor: Send GracefulShutdown message (actor)
    Actor->>Actor: Complete current cycle
    Actor->>EventBus: Flush buffered events + WAL
    Actor-->>RPC: Shutdown ready via oneshot
    RPC->>EventBus: Publish RPC response
    RPC->>Reg: Deregister
    Reg->>EventBus: Publish deregistration
    RPC->>Main: Signal shutdown
    Main->>Main: Exit
```

## Dependencies

**Requires:**
- ticket:54226c8a-719a-479a-863b-9c91f43717a9/[Ticket 2] - Actor pattern must exist for message coordination

**Blocks:**
- ticket:54226c8a-719a-479a-863b-9c91f43717a9/[Ticket 4] - Agent needs registration/heartbeat handling
- ticket:54226c8a-719a-879a-863b-9c91f43717a9/[Ticket 5] - Integration tests need RPC functionality

## Acceptance Criteria

### RpcServiceHandler
- [ ] Subscribes to `control.collector.procmond` topic on startup
- [ ] Parses incoming RPC requests correctly
- [ ] Sends ActorMessage to ProcmondMonitorCollector via mpsc channel
- [ ] Waits for responses via oneshot channels
- [ ] Publishes RPC responses with correct status codes
- [ ] Handles channel full errors gracefully (logs warning, returns error)
- [ ] Serializes concurrent RPC requests (processes one at a time)
- [ ] Unit tests cover: request parsing, actor coordination, response handling, error cases

### RegistrationManager
- [ ] Registers with daemoneye-agent on startup via RPC
- [ ] Reports "ready" status after successful registration
- [ ] Publishes heartbeats every 30 seconds to `control.health.heartbeat.procmond`
- [ ] Includes health status in heartbeat: Healthy/Degraded/Unhealthy
- [ ] Includes metrics in heartbeat: processes collected, events published, buffer level, connection status
- [ ] Tracks registration state and heartbeat sequence number
- [ ] Deregisters on graceful shutdown
- [ ] Unit tests cover: registration, heartbeat publishing, deregistration, state tracking

### RPC Operations
- [ ] **HealthCheck**: Returns accurate health data including event bus connectivity
- [ ] **UpdateConfig**: Validates config, sends to actor, returns success/failure
- [ ] **GracefulShutdown**: Coordinates with actor, waits for completion, signals main

### Integration with Actor
- [ ] RPC operations correctly coordinate with actor via messages
- [ ] Oneshot channels used for request/response patterns
- [ ] No race conditions or deadlocks
- [ ] Graceful handling of actor channel full errors

### main.rs Updates
- [ ] RpcServiceHandler initialized and started
- [ ] RegistrationManager initialized and started
- [ ] Graceful shutdown coordination includes RPC and registration cleanup

## References

- **Epic Brief:** spec:54226c8a-719a-479a-863b-9c91f43717a9/0fc3298b-37df-4722-a761-66a5a0da16b3
- **Core Flows:** spec:54226c8a-719a-479a-863b-9c91f43717a9/f086f464-1e81-42e8-89f5-74a8638360d1 (Flow 5: Configuration Update, Flow 7: Graceful Shutdown)
- **Tech Plan:** spec:54226c8a-719a-479a-863b-9c91f43717a9/f70103e2-e7ef-494f-8638-5a7324565f28 (Phase 2, RPC Service)
- **RPC Patterns:** file:daemoneye-eventbus/docs/rpc-patterns.md
- **Topic Hierarchy:** file:daemoneye-eventbus/docs/topic-hierarchy.md