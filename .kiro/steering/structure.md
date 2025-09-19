# SentinelD Project Structure

## Workspace Organization

SentinelD follows a **three-component security architecture** with strict privilege separation, extensible to multi-tier deployments:

```text
SentinelD/
├── procmond/         # Privileged Process Collector
├── sentinelagent/    # User-Space Orchestrator
├── sentinelcli/      # Command-Line Interface
├── sentinel-lib/     # Shared Library Components
├── security-center/  # Centralized Management (Business/Enterprise)
└── project_spec/     # Project Documentation
```

**Deployment Tiers:**

- **Free Tier**: Standalone agents (procmond + sentinelagent + sentinelcli)
- **Business Tier**: + Security Center + Enterprise integrations
- **Enterprise Tier**: + Kernel monitoring + Federated architecture + Advanced SIEM

## Component Responsibilities

### procmond/ (Privileged Collector)

- **Purpose**: Minimal privileged component for process data collection
- **Security**: Runs with elevated privileges, drops them immediately after init
- **Network**: No network access whatsoever
- **Database**: Write-only access to audit ledger
- **Communication**: IPC server for receiving simple detection tasks from sentinelagent

### sentinelagent/ (Detection Orchestrator)

- **Purpose**: User-space detection rule execution and alert dispatching
- **Security**: Minimal privileges, outbound-only network connections
- **Database**: Read/write access to event store, manages procmond lifecycle
- **Features**: SQL-based detection engine, multi-channel alerting, IPC client
- **Communication**: Translates complex SQL rules into simple protobuf tasks for procmond

### sentinelcli/ (Operator Interface)

- **Purpose**: User-friendly CLI for queries, exports, and configuration
- **Security**: No network access, read-only database operations
- **Features**: JSON/table output, color handling, shell completions

### sentinel-lib/ (Shared Core)

- **Purpose**: Common functionality shared across all components
- **Modules**: config, models, storage, detection, alerting, crypto, telemetry, kernel, network
- **Security**: Trait-based abstractions with security boundaries

### security-center/ (Centralized Management)

- **Purpose**: Centralized aggregation and management for Business/Enterprise tiers
- **Security**: mTLS authentication, certificate management, role-based access
- **Features**: Fleet management, rule distribution, data aggregation, web GUI
- **Deployment**: Optional component for multi-agent environments

## Coding Standards

### Workspace Configuration

- **Edition**: Rust 2024 (MSRV: 1.85+)
- **Resolver**: Version 3 for enhanced dependency resolution
- **Lints**: `unsafe_code = "forbid"`, `warnings = "deny"`
- **Quality**: Zero-warnings policy enforced by CI

### Module Organization

```rust
// Library structure pattern
pub mod alerting; // Multi-channel alert delivery
pub mod config; // Configuration management
pub mod crypto;
pub mod detection; // SQL-based detection engine
pub mod models; // Core data structures
pub mod storage; // Database abstractions // Cryptographic audit functions
```

### Error Handling Pattern

- **Libraries**: Use `thiserror` for structured error types
- **Applications**: Use `anyhow` for error context and chains
- **Recovery**: Graceful degradation with detailed error context
- **Logging**: Structured error events with tracing spans

### Security Boundaries

- **Database Access**: Component-specific access patterns (read-only vs write-only)
- **Network Access**: Strict outbound-only for sentinelagent, none for others
- **Privilege Separation**: Immediate privilege dropping after initialization
- **Input Validation**: Comprehensive validation at all boundaries

## Development Workflow

### Task Runner (justfile)

All development tasks use the `just` command runner:

- `just fmt` - Format code with rustfmt
- `just lint` - Run clippy with strict warnings
- `just test` - Run comprehensive test suite
- `just build` - Build entire workspace

### Testing Architecture

- **Unit Tests**: Component-specific functionality testing
- **Integration Tests**: Cross-component interaction testing
- **CLI Tests**: insta for snapshot testing of command-line interface
- **Performance Tests**: Criterion benchmarks with regression detection

### Configuration Management

Hierarchical configuration with clear precedence:

1. Command-line flags (highest precedence)
2. Environment variables (`SENTINELD_*`)
3. User configuration files (`~/.config/sentineld/`)
4. System configuration files (`/etc/sentineld/`)
5. Embedded defaults (lowest precedence)

## Database Schema Design

### Core Tables

- **processes**: Process snapshots with comprehensive metadata
- **scans**: Collection cycle metadata and statistics
- **detection_rules**: Rule definitions with versioning (rules translated to simple tasks for procmond)
- **alerts**: Generated alerts with execution context
- **alert_deliveries**: Delivery tracking with retry information
- **audit_ledger**: Tamper-evident cryptographic chain

### Business/Enterprise Tables

- **agents**: Agent registration and status tracking
- **agent_connections**: mTLS connection management and certificates
- **fleet_events**: Centralized event aggregation from multiple agents
- **rule_packs**: Curated rule pack management and distribution
- **compliance_mappings**: Compliance framework mappings (NIST, ISO 27001, CIS)
- **network_events**: Network activity correlation (Enterprise tier)
- **kernel_events**: Kernel-level event monitoring (Enterprise tier)

### Access Patterns

- **Event Store**: redb with concurrent access and ACID transactions
- **Audit Ledger**: redb with write-only access for procmond
- **Detection Queries**: Read-only database connections for rule execution
- **Indexing**: Optimized for time-series queries and rule execution
- **Federated Storage**: Hierarchical data aggregation across Security Centers
- **Real-time Events**: Kernel-level event streaming (Enterprise tier)

### IPC Protocol

- **Transport**: Unix domain sockets (Linux/macOS), named pipes (Windows)
- **Format**: Custom protobuf messages for DetectionTask and DetectionResult
- **Security**: Connection authentication and optional encryption
- **Reliability**: Automatic reconnection with exponential backoff

## Security Architecture

### Privilege Separation

- Only procmond runs with elevated privileges when necessary
- Immediate privilege drop after initialization
- Detection and alerting run in user space

### SQL Injection Prevention

- AST validation using sqlparser crate
- Prepared statements and parameterized queries only
- Sandboxed detection rule execution with resource limits
- Query whitelist preventing data modification operations

### Resource Management

- Bounded channels with configurable backpressure policies
- Memory budgets with cooperative yielding
- Timeout enforcement and cancellation support
- Circuit breakers for external dependencies

## File Organization Conventions

### Source Code Structure

```text
src/
├── main.rs          # Binary entrypoint
├── lib.rs           # Library interface (for libs)
├── commands/        # CLI subcommand implementations
├── platform/        # OS-specific implementations
└── [module].rs      # Feature-specific modules
```

### Configuration Files

- **System**: `/etc/sentineld/config.yaml`
- **User**: `~/.config/sentineld/config.yaml`
- **Service**: Platform-specific service definitions in `scripts/service/`

### Documentation Structure

- **Specifications**: `project_spec/` directory with comprehensive docs
- **API Documentation**: Generated from code with `cargo doc`
- **Operator Guide**: User-facing documentation in `docs/`
