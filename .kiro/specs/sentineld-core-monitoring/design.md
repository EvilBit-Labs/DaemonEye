# Design Document

## Overview

SentinelD implements a three-component security architecture with strict privilege separation to provide continuous process monitoring and threat detection. The system is designed around the principle of minimal attack surface while maintaining high performance and audit-grade integrity.

The core design follows a pipeline architecture where process data flows from collection through detection to alerting, with each component having clearly defined responsibilities and security boundaries.

## Architecture

### Component Architecture

SentinelD consists of three main components that work together to provide comprehensive process monitoring:

```text
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│    procmond     │    │  sentinelagent  │    │   sentinelcli   │
│   (Collector)   │◀──▶│ (Orchestrator)  │◀───│  (Interface)    │
│                 │    │                 │    │                 │
│ • Privileged    │    │ • User space    │    │ • User space    │
│ • Process enum  │    │ • SQL engine    │    │ • Queries       │
│ • Hash compute  │    │ • Rule mgmt     │    │ • Management    │
│ • Audit logging │    │ • Detection     │    │ • Diagnostics   │
│ • Protobuf IPC  │    │ • Alerting      │    │ • Config mgmt   │
│                 │    │ • Network comm  │    │                 │
└─────────────────┘    └─────────────────┘    └─────────────────┘
         │                       │
         ▼                       ▼
┌─────────────────┐    ┌─────────────────┐
│  Audit Ledger   │    │   Event Store   │
│ (Merkle Tree)   │    │     (redb)      │
│ procmond write  │    │  sentinelagent  │
│     only        │    │    managed      │
└─────────────────┘    └─────────────────┘

IPC Protocol: Custom Protobuf over Unix Sockets/Named Pipes
Communication: sentinelcli ↔ sentinelagent ↔ procmond
Service Management: sentinelagent manages procmond lifecycle
```

### Data Flow Architecture

The system implements a pipeline processing model with clear phases and strict component separation:

1. **Collection Phase**: procmond enumerates processes and computes hashes
2. **SQL-to-IPC Translation**: sentinelagent uses sqlparser to extract collection requirements from SQL AST
3. **Task Generation**: Complex SQL detection rules translated into simple protobuf collection tasks
4. **IPC Communication**: procmond receives simple detection tasks via protobuf over IPC
5. **Overcollection Strategy**: procmond may overcollect data due to granularity limitations
6. **Audit Logging**: procmond writes to tamper-evident audit ledger (write-only; redb + rs-merkle Merkle tree)
7. **Detection Phase**: sentinelagent executes original SQL rules against collected/stored data
8. **Alert Generation**: Structured alerts with deduplication and context
9. **Delivery Phase**: Multi-channel alert delivery with reliability guarantees

**Key Architectural Principles**:

- procmond has minimal complexity and attack surface
- All complex logic (SQL, networking, redb) handled by sentinelagent
- IPC protocol is simple and purpose-built for security
- Clear separation between audit logging (procmond) and event processing (sentinelagent)
- Pure Rust stack with redb for optimal performance and security
- Zero unsafe code goal with any required unsafe code isolated to procmond only

## Components and Interfaces

### procmond (Privileged Collector)

**Purpose**: Minimal privileged component for secure process data collection with purpose-built simplicity

**Key Interfaces**:

- `ProcessCollector` trait for cross-platform process enumeration
- `HashComputer` trait for executable integrity verification
- `AuditLogger` trait for tamper-evident logging
- `IpcServer` trait for protobuf-based communication with sentinelagent

**Core Implementation**:

