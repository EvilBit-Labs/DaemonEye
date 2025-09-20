# SentinelD Performance Guidelines

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

- **Event Store**: Read/write access for sentinelagent, read-only for sentinelcli
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
use sentinel_lib::proto::{DetectionTask, DetectionResult};

// Unix Domain Sockets (Linux/macOS) or Named Pipes (Windows)
let stream = LocalSocketStream::connect("/tmp/sentineld.sock")?;

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

## Configuration Management

### Hierarchical Configuration

Support multiple configuration sources with precedence:

1. Command-line flags (highest precedence)
2. Environment variables (`PROCMOND_*`)
3. User configuration file (`~/.config/procmond/config.yaml`)
4. System configuration file (`/etc/procmond/config.yaml`)
5. Embedded defaults (lowest precedence)

### Configuration Structure

```yaml
app:
  scan_interval_ms: 30000
  batch_size: 1000

database:
  path: /var/lib/procmond/processes.sqlite
  retention_days: 30

alerting:
  sinks:
    - type: syslog
      facility: daemon
    - type: webhook
      url: https://alerts.example.com/api
```

## Alert System Performance

### Plugin-Based Alert Sinks

Use trait-based plugin system:

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

### Alert Delivery Performance

- Concurrent delivery to multiple sinks
- Retry logic with exponential backoff
- Circuit breaking for failing sinks
- Delivery audit trail

## Observability and Monitoring

### Metrics Export

Support Prometheus metrics for operational monitoring:

```rust
// Example Prometheus metrics
// procmond_collection_duration_seconds{status="success|error"}
// procmond_processes_collected_total
// procmond_alerts_generated_total{severity="low|medium|high|critical"}
// procmond_alert_deliveries_total{sink="stdout|syslog|webhook"}

struct PrometheusMetrics {
    collection_duration: String,
    processes_collected: String,
    alerts_generated: String,
    alert_deliveries: String,
}
```

### Health Checks

- Configuration validation endpoints
- Database connectivity checks
- Alert sink health monitoring
- System capability assessment

## Platform Considerations

### Cross-Platform Compatibility

- Linux: Primary development target with `/proc` filesystem access
- macOS: Native process enumeration with security framework integration
- Windows: Windows API process access with service deployment

### Graceful Degradation

- Continue with reduced functionality when elevated privileges unavailable
- Provide clear diagnostics about system capabilities
- Fallback mechanisms for constrained environments

## Process Collection Performance

- Use `sysinfo` crate as primary cross-platform abstraction
- Implement platform-specific optimizations where beneficial
- Handle permission denied gracefully with partial data collection
- Compute SHA-256 hashes of executable files for integrity checking

## Detection Engine Performance

- Execute SQL rules against redb database with prepared statements
- Implement rule validation and testing capabilities
- Support hot-reloading of rule files with change detection
- Provide performance metrics and optimization hints

## Database Operations Performance

- Use connection pooling for concurrent access
- Implement automatic schema migrations
- Support database maintenance operations (VACUUM, ANALYZE)
- Provide detailed statistics and performance metrics
