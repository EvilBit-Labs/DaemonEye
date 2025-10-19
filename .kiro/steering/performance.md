---
inclusion: always
---

# DaemonEye Performance Guidelines

## Performance Targets (Non-Negotiable)

- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Process Enumeration**: \<5s for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: \<100ms per detection rule execution

## Required Architecture Patterns

### Database: redb Only

- **Primary Storage**: redb embedded database (never SQLite/PostgreSQL)
- **Event Store**: Read/write for daemoneye-agent, read-only for daemoneye-cli
- **Audit Ledger**: Write-only for procmond, separate database file
- **Features**: ACID transactions, zero-copy deserialization, concurrent access

### Resource Management (Mandatory)

- **Bounded Channels**: Use `tokio::sync::mpsc::channel(capacity)` with backpressure
- **Memory Budgets**: Implement cooperative yielding with `tokio::task::yield_now()`
- **Timeouts**: All operations must have bounded execution time
- **Circuit Breakers**: Required for external dependencies (alerts, network)

## Required Data Structures

### Core Types (Use Exactly)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
    pub status: ProcessStatus,
    pub executable_hash: Option<String>, // SHA-256 only
    pub collection_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Stopped,
}
```

### Error Handling (Mandatory Pattern)

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },
    #[error("Database operation failed: {0}")]
    DatabaseError(String),
}
```

## Database Schema (redb TableDefinitions)

### Core Tables (Required)

```rust
use redb::TableDefinition;

// Event Store (events.redb)
const PROCESSES_TABLE: TableDefinition<u64, ProcessInfo> = TableDefinition::new("processes");
const SCANS_TABLE: TableDefinition<u64, ScanMetadata> = TableDefinition::new("scans");
const DETECTION_RULES_TABLE: TableDefinition<String, DetectionRule> =
    TableDefinition::new("detection_rules");
const ALERTS_TABLE: TableDefinition<u64, Alert> = TableDefinition::new("alerts");
const ALERT_DELIVERIES_TABLE: TableDefinition<u64, AlertDelivery> =
    TableDefinition::new("alert_deliveries");

// Audit Ledger (audit.redb - separate file)
const AUDIT_LEDGER_TABLE: TableDefinition<u64, AuditEntry> = TableDefinition::new("audit_ledger");
```

## IPC Communication (Mandatory Patterns)

### Dual-Protocol Architecture (Required)

- **CLI Communication**: IPC via interprocess crate (protobuf + CRC32 framing)
- **Collector Communication**: daemoneye-eventbus message broker (topic-based pub/sub)

### daemoneye-eventbus Integration (Required)

```rust
// Embedded broker within daemoneye-agent
let broker = DaemoneyeBroker::new(broker_config).await?;

// Topic-based pub/sub for collector coordination
let subscription = EventSubscription {
    subscriber_id: "procmond".to_string(),
    topic_patterns: vec!["events.process.*".to_string()],
    enable_wildcards: true,
};
```

### Topic Hierarchy (Exact Patterns Required)

- **Event Topics**: `events.process.*`, `events.network.*`, `events.filesystem.*`
- **Control Topics**: `control.collector.*`, `control.health.*`
- **Message Format**: Bincode serialization with correlation metadata
- **Backpressure**: 10,000+ messages/second throughput, sub-millisecond latency

## Configuration Hierarchy (Exact Order Required)

1. CLI flags (highest precedence)
2. Environment variables (`DAEMONEYE_*` prefix)
3. User config (`~/.config/daemoneye/config.yaml`)
4. System config (`/etc/daemoneye/config.yaml`)
5. Embedded defaults (lowest precedence)

### Required Config Structure

```yaml
app:
  scan_interval_ms: 30000  # Must be configurable
  batch_size: 1000         # Must enforce memory limits

database:
  path: /var/lib/daemoneye/events.redb  # Must use .redb extension
  retention_days: 30       # Must implement cleanup

alerting:
  sinks:
    - type: syslog
      facility: daemon
    - type: webhook
      url: https://alerts.example.com/api
```

## Alert System (Exact Implementation Required)

### AlertSink Trait (Do Not Modify)

```rust
use async_trait::async_trait;

#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, Box<dyn Error + Send + Sync>>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}
```

### Required Features

- Concurrent delivery with bounded parallelism
- Exponential backoff retry (max 3 retries)
- Circuit breaker for failing sinks
- Complete delivery audit trail

## Metrics (Exact Names Required)

```rust
// Do not modify these metric names
// daemoneye_collection_duration_seconds{status="success|error"}
// daemoneye_processes_collected_total
// daemoneye_alerts_generated_total{severity="low|medium|high|critical"}
// daemoneye_alert_deliveries_total{sink="stdout|syslog|webhook"}
```

## Cross-Platform Requirements

### Process Enumeration: sysinfo crate only

- **Linux**: Optimize `/proc` filesystem access (primary target)
- **macOS**: Handle security framework restrictions gracefully
- **Windows**: Windows API access, service deployment support

### Graceful Degradation (Required)

- Continue with reduced functionality when privileges unavailable
- Provide clear diagnostics about missing capabilities
- Implement fallback mechanisms for constrained environments

## Critical Performance Patterns (Always Implement)

### Process Collection

- Handle permission denied gracefully (never crash)
- Compute SHA-256 hashes for executable integrity
- Bounded execution time (\<5s for 10k processes)

### Detection Engine

- Use prepared statements only (never dynamic SQL)
- Validate SQL with AST parsing (sqlparser crate)
- Sandboxed execution with resource limits

### Database Operations

- ACID transactions for consistency
- Automatic schema migrations
- Connection pooling and maintenance operations
- Cooperative yielding under memory pressure

### Resource Management

- All operations must have timeouts
- Implement backpressure on bounded channels
- Memory budgets with `tokio::task::yield_now()`