```rust
pub struct ProcessCollector {
    config: CollectorConfig,
    hash_computer: Box<dyn HashComputer>,
    audit_logger: Box<dyn AuditLogger>,
    ipc_server: Box<dyn IpcServer>,
}

#[async_trait]
pub trait ProcessCollector {
    async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>>;
    async fn handle_detection_task(&self, task: DetectionTask) -> Result<DetectionResult>;
    async fn serve_ipc(&self) -> Result<()>;
}

// Simple protobuf-based detection tasks from sentinelagent
#[derive(Clone, PartialEq, Message)]
pub struct DetectionTask {
    #[prost(string, tag = "1")]
    pub task_id: String,
    #[prost(enumeration = "TaskType", tag = "2")]
    pub task_type: i32,
    #[prost(message, optional, tag = "3")]
    pub process_filter: Option<ProcessFilter>,
    #[prost(message, optional, tag = "4")]
    pub hash_check: Option<HashCheck>,
}
```

**Security Boundaries**:

- Starts with minimal privileges, optionally requests enhanced access
- Drops all elevated privileges immediately after initialization
- No network access whatsoever
- No SQL parsing or complex query logic
- Write-only access to audit ledger
- Simple protobuf IPC only (Unix sockets/named pipes)
- Purpose-built for stability and minimal attack surface
- Zero unsafe code goal; any required unsafe code isolated to highly structured, tested modules

### sentinelagent (Detection Orchestrator)

**Purpose**: User-space detection rule execution, alert management, and procmond lifecycle management

**Key Interfaces**:

- `DetectionEngine` trait for SQL rule execution
- `AlertManager` trait for alert generation and deduplication
- `AlertSink` trait for pluggable delivery channels
- `RuleManager` trait for rule loading and validation
- `ProcessManager` trait for procmond lifecycle management

**Core Implementation**:

```rust
pub struct DetectionEngine {
    db: redb::Database,
    rule_manager: RuleManager,
    alert_manager: AlertManager,
    sql_validator: SqlValidator,
    ipc_client: IpcClient,
}

#[async_trait]
pub trait DetectionEngine {
    async fn execute_rules(&self, scan_id: i64) -> Result<Vec<Alert>>;
    async fn validate_sql(&self, query: &str) -> Result<ValidationResult>;
}
```

**Security Boundaries**:

- Operates in user space with minimal privileges
- Manages redb event store (read/write access)
- **SQL-to-IPC Translation**: Uses sqlparser to analyze SQL detection rules and extract collection requirements
- **Task Generation**: Translates complex SQL queries into simple protobuf collection tasks for procmond
- **Overcollection Handling**: May request broader data collection than SQL requires, then applies SQL filtering to stored data
- **Privilege Separation**: SQL execution never directly touches live processes; only simple collection tasks sent via IPC
- Outbound-only network connections for alert delivery
- Sandboxed rule execution with resource limits
- IPC client for communication with procmond
- Manages procmond process lifecycle (start, stop, restart, health monitoring)

**Service Management**:

- Primary service registered with system service manager (systemd, launchd, Windows Service)
- Responsible for starting and monitoring procmond as a child process
- Handles graceful shutdown coordination between components
- Manages service dependencies and startup ordering
- Provides unified logging and health reporting for the entire system

### sentinelcli (Operator Interface)

**Purpose**: Command-line interface for queries, management, and diagnostics

**Key Interfaces**:

- `QueryExecutor` trait for safe SQL query execution
- `RuleManager` trait for rule management operations
- `HealthChecker` trait for system diagnostics
- `DataExporter` trait for data export functionality

**Core Implementation**:

```rust
pub struct QueryExecutor {
    db: redb::Database,
    sql_validator: SqlValidator,
    output_formatter: OutputFormatter,
}

#[async_trait]
pub trait QueryExecutor {
    async fn execute_query(&self, query: &str, params: &[Value]) -> Result<QueryResult>;
    async fn export_data(&self, format: ExportFormat, filter: &Filter) -> Result<ExportResult>;
}
```

**Security Boundaries**:

- No network access
- No direct database access (communicates through sentinelagent)
- Input validation for all user-provided data
- Safe SQL execution via sentinelagent with prepared statements
- Communicates only with sentinelagent for all operations

### IPC Protocol Design

**Purpose**: Secure, efficient communication between procmond and sentinelagent

**Protocol Specification**:

