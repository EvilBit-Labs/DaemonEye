# SentinelD Technical Stack

## Abstract

This document details the technical stack, architecture patterns, and implementation approaches for
SentinelD. It covers language choices, core crates, cross-platform strategies, security measures,
and performance optimizations. The technical decisions prioritize memory safety, security
boundaries, and operational reliability while addressing critical architecture issues identified in
the review process.

## How to Read This Document

This document provides the technical implementation strategy for SentinelD. See also:

- [Product Specification](product.md) - Mission, architecture overview, and value proposition
- [Project Structure](structure.md) - Workspace layout and module organization
- [Requirements](requirements.md) - Functional and non-functional requirements
- [Tasks & Milestones](tasks.md) - Development phases and priorities

**Note on Diagrams**: Architecture diagrams use Mermaid format. For previews, copy diagram code to
<https://mermaid.live>.

---

## Language & Runtime Foundation

### Rust 2024 Edition

#### Core Language Settings

- **Edition**: 2024 (requires Rust 1.87+)
- **MSRV**: 1.70+ for broader compatibility during transition
- **Safety**: `unsafe_code = "forbid"` at workspace level
- **Quality**: `warnings = "deny"` with `cargo clippy -- -D warnings`

**Rationale**: Rust provides memory safety guarantees essential for security-critical process
monitoring. The 2024 edition enables latest language features while the MSRV ensures reasonable
ecosystem compatibility.

### Async Runtime Foundation

#### Primary Runtime

```toml
tokio = { version = "1.0", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
```

#### Architecture Benefits

- Non-blocking I/O for database operations and network requests
- Cooperative scheduling prevents resource starvation
- Structured logging with context propagation
- Graceful shutdown coordination across components

---

## Core Technology Stack

### Database Layer

#### SQLite with Enhanced Configuration

```toml
rusqlite = {
    version = "0.31",
    features = [
        "bundled",        # Embed SQLite, no system dependency
        "unlock_notify",  # Better concurrent access
        "hooks",         # Authorizer callbacks for security
        "column_decltype" # Schema introspection
    ]
}
```

#### WAL Mode Configuration

```sql
-- Performance-optimized settings
PRAGMA journal_mode = WAL;           -- Write-Ahead Logging
PRAGMA synchronous = NORMAL;         -- Balance durability/performance
PRAGMA cache_size = -64000;          -- 64MB cache
PRAGMA temp_store = MEMORY;          -- Memory temp tables
PRAGMA mmap_size = 268435456;        -- 256MB memory-mapped I/O

-- Security settings
PRAGMA foreign_keys = ON;            -- Enforce constraints
PRAGMA query_only = ON;              -- Read-only connections for detection
PRAGMA defensive = ON;               -- Additional safety checks
```

#### Database Architecture Strategy

- **Event Store**: High-performance process snapshots with WAL=NORMAL
- **Audit Ledger**: Tamper-evident chain with WAL=FULL for durability
- **Detection Views**: Read-only connections with prepared statements
- **Migration System**: Embedded schema versioning with rusqlite_migration

### Configuration Management

#### Hierarchical Configuration System

```toml
config = "0.14"                                                            # Multi-format config loading
figment = { version = "0.10", features = ["toml", "json", "yaml", "env"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

#### Configuration Precedence

1. Embedded defaults
2. System files (`/etc/sentineld/config.yaml`)
3. User files (`~/.config/sentineld/config.yaml`)
4. Environment variables (`SENTINELD_*`)
5. Command-line arguments (highest)

### Error Handling Strategy

#### Structured Error Management

```toml
anyhow = "1.0"    # Application error context
thiserror = "1.0" # Library error definitions
```

#### Pattern Implementation

- **Libraries**: Use `thiserror` for structured error types
- **Applications**: Use `anyhow` for context and error chains
- **Recovery**: Graceful degradation with detailed error context
- **Logging**: Structured error events with tracing spans

### CLI & Terminal Integration

#### User Interface Framework

```toml
clap = { version = "4.0", features = ["derive", "completion"] }
```

#### Color & Terminal Handling

- Automatic color detection via `tty` detection
- **NO_COLOR** environment variable support (RFC 8405)
- **TERM=dumb** terminal detection for CI/automation
- Shell completion generation (bash, zsh, fish, PowerShell)

---

## Cross-Platform OS Integration

### Process Enumeration Strategy

#### Phase 1: Cross-Platform Baseline

```toml
sysinfo = "0.30" # Unified process enumeration
```

#### Platform-Specific Enhancements

```rust
#[cfg(target_os = "linux")]
mod linux {
    // /proc filesystem parsing
    // Optional: procfs crate for structured access
}

