# Design Document

## Overview

DaemonEye implements a three-component security architecture with strict privilege separation to provide continuous process monitoring and threat detection. The system is designed around the principle of minimal attack surface while maintaining high performance and audit-grade integrity.

The core design follows a pipeline architecture where process data flows from collection through detection to alerting, with each component having clearly defined responsibilities and security boundaries.

## Architecture

### Collector-Core Framework Architecture

The collector-core framework provides a unified foundation for multiple collection components, enabling extensible monitoring capabilities while maintaining shared operational infrastructure:

```mermaid
graph LR
    subgraph CC["collector-core Framework"]
        subgraph ES["Event Sources"]
            PS["EventSource<br/>(Process)<br/>• sysinfo<br/>• /proc enum<br/>• Hash compute"]
            NS["EventSource<br/>(Network)<br/>• Netlink/WFP<br/>• Packet cap<br/>• DNS monitor"]
            FS["EventSource<br/>(Filesystem)<br/>• inotify/FSE<br/>• File ops<br/>• Access track"]
            PerfS["EventSource<br/>(Performance)<br/>• /proc/perf<br/>• Resource mon<br/>• System metrics"]
        end

        subgraph CR["Collector Runtime"]
            AGG["Event aggregation and batching"]
            IPC["IPC server (protobuf + CRC32 framing)"]
            CFG["Configuration management and validation"]
            LOG["Structured logging and metrics"]
            HEALTH["Health checks and graceful shutdown"]
            CAP["Capability negotiation"]
        end

        PS --> CR
        NS --> CR
        FS --> CR
        PerfS --> CR
    end

    CC --> SA["daemoneye-agent<br/>(Orchestrator)<br/>• Task dispatch<br/>• Data aggregation<br/>• SQL detection<br/>• Alert management"]


```

**Core Framework Components**:

- **Universal EventSource Trait**: Abstracts collection methodology from operational infrastructure
- **Collector Runtime**: Manages event sources, IPC communication, and shared services
- **Extensible Event Model**: Supports multiple collection domains through unified event types
- **Shared Infrastructure**: Common configuration, logging, health checks, and capability negotiation

**Multi-Component Vision**:

1. **procmond**: Process monitoring using collector-core + process `EventSource`
2. **netmond**: Network monitoring using collector-core + network `EventSource` (future)
3. **fsmond**: Filesystem monitoring using collector-core + filesystem `EventSource` (future)
4. **perfmond**: Performance monitoring using collector-core + performance `EventSource` (future)

### Component Architecture

DaemonEye consists of three main components that work together to provide comprehensive process monitoring, with procmond built on the collector-core framework:

```mermaid
graph LR
    subgraph "DaemonEye Architecture"
        CLI["daemoneye-cli<br/>(Interface)<br/>• User space<br/>• Queries<br/>• Management<br/>• Diagnostics<br/>• Config mgmt"]

        AGENT["daemoneye-agent<br/>(Orchestrator)<br/>• User space<br/>• SQL engine<br/>• Rule mgmt<br/>• Detection<br/>• Alerting<br/>• Network comm<br/>• Task dispatch<br/>• Multi-domain correlation"]

        PROC["procmond<br/>(collector-core + process EventSource)<br/>• Privileged<br/>• Process enum<br/>• Hash compute<br/>• Audit logging<br/>• Protobuf IPC<br/>• Extensible architecture"]

        AUDIT["Audit Ledger<br/>(Merkle Tree)<br/>collector-core managed"]

        STORE["Event Store<br/>(redb)<br/>daemoneye-agent managed"]
    end

    CLI ---|IPC| AGENT
    AGENT ---|IPC| PROC
    PROC --> AUDIT
    AGENT --> STORE


```

**IPC Protocol**: Protobuf + CRC32 framing over interprocess crate (existing implementation) **Communication Flow**: daemoneye-cli ↔ daemoneye-agent ↔ collector-core components Service Management: daemoneye-agent manages collector-core component lifecycle Extensibility: collector-core enables future business/enterprise tier functionality

### Data Flow Architecture

The system implements a pipeline processing model with clear phases and strict component separation:

```mermaid
sequenceDiagram
    participant CLI as daemoneye-cli
    participant AGENT as daemoneye-agent
    participant CORE as collector-core
    participant PROC as ProcessEventSource
    participant AUDIT as Audit Ledger
    participant STORE as Event Store

    Note over CLI,STORE: Detection Rule Execution Pipeline

    CLI->>AGENT: SQL Detection Rule
    AGENT->>AGENT: SQL AST Parsing & Validation
    AGENT->>AGENT: Extract Collection Requirements
    AGENT->>CORE: Simple Protobuf Collection Task

    Note over CORE,PROC: Collection Phase
    CORE->>PROC: Start Collection
    PROC->>PROC: Enumerate Processes
    PROC->>PROC: Compute SHA-256 Hashes
    PROC->>AUDIT: Write Audit Entry (Tamper-Evident)
    PROC->>CORE: Collection Events
    CORE->>AGENT: Aggregated Results

    Note over AGENT,STORE: Detection & Alert Phase
    AGENT->>STORE: Store Process Data
    AGENT->>AGENT: Execute Original SQL Rule
    AGENT->>AGENT: Generate Structured Alerts
    AGENT->>AGENT: Multi-Channel Delivery

    Note over CLI,AGENT: Query & Management
    CLI->>AGENT: Query Request (IPC)
    AGENT->>STORE: Execute Query
    AGENT->>CLI: Formatted Results
```