```protobuf
syntax = "proto3";

// Simple detection tasks sent from sentinelagent to procmond
message DetectionTask {
    string task_id = 1;
    TaskType task_type = 2;
    optional ProcessFilter process_filter = 3;
    optional HashCheck hash_check = 4;
    optional string metadata = 5;
}

enum TaskType {
    ENUMERATE_PROCESSES = 0;
    CHECK_PROCESS_HASH = 1;
    MONITOR_PROCESS_TREE = 2;
    VERIFY_EXECUTABLE = 3;
}

message ProcessFilter {
    repeated string process_names = 1;
    repeated uint32 pids = 2;
    optional string executable_pattern = 3;
}

message HashCheck {
    string expected_hash = 1;
    string hash_algorithm = 2;
    string executable_path = 3;
}

// Results sent back from procmond to sentinelagent
message DetectionResult {
    string task_id = 1;
    bool success = 2;
    optional string error_message = 3;
    repeated ProcessRecord processes = 4;
    optional HashResult hash_result = 5;
}
```

**Transport Layer**:

- Unix domain sockets on Linux/macOS
- Named pipes on Windows
- Async message handling with tokio
- Connection authentication and encryption (optional)
- Automatic reconnection with exponential backoff

## Data Models

### Core Data Structures

**ProcessRecord**: Represents a single process snapshot

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub id: Uuid,
    pub scan_id: i64,
    pub collection_time: i64, // Unix timestamp in milliseconds
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub executable_path: Option<PathBuf>,
    pub command_line: Vec<String>,
    pub start_time: Option<i64>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
    pub executable_hash: Option<String>,
    pub hash_algorithm: Option<String>,
    pub user_id: Option<String>,
    pub accessible: bool,
    pub file_exists: bool,
    pub platform_data: Option<serde_json::Value>,
}
```

**Alert**: Represents a detection result with full context

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: Uuid,
    pub alert_time: i64,
    pub rule_id: String,
    pub title: String,
    pub description: String,
    pub severity: AlertSeverity,
    pub scan_id: Option<i64>,
    pub affected_processes: Vec<u32>,
    pub process_count: i32,
    pub alert_data: serde_json::Value,
    pub rule_execution_time_ms: Option<i64>,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}
```

**DetectionRule**: SQL-based detection rule with metadata

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionRule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: i32,
    pub sql_query: String,
    pub enabled: bool,
    pub severity: AlertSeverity,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub source_type: RuleSourceType,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleSourceType {
    Builtin,
    File,
    User,
}
```

### Database Schema Design

The system uses redb (pure Rust embedded database) for optimal performance and security:

**Core Tables**:

- `processes`: Process snapshots with comprehensive metadata
- `scans`: Collection cycle metadata and statistics
- `detection_rules`: Rule definitions with versioning (rules translated to simple tasks for procmond)
- `alerts`: Generated alerts with execution context
- `alert_deliveries`: Delivery tracking with retry information

**Audit Ledger** (redb + rs-merkle, CT-style Merkle log):

The audit ledger is an append-only Merkle tree backed by `redb` rather than a relational schema. It provides tamper-evident logging with inclusion proofs and periodic signed checkpoints.

Key Concepts:

- Append-only sequence of canonical JSON leaf entries.
- BLAKE3 used for leaf hashing (uniform algorithm per lineage).
- Incremental Merkle root updated each append (`O(log n)` path).
- Inclusion proofs (sibling hash list) generated on demand; optionally cached.
- Periodic checkpoints `(tree_size, root_hash[, signature])` persisted for rapid recovery & external audit.
- Optional Ed25519 signatures over `(tree_size || root_hash)` for auditor trust anchoring.
- Air‑gap export: Signed checkpoints + sampled proofs can be exported for offline verification.

redb Logical Layout (conceptual):

- `AUDIT_LEDGER (u64 -> AuditEntry)`
- `AUDIT_CHECKPOINTS (u64 tree_size -> Checkpoint)`

Simplified Data Structures (illustrative):

```rust
#[derive(Serialize, Deserialize, Clone)]
struct AuditEntry {
    id: u64,    // monotonic
    ts_ms: i64, // epoch millis
    actor: String,
    action: String,
    payload: serde_json::Value,
    leaf_hash: [u8; 32],                    // BLAKE3(canonical_leaf)
    tree_size: u64,                         // size AFTER insertion
    root_hash: [u8; 32],                    // Merkle root at tree_size
    inclusion_proof: Option<Vec<[u8; 32]>>, // optional cached siblings
    checkpoint_sig: Option<Vec<u8>>,        // optional signature at this state
}