#[cfg(target_os = "macos")]
mod macos {
    // libproc bindings
    // System Configuration framework
}

#[cfg(target_os = "windows")]
mod windows {
    // PSAPI and WMI integration
    // Performance Data Helper (PDH)
}
```

### Event Ingestion (Phase 2/Future)

#### Linux eBPF Integration

```toml
aya = { version = "0.12", optional = true }
aya-bpf = { version = "0.1", optional = true }
```

- **Capabilities**: CAP_BPF, CAP_SYS_ADMIN required
- **Fallback**: /proc polling when eBPF unavailable
- **Programs**: Process lifecycle, exec, clone, exit events

#### Windows ETW Integration

```toml
windows = { version = "0.52", features = ["Win32_System_Diagnostics_Etw"] }
wmi = "0.13"
```

- **Privileges**: SeDebugPrivilege for enhanced access
- **Providers**: Kernel process events, WMI process creation
- **Fallback**: WMI polling when ETW unavailable

#### macOS Event Sources

```toml
# EndpointSecurity framework (when available)
core-foundation = "0.9"
core-foundation-sys = "0.8"
# Fallback: kqueue/OpenBSM
libc = "0.2"
```

- **Entitlements**: EndpointSecurity requires special entitlements
- **Fallback**: kqueue for file events, OpenBSM for audit trail
- **Sandboxing**: App Sandbox compatibility considerations

### Privilege Management

#### Linux Capabilities

```rust
// Minimal required capabilities
CAP_SYS_PTRACE  // Process memory access (optional)
CAP_DAC_READ_SEARCH // Enhanced file access (optional)

// Immediate drop after initialization
fn drop_privileges() -> Result<()> {
    // Implementation using caps crate
}
```

#### Windows Privileges

```rust
// Enhanced debug privileges (optional)
SeDebugPrivilege  // Enhanced process access