**Pipeline Phases**:

1. **Collection Phase**: procmond enumerates processes and computes hashes
2. **SQL-to-IPC Translation**: daemoneye-agent uses sqlparser to extract collection requirements from SQL AST
3. **Task Generation**: Complex SQL detection rules translated into simple protobuf collection tasks
4. **IPC Communication**: procmond receives simple detection tasks via protobuf over IPC
5. **Overcollection Strategy**: procmond may overcollect data due to granularity limitations
6. **Audit Logging**: procmond writes to tamper-evident audit ledger (write-only; redb + rs-merkle Merkle tree)
7. **Detection Phase**: daemoneye-agent executes original SQL rules against collected/stored data
8. **Alert Generation**: Structured alerts with deduplication and context
9. **Delivery Phase**: Multi-channel alert delivery with reliability guarantees

**Key Architectural Principles**:

- procmond has minimal complexity and attack surface
- All complex logic (SQL, networking, redb) handled by daemoneye-agent
- IPC protocol is simple and purpose-built for security
- Clear separation between audit logging (procmond) and event processing (daemoneye-agent)
- Pure Rust stack with redb for optimal performance and security
- Zero unsafe code goal with any required unsafe code isolated to procmond only

## Components and Interfaces

### collector-core Framework

**Purpose**: Reusable collection infrastructure that enables multiple monitoring components while maintaining shared operational foundation

**Reusable Components from Existing Implementation**:

The collector-core framework will wrap and extend existing proven components:

- **IPC Infrastructure**: Complete interprocess crate integration (docs/src/technical/ipc-implementation.md)

  - `InterprocessServer` and `InterprocessClient` from daemoneye-lib/src/ipc/
  - `IpcCodec` with CRC32 validation and frame protocol
  - `IpcConfig` with comprehensive timeout and security settings
  - Cross-platform transport layer (Unix sockets, Windows named pipes)

- **Process Collection Logic**: Existing ProcessMessageHandler (procmond/src/lib.rs)

  - `enumerate_processes()` using sysinfo crate
  - `convert_process_to_record()` with comprehensive metadata
  - `handle_detection_task()` with task routing and error handling
  - Support for process filtering and hash verification

- **Database Integration**: Existing storage layer (daemoneye-lib/src/storage.rs)

  - `DatabaseManager` with redb backend
  - Table definitions and transaction handling
  - Serialization and error handling

- **Configuration Management**: Existing config system (daemoneye-lib/src/config.rs)

  - `ConfigLoader` with hierarchical overrides
  - Environment variable and file-based configuration
  - Validation and error handling

- **Telemetry and Logging**: Existing observability (daemoneye-lib/src/telemetry.rs)

  - `TelemetryCollector` for performance monitoring
  - Structured logging with tracing crate
  - Health check and metrics collection

**Key Interfaces**:

- `EventSource` trait for pluggable collection implementations
- `Collector` struct for runtime management and event aggregation
- `CollectionEvent` enum for unified event handling across domains
- `CollectorConfig` for shared configuration management

**Core Implementation**:

```rust
#[async_trait]
pub trait EventSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> SourceCaps;
    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()>;
    async fn stop(&self) -> anyhow::Result<()>;
}

pub struct Collector {
    config: CollectorConfig,
    sources: Vec<Box<dyn EventSource>>,
    runtime: CollectorRuntime,
    ipc_server: IpcServer,
}

impl Collector {
    pub fn new(config: CollectorConfig) -> Self { ... }
    pub fn register<S: EventSource + 'static>(&mut self, source: S) { ... }
    pub async fn run(self) -> anyhow::Result<()> { ... }
}

#[derive(Debug, Clone)]
pub enum CollectionEvent {
    Process(ProcessEvent),
    Network(NetworkEvent),      // Future: netmond
    Filesystem(FilesystemEvent), // Future: fsmond
    Performance(PerformanceEvent), // Future: perfmond
}

bitflags! {
    pub struct SourceCaps: u32 {
        const PROCESS = 1 << 0;
        const NETWORK = 1 << 1;
        const FILESYSTEM = 1 << 2;
        const PERFORMANCE = 1 << 3;
        const REALTIME = 1 << 4;
        const KERNEL_LEVEL = 1 << 5;
        const SYSTEM_WIDE = 1 << 6;
    }
}
```

**Shared Infrastructure**:

