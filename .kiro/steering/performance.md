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

### Core Types (Use Exactly: `ProcessRecord` from `daemoneye-lib/src/models/process.rs`)

```rust,ignore
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Strongly-typed process identifier (newtype over u32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessId(u32);

/// Comprehensive process record with all metadata fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRecord {
    /// Process identifier
    pub pid: ProcessId,
    /// Parent process identifier
    pub ppid: Option<ProcessId>,
    /// Process name
    pub name: String,
    /// Path to executable file
    pub executable_path: Option<PathBuf>,
    /// Command line arguments
    pub command_line: Option<String>,
    /// Process start time
    pub start_time: Option<SystemTime>,
    /// CPU usage percentage
    pub cpu_usage: Option<f64>,
    /// Memory usage in bytes
    pub memory_usage: Option<u64>,
    /// Process status
    pub status: ProcessStatus,
    /// Executable file hash
    pub executable_hash: Option<String>,
    /// Hash algorithm used
    pub hash_algorithm: Option<String>,
    /// Collection timestamp
    pub collection_time: DateTime<Utc>,
    /// User ID
    pub user_id: Option<u32>,
    /// Group ID
    pub group_id: Option<u32>,
    /// Environment variables
    pub environment_vars: HashMap<String, String>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Process status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcessStatus {
    /// Process is running
    Running,
    /// Process is sleeping/waiting
    Sleeping,
    /// Process is stopped
    Stopped,
    /// Process is a zombie
    Zombie,
    /// Process is being traced
    Traced,
    /// Process is in an unknown state
    Unknown(String),
}
```

### Error Handling (Mandatory Pattern)

```rust,ignore
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

```rust,ignore
use redb::TableDefinition;

// Current tables (daemoneye-lib/src/storage.rs) — values are serialized bytes
pub const PROCESSES: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("processes");
pub const DETECTION_RULES: TableDefinition<'static, &str, Vec<u8>> =
    TableDefinition::new("detection_rules");
pub const ALERTS: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("alerts");
pub const SYSTEM_INFO: TableDefinition<'static, u64, Vec<u8>> =
    TableDefinition::new("system_info");
pub const SCAN_METADATA: TableDefinition<'static, u64, Vec<u8>> =
    TableDefinition::new("scan_metadata");

// [Planned] — not yet present in storage.rs:
// - alert_deliveries (delivery tracking)
// - audit_ledger (audit.redb - separate file)
```

## IPC Communication (Mandatory Patterns)

### Dual-Protocol Architecture (Required)

- **CLI Communication**: IPC via interprocess crate (protobuf + CRC32 framing)
- **Collector Communication**: daemoneye-eventbus message broker (topic-based pub/sub)

### daemoneye-eventbus Integration (Required)

```rust,ignore
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
- **Message Format**: postcard serialization with correlation metadata
- **Backpressure**: 10,000+ messages/second throughput, sub-millisecond latency

## Configuration Hierarchy (Exact Order Required)

1. CLI flags (highest precedence)
2. Environment variables, component-namespaced (`PROCMOND_*`, `DAEMONEYE_AGENT_*`, `DAEMONEYE_CLI_*`)
3. User config (`~/.config/daemoneye/config.toml`)
4. System config (`/etc/daemoneye/config.toml`)
5. Embedded defaults (lowest precedence)

### Required Config Structure

```toml
[app]
scan_interval_ms = 30000 # Must be configurable
batch_size = 1000        # Must enforce memory limits

[database]
path = "/var/lib/daemoneye/events.redb" # Must use .redb extension
retention_days = 30                     # Must implement cleanup

[[alerting.sinks]]
type = "syslog"
facility = "daemon"

[[alerting.sinks]]
type = "webhook"
url = "https://alerts.example.com/api"
```

## Alert System (Exact Implementation Required)

### AlertSink Trait (Do Not Modify)

```rust,ignore
use async_trait::async_trait;

#[async_trait]
pub trait AlertSink: Send + Sync {
    /// Send an alert through this sink.
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError>;

    /// Get the sink name.
    fn name(&self) -> &str;

    /// Check if the sink is healthy.
    async fn health_check(&self) -> Result<(), AlertingError>;
}
```

### Required Features

- Concurrent delivery with bounded parallelism
- Exponential backoff retry (max 3 retries)
- Circuit breaker for failing sinks
- Complete delivery audit trail

## Metrics (Exact Names Required)

```rust,ignore
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