// Token restriction after init
fn restrict_token() -> Result<()> {
    // Implementation using windows crate
}
```

#### macOS Sandbox/Entitlements

```xml
<!-- Minimal required entitlements -->
<key>com.apple.security.cs.allow-unsigned-executable-memory</key>
<false/>
<key>com.apple.security.cs.disable-library-validation</key>
<false/>
```

---

## SQL Detection Engine Security

### SQL Parser & Validation

#### AST-Based Security

```toml
sqlparser = { version = "0.40", features = ["serde"] }
```

#### Query Whitelist Strategy

```rust
// Allowed SQL constructs
allowed_statements: [SELECT]
allowed_clauses: [WHERE, GROUP_BY, HAVING, ORDER_BY, LIMIT]
allowed_functions: [COUNT, SUM, AVG, MIN, MAX, LENGTH, SUBSTR]
forbidden_operations: [
    INSERT, UPDATE, DELETE,    // Data modification
    CREATE, DROP, ALTER,       // Schema changes
    PRAGMA,                    // Configuration changes
    ATTACH, DETACH,           // Database manipulation
    VACUUM, ANALYZE           // Maintenance operations
]
```

#### Prepared Statement Security

```rust
// All user queries must use prepared statements
let stmt = conn.prepare_cached(validated_sql)?;
let params = rusqlite::params![]; // Only positional parameters
let results = stmt.query_and_then(params, |row| {
    // Resource-limited row processing
})?;
```

### Sandboxed Execution Environment

#### Process Isolation Strategy

```toml
# Optional separate detection executor
nix = "0.27"     # Linux: seccomp, namespaces
windows = "0.52" # Windows: job objects, restricted tokens
libc = "0.2"     # macOS: resource limits, sandbox
```

#### Security Boundaries

- **Linux**: seccomp-bpf profiles, user namespaces, rlimits
- **Windows**: Job objects with restricted tokens, memory limits
- **macOS**: Resource limits, mandatory access control
- **All Platforms**: Timeout enforcement, memory budgets, FD limits

#### Isolation Implementation

```rust
pub struct DetectionExecutor {
    // Read-only database snapshot
    db_path: PathBuf,
    // Resource limits
    memory_limit: usize,  // 64MB default
    time_limit: Duration, // 30s default
    // Cancellation support
    cancel_token: CancellationToken,
}
```

---

## Cryptographic & Audit Components

### Tamper-Evident Logging

#### Cryptographic Foundation

```toml
blake3 = "1.5"        # Fast cryptographic hashing
ed25519-dalek = "2.0" # Optional digital signatures
hmac = "0.12"         # Message authentication
subtle = "2.4"        # Constant-time comparisons
```

#### Audit Chain Structure

```rust
#[derive(Serialize, Deserialize)]
pub struct AuditEntry {
    sequence: u64,
    timestamp: SystemTime,
    actor: String,               // Component performing action
    action: String,              // Operation type
    payload_hash: [u8; 32],      // BLAKE3 of event data
    prev_hash: [u8; 32],         // Previous entry hash
    entry_hash: [u8; 32],        // This entry's hash
    signature: Option<[u8; 64]>, // Optional Ed25519 signature
}
```

#### Chain Verification

```rust
pub fn verify_audit_chain(entries: &[AuditEntry]) -> Result<bool> {
    for (i, entry) in entries.iter().enumerate() {
        // Verify hash chain
        if i > 0 && entry.prev_hash != entries[i - 1].entry_hash {
            return Ok(false);
        }

        // Verify entry hash
        let computed = blake3::hash(&entry.canonical_bytes());
        if computed.as_bytes() != &entry.entry_hash {
            return Ok(false);
        }

        // Verify signature if present
        if let Some(sig) = entry.signature {
            // Ed25519 verification
        }
    }
    Ok(true)
}
```

### Database Performance Optimization

#### Write Bottleneck Mitigation

```rust
pub struct BatchWriter {
    queue: bounded_channel::Receiver<ProcessRecord>,
    db: Connection,
    batch_size: usize,        // 1000 records default
    flush_interval: Duration, // 5s default
    backpressure: BackpressurePolicy,
}

pub enum BackpressurePolicy {
    DropOldest,                 // Evict oldest records
    BlockWithTimeout(Duration), // Block up to timeout
    SpillToDisk(PathBuf),       // Overflow to disk
    AdaptiveThrottling,         // Reduce collection rate
}
```

#### Fsync Strategy

- **Event Store**: WAL with NORMAL sync (performance priority)
- **Audit Ledger**: WAL with FULL sync (durability priority)
- **Coalescing**: Batch fsync operations during low activity
- **Checkpointing**: Periodic WAL checkpoint during maintenance windows

---

## Alerting & Integration Layer

### Multi-Channel Alert Delivery

#### Alert Sink Implementations

```toml
# HTTP webhooks
reqwest = { version = "0.11", features = ["json", "rustls-tls"] }

# Email delivery
lettre = { version = "0.10", features = ["rustls-tls", "smtp-transport"] }

# Syslog integration
syslog = "6.0"

# Structured output
serde_json = "1.0"
```

#### Reliability Framework

```rust
pub trait AlertSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult>;
    async fn health_check(&self) -> HealthStatus;
}

pub struct ReliableAlertDispatcher {
    sinks: Vec<Box<dyn AlertSink>>,
    circuit_breakers: HashMap<String, CircuitBreaker>,
    retry_policy: RetryPolicy,
    dead_letter_queue: Option<DeadLetterQueue>,
}