- **Configuration Management**: Hierarchical config loading with validation (daemoneye-lib/src/config.rs)
- **Logging Infrastructure**: Structured tracing with JSON output and metrics (daemoneye-lib/src/telemetry.rs)
- **IPC Server**: Protobuf-based communication with daemoneye-agent (daemoneye-lib/src/ipc/)
- **Health Monitoring**: Component status tracking and graceful shutdown
- **Event Batching**: Efficient event aggregation and backpressure handling
- **Capability Negotiation**: Dynamic feature discovery and task routing

**Integration with Existing Components**:

```rust
// Refactored procmond using collector-core + existing components
fn main() -> anyhow::Result<()> {
    // EXISTING: Reuse CLI parsing and initialization
    let cli = Cli::parse();
    tracing_subscriber::fmt::init();

    // EXISTING: Reuse configuration loading
    let config_loader = config::ConfigLoader::new("procmond");
    let config = config_loader.load()?;

    // EXISTING: Reuse database initialization
    let db_manager = Arc::new(Mutex::new(storage::DatabaseManager::new(&cli.database)?));

    // NEW: Create collector-core with existing components
    let collector_config = CollectorConfig::from(config);
    let mut collector = collector_core::Collector::new(collector_config);

    // NEW: Register process source wrapping existing ProcessMessageHandler
    let process_handler = ProcessMessageHandler::new(Arc::clone(&db_manager));
    let process_source = ProcessEventSource::new(process_handler, config.app.clone());
    collector.register(process_source);

    // EXISTING: Reuse IPC server creation with collector-core integration
    collector.run().await
}

// ProcessEventSource wraps existing ProcessMessageHandler
pub struct ProcessEventSource {
    handler: ProcessMessageHandler, // EXISTING: Complete process logic
    config: AppConfig,              // EXISTING: Configuration structure
}

#[async_trait]
impl EventSource for ProcessEventSource {
    fn name(&self) -> &'static str {
        "process-monitor"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        // EXISTING: Reuse ProcessMessageHandler.enumerate_processes logic
        loop {
            let task = DetectionTask {
                task_id: format!("scan-{}", chrono::Utc::now().timestamp_millis()),
                task_type: ProtoTaskType::EnumerateProcesses as i32,
                process_filter: None,
                hash_check: None,
                metadata: None,
            };

            let result = self.handler.enumerate_processes(&task).await?;

            for process in result.processes {
                let event = CollectionEvent::Process(ProcessEvent::from(process));
                tx.send(event).await?;
            }

            tokio::time::sleep(Duration::from_millis(self.config.scan_interval_ms)).await;
        }
    }
}
```

**Multi-Component Support**:

```rust
// Example: Future network monitoring component
fn main() -> anyhow::Result<()> {
    let config = collector_core::config::load()?;
    let mut collector = collector_core::Collector::new(config);

    // Register network collection sources
    collector.register(NetworkEventSource::new()?);

    #[cfg(target_os = "linux")]
    collector.register(NetlinkSource::new()?);

    collector.run().await
}
```

### procmond (Process Collection Component)

**Purpose**: Process monitoring implementation using collector-core framework with process-specific EventSource

**Key Interfaces**:

- `ProcessEventSource` implementing the EventSource trait
- **EXISTING**: `ProcessMessageHandler` for process enumeration and task handling (from procmond/src/lib.rs)
- **EXISTING**: `IpcConfig` and IPC server creation (from procmond/src/ipc/mod.rs)
- **EXISTING**: Database integration via `storage::DatabaseManager` (from daemoneye-lib)
- Integration with collector-core runtime and existing IPC infrastructure

**Core Implementation**:

```rust
pub struct ProcessEventSource {
    collector: Box<dyn ProcessCollector>,
    hash_computer: Box<dyn HashComputer>,
    config: ProcessConfig,
}

#[async_trait]
impl EventSource for ProcessEventSource {
    fn name(&self) -> &'static str {
        "process-monitor"
    }

    fn capabilities(&self) -> SourceCaps {
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        // Process enumeration and event generation
        loop {
            let processes = self.collector.enumerate_processes().await?;
            for process in processes {
                let event = CollectionEvent::Process(ProcessEvent::from(process));
                tx.send(event).await?;
            }
            tokio::time::sleep(self.config.scan_interval).await;
        }
    }
}

// Simplified procmond main using collector-core
fn main() -> anyhow::Result<()> {
    let config = collector_core::config::load()?;
    let mut collector = collector_core::Collector::new(config);

    // Register process monitoring source
    collector.register(ProcessEventSource::new()?);

    collector.run().await
}
```

**Security Boundaries**:

- Starts with minimal privileges, optionally requests enhanced access
- Drops all elevated privileges immediately after initialization
- No network access whatsoever
- No SQL parsing or complex query logic
- Write-only access to audit ledger (managed by collector-core)
- Cross-platform IPC via interprocess crate (managed by collector-core)
- Purpose-built for stability and minimal attack surface
- Zero unsafe code goal; any required unsafe code isolated to highly structured, tested modules
- Leverages collector-core framework for operational infrastructure

