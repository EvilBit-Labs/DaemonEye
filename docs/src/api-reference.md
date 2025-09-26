# API Reference

This section contains comprehensive API documentation for DaemonEye, covering all public interfaces, data structures, and usage examples.

## Core API

The core API provides the fundamental interfaces for process monitoring, alerting, and data management.

[Read Core API Documentation →](./api-reference/core-api.md)

## API Overview

### Component APIs

DaemonEye provides APIs for each component:

- **ProcMonD API**: Process collection and system monitoring
- **daemoneye-agent API**: Alerting and orchestration
- **daemoneye-cli API**: Command-line interface and management
- **daemoneye-lib API**: Shared library interfaces

### API Design Principles

1. **Async-First**: All APIs use async/await patterns
2. **Error Handling**: Structured error types with `thiserror`
3. **Type Safety**: Strong typing with Rust's type system
4. **Documentation**: Comprehensive rustdoc comments
5. **Examples**: Code examples for all public APIs

## Quick Reference

### Process Collection

```rust
use daemoneye_lib::collector::ProcessCollector;

let collector = ProcessCollector::new();
let processes = collector.collect_processes().await?;
```

### Database Operations

```rust
use daemoneye_lib::storage::Database;

let db = Database::new("processes.db").await?;
let processes = db.query_processes("SELECT * FROM processes WHERE pid = ?", &[1234]).await?;
```

### Alert Management

```rust
use daemoneye_lib::alerting::AlertManager;

let mut alert_manager = AlertManager::new();
alert_manager.add_sink(Box::new(SyslogSink::new("daemon")?));
alert_manager.send_alert(alert).await?;
```

### Configuration Management

```rust
use daemoneye_lib::config::Config;

let config = Config::load_from_file("config.yaml").await?;
let scan_interval = config.get::<u64>("app.scan_interval_ms")?;
```

## Data Structures

### ProcessInfo

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
    pub status: ProcessStatus,
    pub executable_hash: Option<String>,
    pub collection_time: DateTime<Utc>,
}
```

### Alert

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Alert {
    pub id: Uuid,
    pub rule_name: String,
    pub severity: AlertSeverity,
    pub message: String,
    pub process: ProcessInfo,
    pub timestamp: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}
```

### DetectionRule

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectionRule {
    pub name: String,
    pub description: String,
    pub sql_query: String,
    pub priority: u32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

## Error Types

### CollectionError

```rust
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },

    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("System error: {0}")]
    SystemError(String),
}
```

### DatabaseError

```rust
#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("Database connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Query execution failed: {0}")]
    QueryFailed(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    #[error("SQLite error: {0}")]
    SqliteError(#[from] rusqlite::Error),
}
```

### AlertError

```rust
#[derive(Debug, Error)]
pub enum AlertError {
    #[error("Alert delivery failed: {0}")]
    DeliveryFailed(String),

    #[error("Invalid alert format: {0}")]
    InvalidFormat(String),

    #[error("Alert sink error: {0}")]
    SinkError(String),

    #[error("Alert queue full")]
    QueueFull,
}
```

## Service Traits

### ProcessCollectionService

```rust
#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    async fn collect_processes(&self) -> Result<CollectionResult, CollectionError>;
    async fn get_system_info(&self) -> Result<SystemInfo, CollectionError>;
    async fn get_process_by_pid(&self, pid: u32) -> Result<Option<ProcessInfo>, CollectionError>;
}
```

### DetectionService

```rust
#[async_trait]
pub trait DetectionService: Send + Sync {
    async fn execute_rules(&self, scan_context: &ScanContext)
    -> Result<Vec<Alert>, DetectionError>;
    async fn load_rules(&self) -> Result<Vec<DetectionRule>, DetectionError>;
    async fn add_rule(&self, rule: DetectionRule) -> Result<(), DetectionError>;
    async fn remove_rule(&self, rule_name: &str) -> Result<(), DetectionError>;
}
```

### AlertSink

```rust
#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, DeliveryError>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}
```

## Configuration API

### Config

```rust
pub struct Config {
    app: AppConfig,
    database: DatabaseConfig,
    alerting: AlertingConfig,
    security: SecurityConfig,
}

impl Config {
    pub async fn load_from_file(path: &str) -> Result<Self, ConfigError>;
    pub async fn load_from_env() -> Result<Self, ConfigError>;
    pub fn get<T>(&self, key: &str) -> Result<T, ConfigError>
    where
        T: DeserializeOwned;
    pub fn set<T>(&mut self, key: &str, value: T) -> Result<(), ConfigError>
    where
        T: Serialize;
    pub fn validate(&self) -> Result<(), ConfigError>;
}
```

### AppConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub scan_interval_ms: u64,
    pub batch_size: usize,
    pub log_level: String,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub pid_file: Option<PathBuf>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub max_memory_mb: Option<u64>,
    pub max_cpu_percent: Option<f64>,
}
```

## Database API

### Database

```rust
pub struct Database {
    conn: Connection,
}

