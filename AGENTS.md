# AI Coding Assistant Configuration: SentinelD

---

## Project Overview

**SentinelD** is a high-performance, security-focused process monitoring system built in Rust 2024 Edition. It's a complete rewrite of the Python prototype, designed for cybersecurity professionals, threat hunters, and security operations centers.

### Architecture Principles

SentinelD follows a **three-component security architecture**:

1. **ProcMonD (Collector)**: Privileged process monitoring daemon with minimal attack surface
2. **SentinelAgent (Orchestrator)**: User-space process for alerting and network operations
3. **SentinelCLI**: Command-line interface for queries and configuration
4. **sentinel-lib**: Shared library providing common functionality across components

This separation ensures **robust security** by isolating privileged operations from network functionality.

---

## Technology Stack Requirements

### Core Technologies

- **Language**: Rust 2024 Edition (MSRV: 1.70+)
- **Async Runtime**: Tokio for I/O and task management
- **Database**: SQLite 3.42+ with WAL mode for concurrent operations
- **CLI Framework**: clap v4 with derive macros and shell completions
- **Process Enumeration**: sysinfo crate with platform-specific optimizations
- **Logging**: tracing ecosystem with structured JSON output
- **Configuration**: YAML/TOML via serde with hierarchical overrides
- **Testing**: Comprehensive unit and integration tests with assert_cmd and predicates

### Key Dependencies

```toml
[workspace.dependencies]
tokio = { version = "1.0", features = ["full"] }
clap = { version = "4.0", features = ["derive", "completion"] }
serde = { version = "1.0", features = ["derive"] }
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "sqlite"] }
sysinfo = "0.30"
tracing = "0.1"
thiserror = "1.0"
anyhow = "1.0"
```

---

## Coding Standards and Conventions

### Rust-Specific Requirements

- **Edition**: Always use Rust 2024 Edition
- **Linting**: `cargo clippy -- -D warnings` (zero warnings policy)
- **Safety**: `unsafe_code = "forbid"` enforced at workspace level
- **Formatting**: Standard `rustfmt` with 119 character line length
- **Error Handling**: Use `thiserror` for structured errors, `anyhow` for error context
- **Async**: Async-first design using Tokio runtime

### Performance Requirements

- **CPU Usage**: <5% sustained during continuous monitoring
- **Memory Usage**: <100MB resident under normal operation
- **Process Enumeration**: <5s for 10,000+ processes
- **Database Operations**: >1000 records/sec write rate
- **Alert Latency**: <100ms per detection rule

### Security Requirements

- **Principle of Least Privilege**: Components run with minimal required permissions
- **SQL Injection Prevention**: Use parameterized queries and prepared statements
- **Credential Management**: No hardcoded credentials, prefer environment variables
- **Input Validation**: Comprehensive validation with detailed error messages
- **Attack Surface Minimization**: No network listening, outbound-only connections

---

## Code Organization and Architecture

### Workspace Structure

```text
SentinelD/
├── procmond/           # Process monitoring daemon (bin)
│   ├── src/main.rs     # Entry point
│   ├── src/lib.rs      # Core logic
│   └── tests/          # Integration tests
├── sentinelcli/        # CLI interface (bin)
├── sentinelagent/      # Orchestrator agent (bin)
├── sentinel-lib/       # Shared library
│   ├── src/config.rs   # Configuration management
│   ├── src/model.rs    # Data structures
│   ├── src/storage.rs  # Database operations
│   ├── src/collector.rs # Process enumeration
│   ├── src/detector.rs # Detection engine
│   └── src/alerting.rs # Alert delivery
└── tests/              # Integration tests
```

### Service Layer Pattern

Implement clear separation of concerns with trait-based service interfaces:

```rust
#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    async fn collect_processes(&self) -> Result<CollectionResult, CollectionError>;
    async fn get_system_info(&self) -> Result<SystemInfo, CollectionError>;
}

#[async_trait]
pub trait DetectionService: Send + Sync {
    async fn execute_rules(&self, scan_context: &ScanContext) -> Result<Vec<Alert>, DetectionError>;
    async fn load_rules(&self) -> Result<Vec<DetectionRule>, DetectionError>;
}
```

---

## Data Structures and Models

### Core Data Types

Use strongly-typed structures with serde for serialization:

```rust
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
    pub executable_hash: Option<String>,
    pub collection_time: DateTime<Utc>,
}
```

### Error Handling

Use thiserror for structured error types:

```rust
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },

    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}
```

---

## Database Design

### SQLite Schema

Use SQLite with WAL mode for concurrent operations:

```sql
CREATE TABLE processes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scan_id INTEGER NOT NULL,
    collection_time INTEGER NOT NULL,
    pid INTEGER NOT NULL,
    ppid INTEGER,
    name TEXT NOT NULL,
    executable_path TEXT,
    command_line TEXT,
    executable_hash TEXT,
    -- Additional fields...
    FOREIGN KEY (scan_id) REFERENCES scans(id)
);
```

### Migration Strategy

Use embedded migrations with rusqlite_migration for schema versioning.

---

## Testing Strategy

### Three-Tier Testing Architecture

