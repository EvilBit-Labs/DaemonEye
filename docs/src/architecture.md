# DaemonEye Architecture Overview

## Three-Component Security Architecture

DaemonEye implements a **workspace-based architecture** with multiple crates and binaries using feature flags for precise dependency control. The system follows a three-component security design with strict privilege separation to provide continuous process monitoring and threat detection. The system is designed around the principle of minimal attack surface while maintaining high performance and audit-grade integrity.

The architecture is built on the **collector-core framework**, which provides extensible collection infrastructure for multiple monitoring components while maintaining shared operational foundation. See the [Collector-Core Framework](./architecture/collector-core-framework.md) documentation for detailed information about the underlying collection infrastructure.

```mermaid
graph TB
    subgraph "DaemonEye Three-Component Architecture"
        subgraph "procmond (Privileged Collector)"
            PM[Process Monitor]
            HC[Hash Computer]
            AL[Audit Logger]
            IPC1[IPC Server]
        end

        subgraph "daemoneye-agent (Detection Orchestrator)"
            DE[Detection Engine]
            AM[Alert Manager]
            RM[Rule Manager]
            IPC2[IPC Client]
            NS[Network Sinks]
        end

        subgraph "daemoneye-cli (Operator Interface)"
            QE[Query Executor]
            HC2[Health Checker]
            DM[Data Manager]
        end

        subgraph "daemoneye-lib (Shared Core)"
            CFG[Configuration]
            MOD[Models]
            STO[Storage]
            DET[Detection]
            ALT[Alerting]
            CRY[Crypto]
        end
    end

    subgraph "Data Stores"
        ES[Event Store<br/>redb]
        AL2[Audit Ledger<br/>Certificate Transparency]
    end

    subgraph "External Systems"
        SIEM[SIEM Systems]
        WEBHOOK[Webhooks]
        SYSLOG[Syslog]
    end

    PM --> DE
    HC --> DE
    AL --> AL2
    IPC1 <--> IPC2
    DE --> AM
    AM --> NS
    NS --> SIEM
    NS --> WEBHOOK
    NS --> SYSLOG
    QE --> DE
    HC2 --> DE
    DM --> DE
    DE --> ES
    AL --> AL2
```

## Component Responsibilities

### **procmond (Privileged Process Collector)**

**Primary Purpose**: Minimal privileged component for secure process data collection with purpose-built simplicity.

**Key Responsibilities**:

- **Process Enumeration**: Cross-platform process data collection using sysinfo crate
- **Executable Hashing**: SHA-256 hash computation for integrity verification
- **Audit Logging**: Certificate Transparency-style Merkle tree with inclusion proofs
- **IPC Communication**: Simple protobuf-based communication with daemoneye-agent

**Security Boundaries**:

- Starts with minimal privileges, optionally requests enhanced access
- Drops all elevated privileges immediately after initialization
- No network access whatsoever
- No SQL parsing or complex query logic
- Write-only access to audit ledger
- Simple protobuf IPC only (Unix sockets/named pipes)

**Key Interfaces**:

```rust
#[async_trait]
pub trait ProcessCollector {
    async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>>;
    async fn handle_detection_task(&self, task: DetectionTask) -> Result<DetectionResult>;
    async fn serve_ipc(&self) -> Result<()>;
}
```

### **daemoneye-agent (Detection Orchestrator)**

**Primary Purpose**: User-space detection rule execution, alert management, and procmond lifecycle management.

**Key Responsibilities**:

- **Detection Engine**: SQL-based rule execution with security validation
- **Alert Management**: Alert generation, deduplication, and delivery
- **Rule Management**: Rule loading, validation, and hot-reloading
- **Process Management**: procmond lifecycle management (start, stop, restart, health monitoring)
- **Network Communication**: Outbound-only connections for alert delivery

**Security Boundaries**:

- Operates in user space with minimal privileges
- Manages redb event store (read/write access)
- Translates complex SQL rules into simple protobuf tasks for procmond
- Outbound-only network connections for alert delivery
- Sandboxed rule execution with resource limits
- IPC client for communication with procmond

**Key Interfaces**:

```rust
#[async_trait]
pub trait DetectionEngine {
    async fn execute_rules(&self, scan_id: i64) -> Result<Vec<Alert>>;
    async fn validate_sql(&self, query: &str) -> Result<ValidationResult>;
}

#[async_trait]
pub trait AlertManager {
    async fn generate_alert(&self, detection: DetectionResult) -> Result<Alert>;
    async fn deliver_alert(&self, alert: &Alert) -> Result<DeliveryResult>;
}
```

### **daemoneye-cli (Operator Interface)**

**Primary Purpose**: Command-line interface for queries, management, and diagnostics.

**Key Responsibilities**:

- **Data Queries**: Safe SQL query execution with parameterization
- **System Management**: Configuration, rule management, health monitoring
- **Data Export**: Multiple output formats (JSON, table, CSV)
- **Diagnostics**: System health checks and troubleshooting

**Security Boundaries**:

- No network access
- No direct database access (communicates through daemoneye-agent)
- Input validation for all user-provided data
- Safe SQL execution via daemoneye-agent with prepared statements
- Communicates only with daemoneye-agent for all operations

**Key Interfaces**:

```rust
#[async_trait]
pub trait QueryExecutor {
    async fn execute_query(&self, query: &str, params: &[Value]) -> Result<QueryResult>;
    async fn export_data(&self, format: ExportFormat, filter: &Filter) -> Result<ExportResult>;
}
```

### **daemoneye-lib (Shared Core)**

**Primary Purpose**: Common functionality shared across all components.