impl Database {
    pub async fn new(path: &str) -> Result<Self, DatabaseError>;
    pub async fn create_schema(&self) -> Result<(), DatabaseError>;
    pub async fn insert_process(&self, process: &ProcessInfo) -> Result<(), DatabaseError>;
    pub async fn get_process(&self, pid: u32) -> Result<Option<ProcessInfo>, DatabaseError>;
    pub async fn query_processes(
        &self,
        query: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<ProcessInfo>, DatabaseError>;
    pub async fn insert_alert(&self, alert: &Alert) -> Result<(), DatabaseError>;
    pub async fn get_alerts(&self, limit: Option<usize>) -> Result<Vec<Alert>, DatabaseError>;
    pub async fn cleanup_old_data(&self, retention_days: u32) -> Result<(), DatabaseError>;
}
```

## Alerting API

### AlertManager

```rust
pub struct AlertManager {
    sinks: Vec<Box<dyn AlertSink>>,
    queue: Arc<Mutex<VecDeque<Alert>>>,
    max_queue_size: usize,
}

impl AlertManager {
    pub fn new() -> Self;
    pub fn add_sink(&mut self, sink: Box<dyn AlertSink>);
    pub async fn send_alert(&self, alert: Alert) -> Result<(), AlertError>;
    pub async fn health_check(&self) -> HealthStatus;
    pub fn queue_size(&self) -> usize;
    pub fn queue_capacity(&self) -> usize;
}
```

### Alert Sinks

#### SyslogSink

```rust
pub struct SyslogSink {
    facility: String,
    priority: String,
    tag: String,
}

impl SyslogSink {
    pub fn new(facility: &str) -> Result<Self, SinkError>;
    pub fn with_priority(self, priority: &str) -> Self;
    pub fn with_tag(self, tag: &str) -> Self;
}
```

#### WebhookSink

```rust
pub struct WebhookSink {
    url: String,
    method: String,
    timeout: Duration,
    retry_attempts: u32,
    headers: HashMap<String, String>,
}

impl WebhookSink {
    pub fn new(url: &str) -> Self;
    pub fn with_method(self, method: &str) -> Self;
    pub fn with_timeout(self, timeout: Duration) -> Self;
    pub fn with_retry_attempts(self, attempts: u32) -> Self;
    pub fn with_header(self, key: &str, value: &str) -> Self;
}
```

#### FileSink

```rust
pub struct FileSink {
    path: PathBuf,
    format: OutputFormat,
    rotation: RotationPolicy,
    max_files: usize,
}

impl FileSink {
    pub fn new(path: &str) -> Self;
    pub fn with_format(self, format: OutputFormat) -> Self;
    pub fn with_rotation(self, rotation: RotationPolicy) -> Self;
    pub fn with_max_files(self, max_files: usize) -> Self;
}
```

## CLI API

### Cli

```rust
pub struct Cli {
    pub command: Commands,
    pub config: Option<PathBuf>,
    pub log_level: String,
}

#[derive(Subcommand)]
pub enum Commands {
    Run(RunCommand),
    Config(ConfigCommand),
    Rules(RulesCommand),
    Alerts(AlertsCommand),
    Health(HealthCommand),
    Query(QueryCommand),
    Logs(LogsCommand),
}

impl Cli {
    pub async fn execute(self) -> Result<(), CliError>;
}
```

### Commands

#### RunCommand

```rust
#[derive(Args)]
pub struct RunCommand {
    #[arg(short, long)]
    pub daemon: bool,

    #[arg(short, long)]
    pub foreground: bool,
}
```

#### ConfigCommand

```rust
#[derive(Subcommand)]
pub enum ConfigCommand {
    Show(ConfigShowCommand),
    Set(ConfigSetCommand),
    Get(ConfigGetCommand),
    Validate(ConfigValidateCommand),
    Load(ConfigLoadCommand),
}

#[derive(Args)]
pub struct ConfigShowCommand {
    #[arg(long)]
    pub include_defaults: bool,