1. **Unit Tests**: Test individual components with mocked dependencies
2. **Integration Tests**: Use testcontainers for database operations
3. **End-to-End Tests**: Full system testing with sample data

### Test Organization

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use assert_cmd::prelude::*;
    use predicates::prelude::*;

    #[tokio::test]
    async fn test_process_collection() {
        // Test implementation
    }
}
```

---

## CLI Design Guidelines

### Command Structure

Use clap v4 with derive macros:

```rust
#[derive(Parser)]
#[command(name = "procmond", about = "Process monitoring daemon")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[arg(short, long, default_value = "info")]
    pub log_level: String,
}

#[derive(Subcommand)]
pub enum Commands {
    Run(RunCommand),
    Config(ConfigCommand),
    Rules(RulesCommand),
    // Additional commands...
}
```

### Output Formatting

Support both human-readable and JSON output:

- Use `--json` flag for machine-readable output
- Respect `NO_COLOR` and `TERM=dumb` for color handling
- Provide clear error messages with actionable suggestions

---

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
  path: "/var/lib/procmond/processes.sqlite"
  retention_days: 30

alerting:
  sinks:
    - type: "syslog"
      facility: "daemon"
    - type: "webhook"
      url: "https://alerts.example.com/api"
```

---

## Alert System Design

### Plugin-Based Alert Sinks

Use trait-based plugin system:

```rust
#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}
```

### Alert Delivery

- Concurrent delivery to multiple sinks
- Retry logic with exponential backoff
- Circuit breaking for failing sinks
- Delivery audit trail

---

## Development Workflow

### Task Runner (just)

Use DRY principles in justfile recipes:

```just
fmt:
  @cargo fmt --all

lint:
  @just fmt-check
  @cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
  @cargo test --workspace

build:
  @cargo build --workspace
```

### Git Workflow

- Use conventional commits format
- Create feature branches for new work
- Ensure all tests pass before merging
- No commits without explicit permission

---

## Platform Considerations

### Cross-Platform Compatibility

- Linux: Primary development target with `/proc` filesystem access
- macOS: Native process enumeration with security framework integration
- Windows: Windows API process access with service deployment

### Graceful Degradation

- Continue with reduced functionality when elevated privileges unavailable
- Provide clear diagnostics about system capabilities
- Fallback mechanisms for constrained environments

---

## Security Considerations

### Privilege Management

- Run with minimal required privileges by default
- Optional elevated mode for enhanced metadata collection
- Automatic privilege dropping after initialization
- Comprehensive audit logging

### Data Protection

- Optional command-line redaction for privacy
- Configurable field masking in logs
- Secure credential storage (OS keychain integration)
- Database encryption support for sensitive deployments

---

## Observability and Monitoring

### Metrics Export

Support Prometheus metrics for operational monitoring:

```rust
procmond_collection_duration_seconds{status="success|error"}
procmond_processes_collected_total
procmond_alerts_generated_total{severity="low|medium|high|critical"}
procmond_alert_deliveries_total{sink="stdout|syslog|webhook"}
```

### Health Checks

- Configuration validation endpoints
- Database connectivity checks
- Alert sink health monitoring
- System capability assessment

---

## Migration from Python Prototype

### Data Migration

- Built-in database import utility
- Configuration format conversion (INI → YAML/TOML)
- Rule migration assistance
- Feature parity validation

### Deployment Strategy

1. Parallel operation during transition
2. Output comparison and validation
3. Gradual migration after validation period
4. Comprehensive migration documentation

---

## Code Generation Guidelines

When generating code for SentinelD:

1. **Always use Rust 2024 Edition** in Cargo.toml files
2. **Implement comprehensive error handling** with thiserror
3. **Use async/await patterns** with Tokio runtime
4. **Include comprehensive tests** with assert_cmd for CLI testing
5. **Follow the service layer pattern** with trait definitions
6. **Implement proper logging** with tracing framework
7. **Use workspace inheritance** for common dependencies
8. **Include performance considerations** in implementation
9. **Follow security best practices** with input validation
10. **Document all public APIs** with rustdoc comments

---

## Specific Implementation Notes

### Process Collection

- Use `sysinfo` crate as primary cross-platform abstraction
- Implement platform-specific optimizations where beneficial
- Handle permission denied gracefully with partial data collection
- Compute SHA-256 hashes of executable files for integrity checking

### Detection Engine

- Execute SQL rules against SQLite database with prepared statements
- Implement rule validation and testing capabilities
- Support hot-reloading of rule files with change detection
- Provide performance metrics and optimization hints

### Alert Delivery Implementation

- Implement concurrent delivery with configurable parallelism
- Use circuit breaker pattern for failing external services
- Maintain delivery audit trail for compliance
- Support rate limiting and backpressure handling

### Database Operations

- Use connection pooling for concurrent access
- Implement automatic schema migrations
- Support database maintenance operations (VACUUM, ANALYZE)
- Provide detailed statistics and performance metrics

---

**Remember**: SentinelD is a security-focused system. Always prioritize security, performance, and reliability in implementation decisions. When in doubt, choose the more secure and observable approach.