#[derive(Serialize, Deserialize, Clone)]
struct Checkpoint {
    tree_size: u64,
    root_hash: [u8; 32],
    created_at_ms: i64,
    signature: Option<Vec<u8>>, // Ed25519(tree_size || root_hash)
}
```

rs-merkle Integration (minimal sketch):

```rust
use blake3;
use rs_merkle::{Hasher, MerkleProof, MerkleTree};

#[derive(Clone, Debug)]
struct Blake3Hasher;
impl Hasher for Blake3Hasher {
    type Hash = [u8; 32];
    fn hash(data: &[u8]) -> Self::Hash {
        *blake3::hash(data).as_bytes()
    }
}

fn canonical_leaf(actor: &str, action: &str, payload: &serde_json::Value, ts_ms: i64) -> Vec<u8> {
    // Fixed key order for stability; if stronger guarantees needed adopt RFC 8785 canonical JSON.
    serde_json::to_vec(&serde_json::json!({"a": actor, "ac": action, "p": payload, "ts": ts_ms}))
        .unwrap()
}

fn append(
    tree: &mut MerkleTree<Blake3Hasher>,
    leaves: &mut Vec<[u8; 32]>,
    actor: &str,
    action: &str,
    payload: &serde_json::Value,
    ts_ms: i64,
) -> (u64, [u8; 32], Vec<[u8; 32]>) {
    let leaf_bytes = canonical_leaf(actor, action, payload, ts_ms);
    let leaf_hash = Blake3Hasher::hash(&leaf_bytes);
    tree.insert(leaf_hash).commit();
    leaves.push(leaf_hash);
    let size = leaves.len() as u64;
    let root = tree.root().expect("root present after append");
    let proof = tree.proof(&[(size - 1) as usize]);
    (size, root, proof.proof_hashes().to_vec())
}

fn verify(
    root: [u8; 32],
    index: u64,
    total: u64,
    leaf_hash: [u8; 32],
    siblings: &[[u8; 32]],
) -> bool {
    let proof = MerkleProof::<Blake3Hasher>::new(siblings.to_vec());
    proof.verify(root, &[index as usize], &[leaf_hash], total as usize)
}
```

Operational Flow:

- Append Path: canonicalize → hash → insert → commit → record root → (optional) store proof.
- Checkpoint: every N appends or T seconds persist `(tree_size, root_hash[, signature])`.
- Recovery: load max checkpoint, replay subsequent entries, validate root; enter safe mode if mismatch.
- Proof Serving: regenerate on demand if not cached (logarithmic complexity).
- Pruning: logical tombstone retains leaf hash; full physical pruning requires rebuild under maintenance.

Security Invariants:

1. Canonical serialization stability; change requires full tree rebuild + lineage reset.
2. Single monotonic root sequence; no forks outside controlled offline maintenance.
3. Uniform hash algorithm (BLAKE3) per lineage.
4. Signed checkpoints (if enabled) anchor external trust.
5. Inclusion verification inputs: `(root, leaf_index, total_leaves_at_checkpoint, leaf_hash, siblings[])`.

Future Enhancements:

- Domain separation tags for leaf vs internal nodes (e.g., `BLAKE3("SD_LEAF" || data)`).
- Multi-proof batching for range forensic queries.
- RFC 8785 canonical JSON implementation for cross-implementation determinism.
- Streaming proof generation API for bulk verification.

## Error Handling

### Error Architecture

The system implements a layered error handling approach using Rust's type system:

**Library Errors** (using `thiserror`):

```rust
#[derive(Error, Debug)]
pub enum CollectionError {
    #[error("Process enumeration failed: {source}")]
    EnumerationFailed { source: std::io::Error },