### daemoneye-agent (Detection Orchestrator)

**Purpose**: User-space detection rule execution, alert management, and collector-core component lifecycle management

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
- **Task Generation**: Translates complex SQL queries into simple protobuf collection tasks for collector-core components
- **Multi-Domain Correlation**: Aggregates events from multiple collection domains (process, network, filesystem, performance)
- **Overcollection Handling**: May request broader data collection than SQL requires, then applies SQL filtering to stored data
- **Privilege Separation**: SQL execution never directly touches live processes; only simple collection tasks sent via IPC
- Outbound-only network connections for alert delivery
- Sandboxed rule execution with resource limits
- IPC client for communication with collector-core components via interprocess crate
- Manages collector-core component lifecycle (start, stop, restart, health monitoring)
- **Capability Negotiation**: Discovers available monitoring capabilities from collector-core components

**Service Management**:

- Primary service registered with system service manager (systemd, launchd, Windows Service)
- Responsible for starting and monitoring procmond as a child process
- Handles graceful shutdown coordination between components
- Manages service dependencies and startup ordering
- Provides unified logging and health reporting for the entire system

### daemoneye-cli (Operator Interface)

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
- No direct database access (communicates through daemoneye-agent)
- Input validation for all user-provided data
- Safe SQL execution via daemoneye-agent with prepared statements
- Communicates only with daemoneye-agent for all operations

### IPC Protocol Design

**Purpose**: Secure, efficient communication between collector-core components and daemoneye-agent

**Protocol Specification** (extending existing protobuf definitions):

```protobuf
syntax = "proto3";

// NEW: Capability negotiation between collector-core and daemoneye-agent
message CollectionCapabilities {
    bool supports_processes = 1;
    bool supports_network = 2;      // Future: netmond
    bool supports_filesystem = 3;   // Future: fsmond
    bool supports_performance = 4;  // Future: perfmond
    bool kernel_level = 5;
    bool realtime = 6;
    bool system_wide = 7;
}

// EXISTING: Detection tasks (from ipc.proto) - extend for future components
message DetectionTask {
    string task_id = 1;
    TaskType task_type = 2;
    optional ProcessFilter process_filter = 3;
    optional HashCheck hash_check = 4;
    optional string metadata = 5;
    // NEW: Future extensions for additional collection domains
    optional NetworkFilter network_filter = 6;    // Future: netmond
    optional FilesystemFilter fs_filter = 7;      // Future: fsmond
    optional PerformanceFilter perf_filter = 8;   // Future: perfmond
}

// EXISTING: Task types (from common.proto) - extend for future components
enum TaskType {
    ENUMERATE_PROCESSES = 0;
    CHECK_PROCESS_HASH = 1;
    MONITOR_PROCESS_TREE = 2;
    VERIFY_EXECUTABLE = 3;
    // NEW: Future task types
    MONITOR_NETWORK_CONNECTIONS = 4;    // Future: netmond
    TRACK_FILE_OPERATIONS = 5;          // Future: fsmond
    COLLECT_PERFORMANCE_METRICS = 6;    // Future: perfmond
}

// EXISTING: Process filtering (from common.proto)
message ProcessFilter {
    repeated string process_names = 1;
    repeated uint32 pids = 2;
    optional string executable_pattern = 3;
}

// EXISTING: Hash verification (from common.proto)
message HashCheck {
    string expected_hash = 1;
    string hash_algorithm = 2;
    string executable_path = 3;
}

// EXISTING: Detection results (from ipc.proto) - extend for future components
message DetectionResult {
    string task_id = 1;
    bool success = 2;
    optional string error_message = 3;
    repeated ProcessRecord processes = 4;
    optional HashResult hash_result = 5;
    // NEW: Future extensions for additional collection domains
    repeated NetworkRecord network_events = 6;    // Future: netmond
    repeated FilesystemRecord fs_events = 7;      // Future: fsmond
    repeated PerformanceRecord perf_events = 8;   // Future: perfmond
}

// EXISTING: Process record (from common.proto)
message ProcessRecord {
    uint32 pid = 1;
    optional uint32 ppid = 2;
    string name = 3;
    optional string executable_path = 4;
    repeated string command_line = 5;
    optional int64 start_time = 6;
    optional double cpu_usage = 7;
    optional uint64 memory_usage = 8;
    optional string executable_hash = 9;
    optional string hash_algorithm = 10;
    optional string user_id = 11;
    bool accessible = 12;
    bool file_exists = 13;
    int64 collection_time = 14;
}
```

**Transport Layer (Existing Interprocess Implementation)**:

- **Implementation**: **EXISTING** `interprocess` crate integration from docs/src/technical/ipc-implementation.md
- **Unix/Linux/macOS**: Unix domain sockets with owner-only permissions (0700 dir, 0600 socket)
- **Windows**: Named pipes with appropriate security descriptors
- **Frame Protocol**: **EXISTING** Length-delimited protobuf messages with CRC32 integrity validation
- **Codec**: **EXISTING** `IpcCodec` with BytesMut buffers, timeout handling, and comprehensive error types
- **Security**: **EXISTING** No network access, local-only endpoints, connection limits
- **Reliability**: **EXISTING** Async message handling, graceful shutdown, and proper cleanup
- **Configuration**: **EXISTING** `IpcConfig` with timeouts, limits, and CRC32 variant selection
- **Error Handling**: **EXISTING** Comprehensive `IpcError` types with automatic reconnection and exponential backoff

### IPC Protocol Flow

```mermaid
sequenceDiagram
    participant SA as daemoneye-agent
    participant PM as procmond
    participant C as Codec
    participant UDS as Unix Socket

    Note over SA,UDS: Connection Establishment
    SA->>UDS: Connect to socket
    UDS-->>SA: Connection accepted

    Note over SA,UDS: Message Exchange
    SA->>C: Encode DetectionTask
    C->>C: Add length + CRC32
    C->>UDS: Write frame
    UDS->>PM: Frame received
    PM->>C: Read and validate frame
    C->>C: Verify CRC32
    C-->>PM: DetectionTask

    PM->>PM: Process task

    PM->>C: Encode DetectionResult
    C->>C: Add length + CRC32
    C->>UDS: Write frame
    UDS->>SA: Frame received
    SA->>C: Read and validate frame
    C->>C: Verify CRC32
    C-->>SA: DetectionResult

    Note over SA,UDS: Connection Cleanup
    SA->>UDS: Close connection
```

### Error Handling Flow

```mermaid
flowchart TD
    A["Incoming Frame"] --> B{"Valid Length?"}
    B -->|No| C["IpcError::InvalidLength"]
    B -->|Yes| D{"Within Size Limit?"}
    D -->|No| E["IpcError::TooLarge"]
    D -->|Yes| F["Read Message Bytes"]
    F --> G{"CRC32 Valid?"}
    G -->|No| H["IpcError::CrcMismatch"]
    G -->|Yes| I["Decode Protobuf"]
    I --> J{"Decode Success?"}
    J -->|No| K["IpcError::Decode"]
    J -->|Yes| L["Process Message"]

    C --> M["Send Error Response"]
    E --> M
    H --> M
    K --> M
    L --> N["Send Success Response"]
```

## Data Models

### Core Data Structures

**ProcessRecord**: Represents a single process snapshot (existing implementation in daemoneye-lib/src/models/process.rs)

```rust
// Existing ProcessRecord structure (from protobuf and Rust models)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub executable_path: Option<String>,
    pub command_line: Vec<String>,
    pub start_time: Option<i64>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
    pub executable_hash: Option<String>,
    pub hash_algorithm: Option<String>,
    pub user_id: Option<String>,
    pub accessible: bool,
    pub file_exists: bool,
    pub collection_time: i64, // Unix timestamp in milliseconds
    // Additional fields for collector-core integration
    pub id: Uuid,     // NEW: Unique record identifier
    pub scan_id: i64, // NEW: Collection cycle identifier
}
```

**Alert**: Represents a detection result with full context (existing implementation in daemoneye-lib/src/models/alert.rs)

```rust
// Existing Alert structure (from daemoneye-lib models)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: AlertId, // Existing: Uuid wrapper
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

// Existing AlertSeverity enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}
```

**DetectionRule**: SQL-based detection rule with metadata (existing implementation in daemoneye-lib/src/models/rule.rs)

```rust
// Existing DetectionRule structure (from daemoneye-lib models)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionRule {
    pub id: RuleId, // Existing: String wrapper
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

// Existing RuleSourceType enum
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
- CLI interface testing with `insta` for snapshot testing
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

## daemoneye-cli Command-Line Interface Design

### Overview

daemoneye-cli provides a comprehensive command-line interface for operators to query data, manage detection rules, monitor system health, and perform administrative tasks. It communicates exclusively with daemoneye-agent via IPC, maintaining the security boundary where CLI never directly accesses the database.

### Command Structure

```bash
daemoneye-cli [GLOBAL_OPTIONS] <COMMAND> [COMMAND_OPTIONS] [ARGS]
```

### Global Options

```bash
--config <PATH>          Configuration file path (default: ~/.config/DaemonEye/cli.yaml)
--output <FORMAT>        Output format: json, table, csv (default: table)
--no-color              Disable colored output
--verbose               Enable verbose logging
--help                  Show help information
--version               Show version information
```

### Core Commands

#### 1. Query Commands (`query`)

Execute SQL queries against historical process data:

```bash
# Interactive query mode
daemoneye-cli query

# Execute single query
daemoneye-cli query --sql "SELECT pid, name, executable_path FROM processes WHERE name LIKE '%suspicious%'"

