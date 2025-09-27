# DaemonEye Core API Reference

This document provides comprehensive API reference for the DaemonEye core library (`daemoneye-lib`) and its public interfaces.

## Table of Contents

- [Core Data Models](#core-data-models)
- [Configuration API](#configuration-api)
- [Storage API](#storage-api)
- [Detection API](#detection-api)
- [Alerting API](#alerting-api)
- [Crypto API](#crypto-api)
- [Telemetry API](#telemetry-api)
- [Error Types](#error-types)

## Core Data Models

### ProcessRecord

Represents a single process snapshot with comprehensive metadata.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRecord {
    /// Unique identifier for this process record
    pub id: Uuid,

    /// Scan identifier this record belongs to
    pub scan_id: i64,

    /// Collection timestamp in milliseconds since Unix epoch
    pub collection_time: i64,

    /// Process ID
    pub pid: u32,

    /// Parent process ID
    pub ppid: Option<u32>,

    /// Process name
    pub name: String,

    /// Path to executable file
    pub executable_path: Option<PathBuf>,

    /// Command line arguments
    pub command_line: Vec<String>,

    /// Process start time in milliseconds since Unix epoch
    pub start_time: Option<i64>,

    /// CPU usage percentage
    pub cpu_usage: Option<f64>,

    /// Memory usage in bytes
    pub memory_usage: Option<u64>,

    /// SHA-256 hash of executable file
    pub executable_hash: Option<String>,

    /// Hash algorithm used (always "sha256")
    pub hash_algorithm: Option<String>,

    /// User ID running the process
    pub user_id: Option<String>,

    /// Whether process data was accessible
    pub accessible: bool,

    /// Whether executable file exists
    pub file_exists: bool,

    /// Platform-specific data
    pub platform_data: Option<serde_json::Value>,
}
```

**Example Usage**:

```rust
use daemoneye_lib::models::ProcessRecord;
use uuid::Uuid;

fn create_process_record() -> ProcessRecord {
    ProcessRecord {
        id: Uuid::new_v4(),
        scan_id: 12345,
        collection_time: 1640995200000, // 2022-01-01 00:00:00 UTC
        pid: 1234,
        ppid: Some(1),
        name: "chrome".to_string(),
        executable_path: Some("/usr/bin/chrome".into()),
        command_line: vec!["chrome".to_string(), "--no-sandbox".to_string()],
        start_time: Some(1640995100000),
        cpu_usage: Some(15.5),
        memory_usage: Some(1073741824), // 1GB
        executable_hash: Some("a1b2c3d4e5f6...".to_string()),
        hash_algorithm: Some("sha256".to_string()),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        platform_data: Some(serde_json::json!({
            "thread_count": 25,
            "priority": "normal"
        })),
    }
}
```

### Alert

Represents a detection result with full context and metadata.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    /// Unique alert identifier
    pub id: Uuid,

    /// Alert timestamp in milliseconds since Unix epoch
    pub alert_time: i64,

    /// Rule identifier that generated this alert
    pub rule_id: String,

    /// Alert title
    pub title: String,

    /// Alert description
    pub description: String,

    /// Alert severity level
    pub severity: AlertSeverity,

    /// Scan identifier (if applicable)
    pub scan_id: Option<i64>,

    /// Affected process IDs
    pub affected_processes: Vec<u32>,

    /// Number of affected processes
    pub process_count: i32,

    /// Additional alert data as JSON
    pub alert_data: serde_json::Value,

    /// Rule execution time in milliseconds
    pub rule_execution_time_ms: Option<i64>,

    /// Deduplication key
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}
```

**Example Usage**:

```rust
use daemoneye_lib::models::{Alert, AlertSeverity};
use uuid::Uuid;

let alert = Alert {
    id: Uuid::new_v4(),
    alert_time: 1640995200000,
    rule_id: "suspicious-processes".to_string(),
    title: "Suspicious Process Detected".to_string(),
    description: "Process with suspicious name detected".to_string(),
    severity: AlertSeverity::High,
    scan_id: Some(12345),
    affected_processes: vec![1234, 5678],
    process_count: 2,
    alert_data: serde_json::json!({
        "processes": [
            {"pid": 1234, "name": "malware.exe"},
            {"pid": 5678, "name": "backdoor.exe"}
        ]
    }),
    rule_execution_time_ms: Some(15),
    dedupe_key: "suspicious-processes:malware.exe".to_string(),
};
```

### DetectionRule

Represents a SQL-based detection rule with metadata and versioning.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectionRule {
    /// Unique rule identifier
    pub id: String,

    /// Rule name
    pub name: String,

    /// Rule description
    pub description: Option<String>,

    /// Rule version
    pub version: i32,

    /// SQL query for detection
    pub sql_query: String,

    /// Whether rule is enabled
    pub enabled: bool,

    /// Alert severity for this rule
    pub severity: AlertSeverity,

    /// Rule category
    pub category: Option<String>,

    /// Rule tags
    pub tags: Vec<String>,

    /// Rule author
    pub author: Option<String>,

    /// Creation timestamp
    pub created_at: i64,

    /// Last update timestamp
    pub updated_at: i64,

    /// Rule source type
    pub source_type: RuleSourceType,

    /// Source file path (if applicable)
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RuleSourceType {
    Builtin,
    File,
    User,
}
```

## Configuration API

### Hierarchical Configuration

The configuration system supports hierarchical loading with environment variable substitution.

```rust
use daemoneye_lib::config::{Config, ConfigBuilder, ConfigError};

// Create configuration builder
let mut builder = ConfigBuilder::new();

// Load configuration from multiple sources
builder
    .add_embedded_defaults()?
    .add_file("/etc/daemoneye/config.yaml")?
    .add_file("~/.config/daemoneye/config.yaml")?
    .add_environment("DaemonEye_")?
    .add_cli_args(args)?;

// Build final configuration
let config: Config = builder.build()?;

// Access configuration values
let scan_interval = config.get::<u64>("app.scan_interval_ms")?;
let log_level = config.get::<String>("app.log_level")?;
```

### Configuration Validation

```rust
use daemoneye_lib::config::{ConfigValidator, ValidationResult};

let validator = ConfigValidator::new();
let result: ValidationResult = validator.validate(&config)?;

if !result.is_valid() {
    for error in result.errors() {
        eprintln!("Configuration error: {}", error);
    }
}
```

### Environment Variable Substitution

```rust
use daemoneye_lib::config::EnvironmentSubstitutor;

let substitutor = EnvironmentSubstitutor::new();
let config_with_env = substitutor.substitute(config)?;
```

## Storage API

### Event Store (redb)

High-performance embedded database for process data storage.

```rust
use daemoneye_lib::storage::{EventStore, EventStoreConfig, ProcessQuery};

// Create event store
let config = EventStoreConfig {
    path: "/var/lib/daemoneye/events.redb".into(),
    max_size_mb: 10240,
    wal_mode: true,
    max_connections: 10,
};

let event_store = EventStore::new(config)?;

// Store process records
let processes = vec![process_record1, process_record2];
event_store.store_processes(&processes).await?;

// Query processes
let query = ProcessQuery::new()
    .with_pid(1234)
    .with_name("chrome")
    .with_time_range(start_time, end_time);

let results = event_store.query_processes(&query).await?;

// Export data
let export_config = ExportConfig {
    format: ExportFormat::Json,
    output_path: "/tmp/export.json".into(),
    time_range: Some((start_time, end_time)),
};

event_store.export_data(&export_config).await?;
```

### Audit Ledger (SQLite)

Tamper-evident audit trail with cryptographic integrity.

```rust
use daemoneye_lib::storage::{AuditLedger, AuditEntry, AuditRecord};

// Create audit ledger
let audit_ledger = AuditLedger::new("/var/lib/daemoneye/audit.sqlite")?;

// Log audit entry
let entry = AuditEntry {
    actor: "procmond".to_string(),
    action: "process_collection".to_string(),
    payload: serde_json::json!({
        "process_count": 150,
        "scan_id": 12345
    }),
};

let record = audit_ledger.append_entry(&entry).await?;

// Verify audit chain
let verification_result = audit_ledger.verify_chain().await?;
if !verification_result.is_valid() {
    eprintln!("Audit chain verification failed: {:?}", verification_result.errors());
}
```

## Detection API

### SQL Validator

Comprehensive SQL validation to prevent injection attacks.

```rust
use daemoneye_lib::detection::{SqlValidator, ValidationResult, ValidationError};

// Create SQL validator
let validator = SqlValidator::new()
    .with_allowed_functions(vec![
        "count", "sum", "avg", "min", "max",
        "length", "substr", "upper", "lower",
        "datetime", "strftime"
    ])
    .with_read_only_mode(true)
    .with_timeout(Duration::from_secs(30));

// Validate SQL query
let sql = "SELECT pid, name, cpu_usage FROM processes WHERE cpu_usage > 50.0";
let result: ValidationResult = validator.validate_query(sql)?;

match result {
    ValidationResult::Valid => println!("Query is valid"),
    ValidationResult::Invalid(errors) => {
        for error in errors {
            eprintln!("Validation error: {}", error);
        }
    }
}
```

### Detection Engine

SQL-based detection rule execution with security validation.

```rust
use daemoneye_lib::detection::{DetectionEngine, DetectionResult, RuleExecutionConfig};

// Create detection engine
let config = DetectionEngineConfig {
    event_store: event_store.clone(),
    rule_timeout: Duration::from_secs(30),
    max_concurrent_rules: 10,
    enable_metrics: true,
};

let detection_engine = DetectionEngine::new(config)?;

// Execute detection rules
let execution_config = RuleExecutionConfig {
    scan_id: Some(12345),
    rule_ids: None, // Execute all enabled rules
    timeout: Duration::from_secs(60),
};

let results: Vec<DetectionResult> = detection_engine
    .execute_rules(&execution_config)
    .await?;

// Process detection results
for result in results {
    if !result.alerts.is_empty() {
        println!("Rule {} generated {} alerts", result.rule_id, result.alerts.len());
    }
}
```

### Rule Manager

Detection rule management with hot-reloading support.

```rust
use daemoneye_lib::detection::{RuleManager, RuleManagerConfig};

// Create rule manager
let config = RuleManagerConfig {
    rules_path: "/etc/daemoneye/rules".into(),
    enable_hot_reload: true,
    validation_enabled: true,
};

let rule_manager = RuleManager::new(config)?;

// Load rules
let rules = rule_manager.load_rules().await?;

// Enable/disable rules
rule_manager.enable_rule("suspicious-processes").await?;
rule_manager.disable_rule("test-rule").await?;

// Validate rule
let validation_result = rule_manager.validate_rule_file("/path/to/rule.sql").await?;

// Test rule
let test_result = rule_manager.test_rule("suspicious-processes", test_data).await?;
```

## Alerting API

### Alert Manager

Alert generation, deduplication, and delivery management.

```rust
use daemoneye_lib::alerting::{AlertManager, AlertManagerConfig, DeduplicationConfig};

// Create alert manager
let config = AlertManagerConfig {
    deduplication: DeduplicationConfig {
        enabled: true,
        window_minutes: 60,
        key_fields: vec!["rule_id".to_string(), "process_name".to_string()],
    },
    max_queue_size: 10000,
    delivery_timeout: Duration::from_secs(30),
};

let alert_manager = AlertManager::new(config)?;

// Generate alert
let alert = alert_manager.generate_alert(
    &detection_result,
    &process_data
).await?;

// Deliver alert
if let Some(alert) = alert {
    let delivery_result = alert_manager.deliver_alert(&alert).await?;
    println!("Alert delivered: {:?}", delivery_result);
}
```

### Alert Sinks

Pluggable alert delivery channels.

```rust
use daemoneye_lib::alerting::sinks::{AlertSink, StdoutSink, SyslogSink, WebhookSink};

// Create alert sinks
let stdout_sink = StdoutSink::new(OutputFormat::Json);
let syslog_sink = SyslogSink::new(SyslogConfig {
    facility: SyslogFacility::Daemon,
    tag: "daemoneye".to_string(),
    host: "localhost".to_string(),
    port: 514,
});

let webhook_sink = WebhookSink::new(WebhookConfig {
    url: "https://your-siem.com/webhook".parse()?,
    headers: vec![
        ("Authorization".to_string(), "Bearer token".to_string()),
    ],
    timeout: Duration::from_secs(30),
    retry_attempts: 3,
});

// Send alert to all sinks
let sinks: Vec<Box<dyn AlertSink>> = vec![
    Box::new(stdout_sink),
    Box::new(syslog_sink),
    Box::new(webhook_sink),
];

for sink in sinks {
    let result = sink.send(&alert).await?;
    println!("Sink {} result: {:?}", sink.name(), result);
}
```

## Crypto API

### Hash Chain

Cryptographic hash chain for audit trail integrity.

```rust
use daemoneye_lib::crypto::{HashChain, HashChainConfig, ChainVerificationResult};

// Create hash chain
let config = HashChainConfig {
    algorithm: HashAlgorithm::Blake3,
    enable_signatures: true,
    private_key_path: Some("/etc/daemoneye/private.key".into()),
};

let mut hash_chain = HashChain::new(config)?;

// Append entry to chain
let entry = AuditEntry {
    actor: "procmond".to_string(),
    action: "process_collection".to_string(),
    payload: serde_json::json!({"process_count": 150}),
};

let record = hash_chain.append_entry(&entry).await?;
println!("Chain entry: {}", record.entry_hash);

// Verify chain integrity
let verification_result: ChainVerificationResult = hash_chain.verify_chain().await?;
if verification_result.is_valid() {
    println!("Chain verification successful");
} else {
    eprintln!("Chain verification failed: {:?}", verification_result.errors());
}
```

### Digital Signatures

Ed25519 digital signatures for enhanced integrity.

```rust
use daemoneye_lib::crypto::{SignatureManager, SignatureConfig};

// Create signature manager
let config = SignatureConfig {
    private_key_path: "/etc/daemoneye/private.key".into(),
    public_key_path: "/etc/daemoneye/public.key".into(),
};

let signature_manager = SignatureManager::new(config)?;

// Sign data
let data = b"important audit data";
let signature = signature_manager.sign(data)?;

// Verify signature
let is_valid = signature_manager.verify(data, &signature)?;
println!("Signature valid: {}", is_valid);
```

## Telemetry API

### Metrics Collection

Prometheus-compatible metrics collection.

```rust
use daemoneye_lib::telemetry::{MetricsCollector, MetricType, MetricValue};

// Create metrics collector
let metrics_collector = MetricsCollector::new()?;

// Record metrics
metrics_collector.record_counter("processes_collected_total", 150)?;
metrics_collector.record_gauge("active_processes", 45.0)?;
metrics_collector.record_histogram("collection_duration_seconds", 2.5)?;

// Export metrics
let metrics_data = metrics_collector.export_metrics()?;
println!("Metrics: {}", metrics_data);
```

### Health Monitoring

System health monitoring and status reporting.

```rust
use daemoneye_lib::telemetry::{HealthMonitor, HealthStatus, ComponentHealth};

// Create health monitor
let health_monitor = HealthMonitor::new()?;

// Check system health
let health_status: HealthStatus = health_monitor.check_system_health().await?;

println!("System Health: {:?}", health_status.status);
for (component, health) in health_status.components {
    println!("{}: {:?}", component, health);
}

// Check specific component
let db_health: ComponentHealth = health_monitor.check_component("database").await?;
println!("Database Health: {:?}", db_health);
```

## Error Types

### Core Error Types

```rust
use daemoneye_lib::errors::{DaemonEyeError, DaemonEyeErrorKind};

// Error handling example
match some_operation().await {
    Ok(result) => println!("Success: {:?}", result),
    Err(DaemonEyeError::Configuration(e)) => {
        eprintln!("Configuration error: {}", e);
    }
    Err(DaemonEyeError::Database(e)) => {
        eprintln!("Database error: {}", e);
    }
    Err(DaemonEyeError::Detection(e)) => {
        eprintln!("Detection error: {}", e);
    }
    Err(e) => {
        eprintln!("Unexpected error: {}", e);
    }
}
```

### Error Categories

- **ConfigurationError**: Configuration loading and validation errors
- **DatabaseError**: Database operation errors
- **DetectionError**: Detection rule execution errors
- **AlertingError**: Alert generation and delivery errors
- **CryptoError**: Cryptographic operation errors
- **ValidationError**: Data validation errors
- **IoError**: I/O operation errors

---

*This API reference provides comprehensive documentation for the DaemonEye core library. For additional examples and advanced usage, consult the source code and integration tests.*