pub struct RetryPolicy {
    max_attempts: u32,        // 3 attempts default
    initial_delay: Duration,  // 1s default
    max_delay: Duration,      // 60s default
    multiplier: f64,         // 2.0 exponential backoff
    jitter: bool,            // Add randomization
}
```

### Circuit Breaker Pattern

```rust
pub struct CircuitBreaker {
    state: Arc<Mutex<CircuitState>>,
    failure_threshold: u32,     // 5 failures default
    recovery_timeout: Duration, // 30s default
    success_threshold: u32,     // 2 successes for recovery
}

pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing fast
    HalfOpen, // Testing recovery
}
```

---

## Resource Management & Performance

### Memory Management Strategy

#### Bounded Resource Design

```rust
pub struct ResourceManager {
    memory_budget: usize,          // 100MB default
    channel_capacity: usize,       // 10000 items default
    task_pool_size: usize,         // CPU cores * 2
    collection_concurrency: usize, // 4 concurrent collectors
}

pub struct BoundedChannel<T> {
    sender: tokio::sync::mpsc::Sender<T>,
    receiver: tokio::sync::mpsc::Receiver<T>,
    capacity: usize,
    backpressure: BackpressurePolicy,
}
```

#### Cooperative Task Management

```rust
async fn process_collection_loop() -> Result<()> {
    loop {
        // Cooperative yield point
        tokio::task::yield_now().await;

        // Check cancellation
        if cancel_token.is_cancelled() {
            break;
        }

        // Bounded work unit
        let batch = collect_process_batch(BATCH_SIZE).await?;

        // Memory pressure check
        if memory_usage() > memory_budget * 0.8 {
            apply_backpressure().await?;
        }

        process_batch(batch).await?;
    }
    Ok(())
}
```

### Performance Monitoring

#### Optional Metrics Integration

```toml
prometheus = { version = "0.13", optional = true }
```

#### Key Metrics

```rust
// Collection performance
process_enumeration_duration_seconds: Histogram
processes_collected_total: Counter
collection_errors_total: Counter

// Database performance
database_write_duration_seconds: Histogram
database_writes_total: Counter
database_size_bytes: Gauge

// Detection performance
detection_rule_duration_seconds: Histogram
alerts_generated_total: Counter
rule_errors_total: Counter

// Alert performance
alert_delivery_duration_seconds: Histogram
alert_deliveries_total: Counter
alert_retry_attempts_total: Counter
```

---

## Deployment & Packaging Strategy

### Static Binary Distribution

#### Build Configuration

```toml
# Cargo.toml - Release optimization
[profile.release]
lto = "thin"      # Link-time optimization
codegen-units = 1 # Better optimization
panic = "abort"   # Smaller binary
strip = "symbols" # Remove debug symbols
```

#### Cross-Platform Packaging

- **Linux**: Static binary with embedded SQLite, systemd service files
- **macOS**: Homebrew formula, launchd plist, code signing/notarization
- **Windows**: MSI installer, Windows Service manifest, code signing

### Service Integration

#### Linux systemd Hardening

```ini
[Unit]
Description=SentinelD Process Monitor
After=network.target

[Service]
Type=notify
ExecStart=/usr/bin/procmond run
Restart=always
RestartSec=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/sentineld
CapabilityBoundingSet=CAP_SYS_PTRACE
AmbientCapabilities=CAP_SYS_PTRACE

[Install]
WantedBy=multi-user.target
```

#### Service Configuration Management

- **Secrets**: Environment variables or OS keychain integration
- **Validation**: Startup configuration validation with clear errors
- **Reloading**: SIGHUP configuration reload support
- **Logging**: Structured JSON logs for log aggregation

---

## Testing & Quality Assurance

### Testing Framework Integration

#### Core Testing Stack

```toml
# Unit and integration testing
tokio-test = "0.4" # Async test utilities
tempfile = "3.8"   # Temporary file management
assert_cmd = "2.0" # CLI integration tests
predicates = "3.0" # Output validation

