# DaemonEye Performance Guidelines

## Performance Targets

- **CPU Usage**: < 5% sustained during continuous monitoring
- **Memory Usage**: < 100 MB resident under normal operation
- **Process Enumeration**: < 5s for 10,000+ processes
- **Database Operations**: > 1,000 records/second write rate
- **Alert Latency**: < 100ms per detection rule execution
- **Query Response**: Sub-second response times for 100,000+ events/minute (Enterprise)

## Resource Management

- **Bounded Channels**: Configurable capacity with backpressure policies
- **Memory Limits**: Cooperative yielding and memory budget enforcement
- **Timeout Support**: Cancellation tokens for graceful shutdown
- **Circuit Breakers**: Reliability patterns for external dependencies
- **Graceful Degradation**: Continue with reduced functionality when resources constrained

## Database Technology: redb

**Primary Storage**: redb pure Rust embedded database for optimal performance and security

- **Event Store**: Read/write access for daemoneye-agent, read-only for daemoneye-cli
- **Audit Ledger**: Write-only access for procmond, read-only for others
- **Features**: Concurrent access, ACID transactions, zero-copy deserialization
- **Performance**: Optimized for time-series queries and high-throughput writes

## Core Data Types

Use strongly-typed structures with serde for serialization:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleType {
    ProcessMonitor,
    NetworkMonitor,
    FileSystemMonitor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskParameters {
    pub filter: String,
    pub threshold: u64,
}

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
    pub executable_hash: Option<String>, // SHA-256
    pub collection_time: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DetectionTask {
    pub task_id: String,
    pub rule_type: RuleType,
    pub parameters: TaskParameters,
    pub timeout: Duration,
}
```

## Error Handling Patterns

Use thiserror for structured error types:

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

    #[error("IPC communication failed: {0}")]
    IpcError(String),
}
```

## Database Schema Design

### Core Tables (redb)

```rust
use redb::TableDefinition;

// Event Store schema
const PROCESSES_TABLE: TableDefinition<u64, ProcessInfo> = TableDefinition::new("processes");
const SCANS_TABLE: TableDefinition<u64, ScanMetadata> = TableDefinition::new("scans");
const DETECTION_RULES_TABLE: TableDefinition<String, DetectionRule> =
    TableDefinition::new("detection_rules");
const ALERTS_TABLE: TableDefinition<u64, Alert> = TableDefinition::new("alerts");
const ALERT_DELIVERIES_TABLE: TableDefinition<u64, AlertDelivery> =
    TableDefinition::new("alert_deliveries");

// Audit Ledger schema (separate database)
const AUDIT_LEDGER_TABLE: TableDefinition<u64, AuditEntry> = TableDefinition::new("audit_ledger");
```

### Business/Enterprise Extensions

```rust
// Additional tables for Business/Enterprise tiers
const AGENTS_TABLE: TableDefinition<String, AgentInfo> = TableDefinition::new("agents");
const AGENT_CONNECTIONS_TABLE: TableDefinition<String, ConnectionInfo> =
    TableDefinition::new("agent_connections");
const FLEET_EVENTS_TABLE: TableDefinition<u64, FleetEvent> = TableDefinition::new("fleet_events");
const RULE_PACKS_TABLE: TableDefinition<String, RulePack> = TableDefinition::new("rule_packs");
const NETWORK_EVENTS_TABLE: TableDefinition<u64, NetworkEvent> =
    TableDefinition::new("network_events"); // Enterprise
const KERNEL_EVENTS_TABLE: TableDefinition<u64, KernelEvent> =
    TableDefinition::new("kernel_events"); // Enterprise
```

## IPC Communication Performance

### Transport Layer

Use `interprocess` crate for cross-platform IPC communication:

```rust
use interprocess::local_socket::LocalSocketStream;
use daemoneye_lib::proto::{DetectionTask, DetectionResult};

// Unix Domain Sockets (Linux/macOS) or Named Pipes (Windows)
let stream = LocalSocketStream::connect("/tmp/DaemonEye.sock")?;

// Protobuf message serialization with CRC32 checksums
let task = DetectionTask::new()
    .with_rule_id("suspicious_process")
    .with_query("SELECT * FROM processes WHERE name = 'malware.exe'")
    .build();

// Serialize and send with framing
let serialized = prost::Message::encode_to_vec(&task)?;
// Send with CRC32 and length prefixing for integrity
```

### Message Framing

- All IPC messages use length-delimited protobuf with CRC32 checksums for corruption detection
- Sequence numbers for ordering
- **Backpressure**: Credit-based flow control with configurable limits (default: 1000 pending records, max 10 concurrent tasks)

## Configuration Management Rules

MUST implement hierarchical configuration in this exact order:

1. Command-line flags (highest precedence)
2. Environment variables (`DAEMONEYE_*` prefix)
3. User config (`~/.config/daemoneye/config.yaml`)
4. System config (`/etc/daemoneye/config.yaml`)
5. Embedded defaults (lowest precedence)

REQUIRED configuration structure:

```yaml
app:
  scan_interval_ms: 30000  # MUST be configurable
  batch_size: 1000         # MUST enforce memory limits

database:
  path: /var/lib/daemoneye/events.redb  # MUST use .redb extension
  retention_days: 30       # MUST implement cleanup

alerting:
  sinks:
    - type: syslog
      facility: daemon
    - type: webhook
      url: https://alerts.example.com/api
```

## Alert System Requirements

MUST use this exact trait-based plugin pattern:

```rust
use async_trait::async_trait;
use std::error::Error;

#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, Box<dyn Error + Send + Sync>>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}
```

REQUIRED alert delivery features:

- Concurrent delivery to multiple sinks with bounded parallelism
- Exponential backoff retry logic (max 3 retries)
- Circuit breaker pattern for failing sinks
- Complete delivery audit trail in database

## Performance Monitoring Requirements

MUST implement these exact Prometheus metrics:

```rust
// REQUIRED metrics - do not modify names
// daemoneye_collection_duration_seconds{status="success|error"}
// daemoneye_processes_collected_total
// daemoneye_alerts_generated_total{severity="low|medium|high|critical"}
// daemoneye_alert_deliveries_total{sink="stdout|syslog|webhook"}
```

REQUIRED health checks:

- Configuration validation with detailed error messages
- Database connectivity with latency measurement
- Alert sink health with circuit breaker status
- System capability assessment with privilege validation

## Cross-Platform Performance Rules

MUST use `sysinfo` crate as primary abstraction with these requirements:

- **Linux**: Primary target, optimize for `/proc` filesystem access
- **macOS**: Native process enumeration, handle security framework restrictions
- **Windows**: Windows API access, support service deployment

REQUIRED graceful degradation:

- Continue with reduced functionality when privileges unavailable
- Provide clear diagnostics about missing capabilities
- Implement fallback mechanisms for constrained environments

## Critical Performance Patterns

ALWAYS implement these patterns:

- **Process Collection**: Handle permission denied gracefully, compute SHA-256 hashes
- **Detection Engine**: Use prepared statements only, validate SQL with AST parsing
- **Database Operations**: Connection pooling, automatic migrations, maintenance operations
- **Resource Limits**: Enforce memory budgets, implement cooperative yielding
- **Timeout Enforcement**: All operations MUST have bounded execution time