# Query with parameters
daemoneye-cli query --sql "SELECT * FROM processes WHERE start_time > ?" --param "2024-01-01T00:00:00Z"

# Export query results
daemoneye-cli query --sql "SELECT * FROM processes" --output csv --file processes.csv

# Streaming for large results
daemoneye-cli query --sql "SELECT * FROM processes" --stream --limit 1000

# Query with pagination
daemoneye-cli query --sql "SELECT * FROM processes ORDER BY start_time DESC" --page 1 --page-size 50
```

**Subcommands:**

- `daemoneye-cli query interactive` - Start interactive SQL shell
- `daemoneye-cli query history` - Show query history
- `daemoneye-cli query explain <SQL>` - Show query execution plan
- `daemoneye-cli query validate <SQL>` - Validate SQL syntax without execution

#### 2. Rule Management (`rules`)

Manage detection rules and rule packs:

```bash
# List all rules
daemoneye-cli rules list

# Show rule details
daemoneye-cli rules show <rule-id>

# Validate rule syntax
daemoneye-cli rules validate <rule-file>

# Test rule against historical data
daemoneye-cli rules test <rule-id> --since "1 hour ago"

# Enable/disable rules
daemoneye-cli rules enable <rule-id>
daemoneye-cli rules disable <rule-id>

# Import/export rules
daemoneye-cli rules import <rule-pack.yaml>
daemoneye-cli rules export --output rules-backup.yaml

# Rule statistics
daemoneye-cli rules stats <rule-id>
daemoneye-cli rules stats --all
```

**Subcommands:**

- `daemoneye-cli rules create` - Create new rule interactively
- `daemoneye-cli rules edit <rule-id>` - Edit existing rule
- `daemoneye-cli rules delete <rule-id>` - Delete rule
- `daemoneye-cli rules pack list` - List available rule packs
- `daemoneye-cli rules pack install <pack-name>` - Install rule pack

#### 3. Alert Management (`alerts`)

View and manage alerts:

```bash
# List recent alerts
daemoneye-cli alerts list

# Show alert details
daemoneye-cli alerts show <alert-id>

# Filter alerts
daemoneye-cli alerts list --severity high --since "24 hours ago"
daemoneye-cli alerts list --rule-id suspicious-process --status open

# Alert statistics
daemoneye-cli alerts stats --by-severity
daemoneye-cli alerts stats --by-rule --since "7 days ago"

# Export alerts
daemoneye-cli alerts export --format json --since "1 week ago" --file alerts.json
```

**Subcommands:**

- `daemoneye-cli alerts acknowledge <alert-id>` - Acknowledge alert
- `daemoneye-cli alerts close <alert-id>` - Close alert
- `daemoneye-cli alerts reopen <alert-id>` - Reopen closed alert

#### 4. System Health (`health`)

Monitor system health and diagnostics:

```bash
# Overall system status
daemoneye-cli health

# Component-specific status
daemoneye-cli health --component procmond
daemoneye-cli health --component daemoneye-agent
daemoneye-cli health --component database

# Performance metrics
daemoneye-cli health metrics

# Configuration validation
daemoneye-cli health config

# Connection testing
daemoneye-cli health connectivity
```

**Subcommands:**

- `daemoneye-cli health logs` - Show recent system logs
- `daemoneye-cli health diagnostics` - Run comprehensive diagnostics
- `daemoneye-cli health repair` - Attempt automatic issue resolution

#### 5. Data Management (`data`)

Data export, import, and maintenance:

```bash
# Export process data
daemoneye-cli data export processes --since "7 days ago" --format json

# Export audit logs
daemoneye-cli data export audit --format csv --file audit.csv

# Database statistics
daemoneye-cli data stats

# Database maintenance
daemoneye-cli data vacuum
daemoneye-cli data integrity-check

# Data retention management
daemoneye-cli data cleanup --older-than "30 days"
```

#### 6. Configuration (`config`)

Configuration management:

```bash
# Show current configuration
daemoneye-cli config show

# Validate configuration
daemoneye-cli config validate

# Set configuration values
daemoneye-cli config set alert.email.smtp_server smtp.example.com
daemoneye-cli config set detection.scan_interval 30s

# Reset configuration
daemoneye-cli config reset
```

#### 7. Service Management (`service`)

Service lifecycle management:

```bash
# Service status
daemoneye-cli service status

# Start/stop services
daemoneye-cli service start
daemoneye-cli service stop
daemoneye-cli service restart