    #[error("Hash computation failed for {path}: {source}")]
    HashComputationFailed {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Database write failed: {source}")]
    DatabaseWriteFailed { source: rusqlite::Error },

    #[error("Backpressure policy triggered: {policy}")]
    BackpressureTriggered { policy: String },
}

#[derive(Error, Debug)]
pub enum DetectionError {
    #[error("SQL validation failed: {reason}")]
    SqlValidationFailed { reason: String },

    #[error("Rule execution timeout: {rule_id}")]
    RuleExecutionTimeout { rule_id: String },

    #[error("Alert generation failed: {source}")]
    AlertGenerationFailed {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
```

**Application Errors** (using `anyhow`):

- Context-rich error chains for debugging
- Graceful degradation with detailed logging
- Recovery strategies for transient failures

### Recovery Strategies

**Process Collection**:

- Continue enumeration when individual processes are inaccessible
- Graceful degradation when enhanced privileges are unavailable
- Automatic retry for transient system errors

**Detection Engine**:

- Rule execution timeouts with cleanup
- Circuit breakers for persistently failing rules
- Fallback to essential rules when resources are constrained

**Alert Delivery**:

- Exponential backoff with jitter for failed deliveries
- Circuit breaker pattern for unhealthy sinks
- Dead letter queue for permanently failed alerts

## Testing Strategy

### Testing Architecture

The system implements comprehensive testing across multiple levels:

**Unit Testing**:

- Component isolation with mock dependencies
- Property-based testing with `proptest` for edge cases
- Cryptographic function verification
- SQL query validation and AST parsing

**Integration Testing**:

- Cross-component interaction testing
- Database schema and migration testing
- CLI interface testing with `assert_cmd`
- Platform-specific functionality verification

**Performance Testing**:

- Criterion benchmarks for critical paths
- Load testing with synthetic high-process scenarios
- Memory usage profiling and leak detection
- Database performance under concurrent load

**Security Testing**:

- SQL injection prevention with comprehensive test vectors
- Privilege boundary verification
- Input validation fuzzing with `cargo-fuzz`
- Cryptographic audit chain verification

### Test Data Management

**Synthetic Data Generation**:

```rust
use proptest::prelude::*;

prop_compose! {
    fn arb_process_record()(
        pid in 1u32..65535,
        name in "[a-zA-Z][a-zA-Z0-9_-]{1,15}",
        cpu_usage in prop::option::of(0.0f64..100.0),
        memory_usage in prop::option::of(1024u64..1_073_741_824), // 1KB to 1GB
    ) -> ProcessRecord {
        ProcessRecord {
            id: Uuid::new_v4(),
            pid,
            name,
            cpu_usage,
            memory_usage,
            // ... other fields
        }
    }
}
```

**Snapshot Testing**:

- CLI output validation with `insta`
- Alert format consistency verification
- Configuration file parsing validation

### Continuous Integration

**Test Matrix**:

- Linux (Ubuntu 20.04, 22.04)
- macOS (latest)
- Windows (latest)
- Rust versions: stable, beta, MSRV (1.70+)

**Quality Gates**:

- Zero warnings with `clippy -- -D warnings`
- Code coverage >85% with `llvm-cov`
- Performance regression detection
- Security vulnerability scanning with `cargo audit`

## Security Considerations

### Memory Safety and Unsafe Code Policy

**Zero Unsafe Code Goal**: The entire codebase targets zero unsafe code usage through:

- Rust's ownership system and borrow checker for memory safety
- Careful selection of safe dependencies and crates
- Comprehensive testing to verify memory safety guarantees
- Static analysis tools to detect potential safety issues

**Unsafe Code Isolation** (if absolutely required):

```rust
// Only in procmond, highly structured and isolated
mod unsafe_platform_specific {
    use std::ffi::c_void;

    /// SAFETY: This function is only called with valid process handles
    /// and all invariants are documented and tested
    pub unsafe fn get_process_memory_info(handle: *const c_void) -> Result<MemoryInfo> {
        // Minimal, well-documented unsafe operations
        // Comprehensive safety documentation
        // Extensive testing including edge cases
    }
}

// Safe wrapper that encapsulates all unsafe operations
pub struct SafeProcessMemoryReader {
    // All public APIs are safe
}

impl SafeProcessMemoryReader {
    pub fn get_memory_info(&self, pid: u32) -> Result<MemoryInfo> {
        // Safe interface that internally uses unsafe_platform_specific
        // with all safety invariants maintained
    }
}
```

**Safety Verification**:

- Miri testing for undefined behavior detection
- AddressSanitizer integration where available
- Comprehensive unit tests for all unsafe code paths
- Documentation of all safety invariants and preconditions

### SQL Injection Prevention

**AST Validation**: All user-provided SQL undergoes comprehensive validation:

```rust
pub struct SqlValidator {
    parser: sqlparser::Parser<sqlparser::dialect::SQLiteDialect>,
    allowed_functions: HashSet<String>,
}

impl SqlValidator {
    pub fn validate_query(&self, sql: &str) -> Result<ValidationResult> {
        let ast = self.parser.parse_sql(sql)?;

        for statement in &ast {
            match statement {
                Statement::Query(query) => self.validate_select_query(query)?,
                _ => return Err(ValidationError::ForbiddenStatement),
            }
        }

        Ok(ValidationResult::Valid)
    }

    fn validate_select_query(&self, query: &Query) -> Result<()> {
        // Validate SELECT body, WHERE clauses, functions, etc.
        // Reject any non-whitelisted constructs
    }
}
```

**Execution Sandboxing**:

- Read-only database connections for rule execution
- Query timeouts and memory limits
- Resource cleanup after execution
- Audit logging of all query attempts

### Privilege Management

**Capability-Based Security**:

```rust
pub struct PrivilegeManager {
    initial_privileges: Privileges,
    current_privileges: Privileges,
    drop_completed: bool,
}

impl PrivilegeManager {
    pub async fn request_enhanced_privileges(&mut self) -> Result<()> {
        // Platform-specific privilege escalation
        #[cfg(target_os = "linux")]
        self.request_linux_capabilities()?;

        #[cfg(target_os = "windows")]
        self.request_windows_privileges()?;

        #[cfg(target_os = "macos")]
        self.request_macos_entitlements()?;

        Ok(())
    }

    pub async fn drop_privileges(&mut self) -> Result<()> {
        // Immediate privilege drop after initialization
        self.drop_all_elevated_privileges()?;
        self.drop_completed = true;
        self.audit_privilege_drop().await?;
        Ok(())
    }
}
```

### Cryptographic Integrity

Refer to the earlier **Audit Ledger (redb + rs-merkle, CT-style Merkle log)** section for full data structures and code sketch. This subsection summarizes cryptographic guarantees without duplicating implementation details:

Core Guarantees:

- Append-only sequencing with monotonic `tree_size` mapped to Merkle roots.
- Canonical JSON leaf serialization → BLAKE3 leaf hash → rs-merkle insertion.
- Inclusion proofs (logarithmic sibling path) verifiable offline using `(root, index, total, leaf_hash, siblings[])`.
- Periodic checkpoints (root + tree_size [+ signature]) enable rapid recovery & external attestation.
- Optional Ed25519 signatures provide trust anchor continuity across exports / air‑gapped validation.

Security Invariants:

1. Single linear history (no forks) outside controlled maintenance rebuilds.
2. Uniform hash algorithm per lineage (BLAKE3) with potential future domain separation tags.
3. Checkpoint verification failure forces safe-mode replay / halt before accepting new appends.
4. Proof regeneration does not mutate state; cached proofs are an optimization only.
5. Canonical format change requires full rebuild and lineage version bump.

See also: `database-standards.mdc` for operational procedures (recovery, pruning, checkpoint export) and future enhancements roadmap.