# Property-based testing
proptest = "1.4" # Generative testing

# Performance testing
criterion = { version = "0.5", features = ["html_reports"] }

# Snapshot testing
insta = "1.34" # Output snapshot testing
```

#### Testing Strategy Implementation

```rust
// Performance benchmark example
#[bench]
fn bench_process_enumeration(b: &mut Bencher) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let collector = ProcessCollector::new();

    b.to_async(&rt).iter(|| async {
        let result = collector.collect_processes().await;
        black_box(result)
    });
}

// Snapshot testing example
#[test]
fn test_alert_format() {
    let alert = create_test_alert();
    let json_output = serde_json::to_string_pretty(&alert).unwrap();

    // Deterministic output for CI
    std::env::set_var("NO_COLOR", "1");
    std::env::set_var("TERM", "dumb");

    insta::assert_snapshot!(json_output);
}

// CLI integration testing
#[test]
fn test_cli_self_check() {
    let mut cmd = Command::cargo_bin("procmond").unwrap();
    cmd.arg("self-check").arg("--verbose");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("System Diagnostics"));
}
```

### Continuous Integration Matrix

#### Platform Testing

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, windows-latest, macos-latest]
    rust: [stable, beta, 1.70.0]  # MSRV testing
```

#### Quality Gates

- **Linting**: `cargo clippy -- -D warnings`
- **Formatting**: `cargo fmt --check`
- **Security**: `cargo audit` dependency scanning
- **Performance**: Criterion benchmark regression detection
- **Coverage**: Minimum 85% code coverage with tarpaulin

---

## Security Considerations

### Supply Chain Security

#### Dependency Management

```toml
[workspace.dependencies]
# Pin critical security dependencies
rusqlite = "=0.31.0"     # Exact version for security
ring = "=0.17.5"         # Crypto exact version
ed25519-dalek = "=2.0.0" # Signature exact version
```

#### Security Scanning Integration

- **cargo-audit**: Known vulnerability scanning
- **cargo-deny**: License and dependency policy enforcement
- **SLSA Build Provenance**: Build attestation generation
- **Reproducible Builds**: Deterministic binary generation

### Runtime Security Measures

#### Input Validation Framework

```rust
pub trait Validate {
    type Error;
    fn validate(&self) -> Result<(), Self::Error>;
}

// Example: SQL query validation
impl Validate for DetectionRule {
    type Error = ValidationError;

    fn validate(&self) -> Result<(), Self::Error> {
        // Parse SQL to AST
        let ast = sqlparser::parse(&self.sql_query)?;

        // Validate against whitelist
        for statement in ast {
            match statement {
                Statement::Query(_) => (), // Allowed
                _ => return Err(ValidationError::ForbiddenStatement),
            }
        }

        Ok(())
    }
}
```

#### Credential Management

```rust
pub struct CredentialManager {
    // Platform-specific secure storage
    #[cfg(target_os = "linux")]
    keyring: secret_service::SecretService,

    #[cfg(target_os = "macos")]
    keychain: security_framework::os::macos::keychain::SecKeychain,

    #[cfg(target_os = "windows")]
    credential_store: windows::Win32::Security::Credentials::CREDENTIAL,
}
```

---

## Conclusion

The SentinelD technical stack prioritizes security, performance, and reliability through careful
technology selection and architectural patterns. The Rust foundation provides memory safety, while
the layered security approach addresses critical vulnerabilities identified in the architecture
review.

Key technical decisions include:

1. **Database Strategy**: Dual-store architecture with separate event and audit databases to balance
   performance and integrity requirements
2. **Security Framework**: Defense-in-depth with AST validation, sandboxed execution, and privilege
   separation
3. **Resource Management**: Bounded channels and cooperative scheduling prevent resource exhaustion
4. **Cross-Platform Support**: Progressive enhancement with graceful degradation for
   platform-specific features
5. **Quality Assurance**: Comprehensive testing strategy with performance benchmarks and security
   validation

This technical foundation addresses the critical issues identified in the architecture review while
providing a scalable platform for future enhancements.