# Service logs
daemoneye-cli service logs --follow
daemoneye-cli service logs --component procmond --lines 100
```

### Output Formats

#### Table Format (Default)

```structured text
┌──────┬─────────────────┬──────────────────────────────────┬─────────────────────┐
│ PID  │ Name            │ Executable Path                  │ Start Time          │
├──────┼─────────────────┼──────────────────────────────────┼─────────────────────┤
│ 1234 │ suspicious.exe  │ /tmp/malware/suspicious.exe      │ 2024-01-15 14:30:25 │
│ 5678 │ normal.exe      │ /usr/bin/normal.exe              │ 2024-01-15 14:25:10 │
└──────┴─────────────────┴──────────────────────────────────┴─────────────────────┘
```

#### JSON Format

```json
{
  "query": "SELECT pid, name, executable_path, start_time FROM processes LIMIT 2",
  "execution_time_ms": 45,
  "row_count": 2,
  "results": [
    {
      "pid": 1234,
      "name": "suspicious.exe",
      "executable_path": "/tmp/malware/suspicious.exe",
      "start_time": "2024-01-15T14:30:25Z"
    },
    {
      "pid": 5678,
      "name": "normal.exe",
      "executable_path": "/usr/bin/normal.exe",
      "start_time": "2024-01-15T14:25:10Z"
    }
  ]
}
```

#### CSV Format

```csv
pid,name,executable_path,start_time
1234,suspicious.exe,/tmp/malware/suspicious.exe,2024-01-15T14:30:25Z
5678,normal.exe,/usr/bin/normal.exe,2024-01-15T14:25:10Z
```

### Interactive Features

#### Interactive Query Shell

```bash
daemoneye-cli query interactive
DaemonEye Query Shell v1.0.0
Type 'help' for commands, 'exit' to quit.

daemoneye> SELECT COUNT(*) FROM processes WHERE name LIKE '%chrome%';
┌─────────┐
│ count   │
├─────────┤
│ 12      │
└─────────┘
Executed in 23ms