**Key Modules**:

- **config**: Hierarchical configuration management
- **models**: Core data structures and serialization
- **storage**: Database abstractions and connection management
- **detection**: SQL validation and rule execution framework
- **alerting**: Multi-channel alert delivery system
- **crypto**: Cryptographic functions for audit chains
- **telemetry**: Observability and metrics collection

## Data Flow Architecture

The system implements a pipeline processing model with clear phases and strict component separation:

### 1. **Collection Phase**

```mermaid
sequenceDiagram
    participant SA as daemoneye-agent
    participant PM as procmond
    participant SYS as System

    SA->>PM: DetectionTask(ENUMERATE_PROCESSES)
    PM->>SYS: Enumerate processes
    SYS-->>PM: Process data
    PM->>PM: Compute hashes
    PM->>PM: Write to audit ledger
    PM-->>SA: DetectionResult(processes)
```

### 2. **Detection Phase**

```mermaid
sequenceDiagram
    participant DE as Detection Engine
    participant DB as Event Store
    participant RM as Rule Manager

    DE->>RM: Load detection rules
    RM-->>DE: SQL rules
    DE->>DB: Execute SQL queries
    DB-->>DE: Query results
    DE->>DE: Generate alerts
    DE->>DB: Store alerts
```

### 3. **Alert Delivery Phase**

```mermaid
sequenceDiagram
    participant AM as Alert Manager
    participant S1 as Sink 1
    participant S2 as Sink 2
    participant S3 as Sink 3

    AM->>S1: Deliver alert (parallel)
    AM->>S2: Deliver alert (parallel)
    AM->>S3: Deliver alert (parallel)
    S1-->>AM: Delivery result
    S2-->>AM: Delivery result
    S3-->>AM: Delivery result
```

## IPC Protocol Design

**Purpose**: Secure, efficient communication between procmond and daemoneye-agent.

**Protocol Specification**:

```protobuf
syntax = "proto3";

// Simple detection tasks sent from daemoneye-agent to procmond
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

// Results sent back from procmond to daemoneye-agent
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

## Database Architecture

### **Event Store (redb)**

- **Purpose**: High-performance process data storage
- **Access**: daemoneye-agent read/write, daemoneye-cli read-only
- **Features**: WAL mode, concurrent access, embedded database
- **Schema**: process_snapshots, scans, detection_rules, alerts

### **Audit Ledger (Certificate Transparency)**

- **Purpose**: Tamper-evident audit trail using Certificate Transparency-style Merkle tree
- **Access**: procmond write-only
- **Features**: Merkle tree with BLAKE3 hashing and Ed25519 signatures
- **Implementation**: rs-merkle for inclusion proofs and periodic checkpoints

## Security Architecture

### **Privilege Separation**

- Only procmond runs with elevated privileges when necessary
- Immediate privilege drop after initialization
- Detection and alerting run in user space
- Component-specific database access patterns

### **SQL Injection Prevention**

- AST validation using sqlparser crate
- Prepared statements and parameterized queries only
- Sandboxed detection rule execution with resource limits
- Query whitelist preventing data modification operations

### **Resource Management**

- Bounded channels with configurable backpressure policies
- Memory budgets with cooperative yielding
- Timeout enforcement and cancellation support
- Circuit breakers for external dependencies

## Performance Characteristics

### **Process Collection**

- **Baseline**: \<5 seconds for 10,000+ processes
- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Hash Computation**: SHA-256 for all accessible executables

### **Detection Engine**

- **Rule Execution**: \<100ms per detection rule
- **SQL Validation**: AST parsing and validation
- **Resource Limits**: 30-second timeout, memory limits
- **Concurrent Execution**: Parallel rule processing

### **Alert Delivery**

- **Multi-Channel**: Parallel delivery to multiple sinks
- **Reliability**: Circuit breakers and retry logic
- **Performance**: Non-blocking delivery with backpressure
- **Monitoring**: Delivery success rates and latency metrics

## Cross-Platform Strategy

### **Process Enumeration**

- **Phase 1**: sysinfo crate for unified cross-platform baseline
- **Phase 2**: Platform-specific enhancements (eBPF, ETW, EndpointSecurity)
- **Fallback**: Graceful degradation when enhanced features unavailable

### **Privilege Management**

- **Linux**: CAP_SYS_PTRACE, immediate capability dropping
- **Windows**: SeDebugPrivilege, token restriction after init
- **macOS**: Minimal entitlements, sandbox compatibility

## Component Communication

### **procmond ↔ daemoneye-agent**

- **Protocol**: Custom Protobuf over Unix Sockets/Named Pipes
- **Direction**: Bidirectional with simple task/result pattern
- **Security**: Process isolation, no network access

### **daemoneye-agent ↔ daemoneye-cli**

- **Protocol**: Local IPC or direct database access
- **Direction**: daemoneye-cli queries daemoneye-agent
- **Security**: Local communication only, input validation

### **External Communication**

- **Alert Delivery**: Outbound-only network connections
- **SIEM Integration**: HTTPS, mTLS, webhook protocols
- **Security Center**: mTLS with certificate authentication

## Error Handling Strategy

### **Graceful Degradation**

- Continue operation when individual components fail
- Fallback mechanisms for enhanced features
- Circuit breakers for external dependencies
- Comprehensive error logging and monitoring

### **Recovery Patterns**

- Automatic retry with exponential backoff
- Health checks and component restart
- Data consistency verification
- Audit trail integrity validation

---

*This architecture provides a robust foundation for implementing DaemonEye's core monitoring functionality while maintaining security, performance, and reliability requirements across all supported platforms.*