    #[arg(long)]
    pub format: Option<String>,
}
```

#### RulesCommand

```rust
#[derive(Subcommand)]
pub enum RulesCommand {
    List(RulesListCommand),
    Add(RulesAddCommand),
    Remove(RulesRemoveCommand),
    Enable(RulesEnableCommand),
    Disable(RulesDisableCommand),
    Validate(RulesValidateCommand),
    Test(RulesTestCommand),
    Reload(RulesReloadCommand),
}
```

## IPC API

### IpcServer

```rust
pub struct IpcServer {
    socket_path: PathBuf,
    handlers: HashMap<String, Box<dyn IpcHandler>>,
}

impl IpcServer {
    pub fn new(socket_path: &str) -> Self;
    pub fn add_handler(&mut self, name: &str, handler: Box<dyn IpcHandler>);
    pub async fn run(self) -> Result<(), IpcError>;
}
```

### IpcClient

```rust
pub struct IpcClient {
    socket_path: PathBuf,
    connection: Option<Connection>,
}

impl IpcClient {
    pub async fn new(socket_path: &str) -> Result<Self, IpcError>;
    pub async fn send_request(&self, request: IpcRequest) -> Result<IpcResponse, IpcError>;
    pub async fn close(self) -> Result<(), IpcError>;
}
```

### IPC Messages

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    CollectProcesses,
    GetProcess { pid: u32 },
    QueryProcesses { query: String, params: Vec<Value> },
    GetAlerts { limit: Option<usize> },
    SendAlert { alert: Alert },
    HealthCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    Processes(Vec<ProcessInfo>),
    Process(Option<ProcessInfo>),
    QueryResult(Vec<ProcessInfo>),
    Alerts(Vec<Alert>),
    AlertSent,
    Health(HealthStatus),
    Error(String),
}
```

## Utility APIs

### Logger

```rust
pub struct Logger {
    level: Level,
    format: LogFormat,
    output: LogOutput,
}

impl Logger {
    pub fn new() -> Self;
    pub fn with_level(self, level: Level) -> Self;
    pub fn with_format(self, format: LogFormat) -> Self;
    pub fn with_output(self, output: LogOutput) -> Self;
    pub fn init(self) -> Result<(), LogError>;
}
```

### Metrics

```rust
pub struct Metrics {
    registry: Registry,
}

impl Metrics {
    pub fn new() -> Self;
    pub fn counter(&self, name: &str) -> Counter;
    pub fn gauge(&self, name: &str) -> Gauge;
    pub fn histogram(&self, name: &str) -> Histogram;
    pub fn register(&self, metric: Box<dyn Metric>) -> Result<(), MetricsError>;
}
```

## Error Handling

### Error Types

All APIs use structured error types:

```rust
#[derive(Debug, Error)]
pub enum DaemonEyeError {
    #[error("Collection error: {0}")]
    Collection(#[from] CollectionError),

    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),

    #[error("Alert error: {0}")]
    Alert(#[from] AlertError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("IPC error: {0}")]
    Ipc(#[from] IpcError),

    #[error("CLI error: {0}")]
    Cli(#[from] CliError),
}
```

### Error Context

Use `anyhow` for error context:

```rust
use anyhow::{Context, Result};

pub async fn collect_processes() -> Result<Vec<ProcessInfo>> {
    let processes = sysinfo::System::new_all()
        .processes()
        .values()
        .map(|p| ProcessInfo::from(p))
        .collect::<Vec<_>>();

    Ok(processes).context("Failed to collect process information")
}
```

## Examples

### Basic Process Monitoring

```rust
use daemoneye_lib::alerting::AlertManager;
use daemoneye_lib::collector::ProcessCollector;
use daemoneye_lib::storage::Database;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize components
    let collector = ProcessCollector::new();
    let db = Database::new("processes.db").await?;
    let mut alert_manager = AlertManager::new();

    // Add alert sink
    alert_manager.add_sink(Box::new(SyslogSink::new("daemon")?));

    // Collect processes
    let processes = collector.collect_processes().await?;

    // Store in database
    for process in &processes {
        db.insert_process(process).await?;
    }

    // Check for suspicious processes
    let suspicious = db
        .query_processes(
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'",
            &[],
        )
        .await?;

    // Send alerts
    for process in suspicious {
        let alert = Alert::new("suspicious_process", process);
        alert_manager.send_alert(alert).await?;
    }

    Ok(())
}
```

### Custom Alert Sink

```rust
use async_trait::async_trait;
use daemoneye_lib::alerting::{Alert, AlertSink, DeliveryError, DeliveryResult};

pub struct CustomSink {
    endpoint: String,
    client: reqwest::Client,
}

impl CustomSink {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AlertSink for CustomSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, DeliveryError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(alert)
            .send()
            .await
            .map_err(|e| DeliveryError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(DeliveryResult::Success)
        } else {
            Err(DeliveryError::Http(response.status().as_u16()))
        }
    }

    async fn health_check(&self) -> HealthStatus {
        match self.client.get(&self.endpoint).send().await {
            Ok(response) if response.status().is_success() => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    fn name(&self) -> &str {
        "custom_sink"
    }
}
```

---

*This API reference provides comprehensive documentation for all DaemonEye APIs. For additional examples and usage patterns, consult the specific API documentation.*