daemoneye> .schema processes
CREATE TABLE processes (
  id INTEGER PRIMARY KEY,
  pid INTEGER NOT NULL,
  ppid INTEGER,
  name TEXT NOT NULL,
  executable_path TEXT,
  command_line TEXT,
  start_time TIMESTAMP,
  hash_sha256 TEXT,
  scan_id INTEGER,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

daemoneye> .help
Available commands:
  .schema [table]     Show table schema
  .tables             List all tables
  .history            Show query history
  .export <format>    Export last result
  .clear              Clear screen
  .exit               Exit shell
```

### Error Handling and User Experience

#### Validation Errors

```bash
$ daemoneye-cli query --sql "DROP TABLE processes;"
Error: Invalid SQL query
  ├─ Reason: DROP statements are not allowed
  ├─ Allowed: SELECT statements only
  └─ Help: Use SELECT queries to retrieve data safely

$ daemoneye-cli rules validate malformed-rule.yaml
Error: Rule validation failed
  ├─ File: malformed-rule.yaml:15:3
  ├─ Issue: Invalid SQL syntax in detection query
  ├─ Query: "SELECT * FROM processes WHERE pid = ?"
  └─ Fix: Add proper WHERE clause parameters
```

#### Connection Errors

```bash
$ daemoneye-cli health
Error: Cannot connect to daemoneye-agent
  ├─ IPC Socket: /var/run/DaemonEye/agent.sock
  ├─ Status: Connection refused
  ├─ Suggestion: Check if daemoneye-agent service is running
  └─ Command: sudo systemctl status daemoneye-agent
```

### Configuration File

CLI configuration in `~/.config/DaemonEye/cli.yaml`:

```yaml
# daemoneye-cli Configuration
connection:
  ipc_socket: /var/run/DaemonEye/agent.sock
  timeout: 30s
  retry_attempts: 3

output:
  default_format: table
  color: true
  pager: less
  max_rows_without_paging: 100

query:
  history_size: 1000
  auto_complete: true
  syntax_highlighting: true

aliases:
  recent: SELECT * FROM processes WHERE start_time > datetime('now', '-1 hour') 
    ORDER BY start_time DESC
  suspicious: SELECT * FROM processes WHERE name NOT IN (SELECT DISTINCT name 
    FROM processes GROUP BY name HAVING COUNT(*) > 10)
```

### Shell Integration

#### Bash Completion

```bash
# Enable bash completion
source <(daemoneye-cli completion bash)

# Tab completion examples
daemoneye-cli ru<TAB>     # completes to "rules"
daemoneye-cli rules <TAB> # shows: list, show, validate, test, enable, disable, import, export, stats
```

#### Shell Aliases

```bash
# Common aliases for .bashrc
alias sctl='daemoneye-cli'
alias sq='daemoneye-cli query'
alias sr='daemoneye-cli rules'
alias sh='daemoneye-cli health'
```

This comprehensive CLI design provides operators with powerful, intuitive tools for managing DaemonEye while maintaining security boundaries and following Unix CLI conventions.

## Testing Strategy and Platform Support

### OS Support Matrix

DaemonEye follows a tiered support model aligned with enterprise deployment requirements:

| OS          | Version              | Architecture  | Status    | CI Testing | Notes                      |
| ----------- | -------------------- | ------------- | --------- | ---------- | -------------------------- |
| **Linux**   | Ubuntu 20.04+ LTS    | x86_64, ARM64 | Primary   | ✅ Full    | Full feature support       |
| **Linux**   | RHEL/CentOS 8+       | x86_64, ARM64 | Primary   | ✅ Full    | Full feature support       |
| **Linux**   | Alma/Rocky Linux 8+  | x86_64, ARM64 | Primary   | ✅ Full    | Full feature support       |
| **Linux**   | Debian 11+ LTS       | x86_64, ARM64 | Primary   | ✅ Full    | Full feature support       |
| **macOS**   | 14.0+ (Sonoma)       | x86_64, ARM64 | Primary   | ✅ Full    | Native process monitoring  |
| **Windows** | Windows 10+          | x86_64, ARM64 | Primary   | ✅ Full    | Service deployment         |
| **Windows** | Windows Server 2019+ | x86_64        | Primary   | ✅ Full    | Enterprise features        |
| **Windows** | Windows Server 2022  | x86_64, ARM64 | Primary   | ✅ Full    | Enterprise standard        |
| **Windows** | Windows 11           | x86_64, ARM64 | Primary   | ✅ Full    | Full feature support       |
| **Linux**   | Alpine 3.16+         | x86_64, ARM64 | Secondary | ⚠️ Limited | Container deployments      |
| **Linux**   | Amazon Linux 2+      | x86_64, ARM64 | Secondary | ⚠️ Limited | Cloud deployments          |
| **Linux**   | Ubuntu 18.04         | x86_64, ARM64 | Secondary | ⚠️ Limited | Best-effort support        |
| **Linux**   | RHEL 7               | x86_64        | Secondary | ⚠️ Limited | Best-effort support        |
| **macOS**   | 12.0+ (Monterey)     | x86_64, ARM64 | Secondary | ⚠️ Limited | Best-effort support        |
| **FreeBSD** | 13.0+                | x86_64, ARM64 | Secondary | ⚠️ Limited | pfSense/OPNsense ecosystem |

### CI/CD Testing Matrix

```mermaid
graph TB
    subgraph "Primary Platform Testing (Full CI)"
        P1["Ubuntu 20.04+ LTS<br/>x86_64, ARM64"]
        P2["RHEL/CentOS 8+<br/>x86_64, ARM64"]
        P3["Debian 11+ LTS<br/>x86_64, ARM64"]
        P4["macOS 14.0+ (Sonoma)<br/>x86_64, ARM64"]
        P5["Windows 10+/11<br/>x86_64, ARM64"]
        P6["Windows Server 2019+/2022<br/>x86_64, ARM64"]
    end

    subgraph "Secondary Platform Testing (Limited CI)"
        S1["Alpine 3.16+<br/>Container Testing"]
        S2["Amazon Linux 2+<br/>Cloud Testing"]
        S3["Ubuntu 18.04<br/>Legacy Support"]
        S4["macOS 12.0+ (Monterey)<br/>Best Effort"]
        S5["FreeBSD 13.0+<br/>Network Appliance"]
    end

    subgraph "Rust Version Matrix"
        R1["Stable (Latest)"]
        R2["Beta"]
        R3["MSRV (1.70+)"]
    end

    subgraph "Quality Gates"
        Q1["cargo clippy -- -D warnings"]
        Q2["cargo fmt --all --check"]
        Q3["cargo audit"]
        Q4["cargo deny check"]
        Q5["overflow-checks = true"]
        Q6["llvm-cov coverage >85%"]
    end

    P1 --> R1
    P1 --> R2
    P1 --> R3
    P2 --> R1
    P3 --> R1
    P4 --> R1
    P5 --> R1
    P6 --> R1

    S1 --> R1
    S2 --> R1

    R1 --> Q1
    R1 --> Q2
    R1 --> Q3
    R1 --> Q4
    R1 --> Q5
    R1 --> Q6

    style P1 fill:#e8f5e8
    style P2 fill:#e8f5e8
    style P3 fill:#e8f5e8
    style P4 fill:#e8f5e8
    style P5 fill:#e8f5e8
    style P6 fill:#e8f5e8
    style S1 fill:#fff3e0
    style S2 fill:#fff3e0
    style S3 fill:#fff3e0
    style S4 fill:#fff3e0
    style S5 fill:#fff3e0
```

### Testing Policy

**Primary Platforms**: Full CI/CD pipeline with comprehensive testing across all Rust versions (stable, beta, MSRV) and both x86_64 and ARM64 architectures.

**Secondary Platforms**: Limited CI testing with stable Rust only, focusing on compilation and basic functionality validation.

**Quality Gates**: All platforms must pass security auditing, formatting, linting, and maintain >85% code coverage.

**Cross-Compilation**: ARM64 targets are cross-compiled and tested where native runners are not available.

**Container Testing**: Alpine and Amazon Linux are tested in containerized environments to validate deployment scenarios.

This comprehensive testing strategy ensures DaemonEye works reliably across enterprise environments while maintaining security and performance standards.
