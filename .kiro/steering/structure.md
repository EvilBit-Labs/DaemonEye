# DaemonEye Project Structure

## Workspace Organization

DaemonEye follows a **three-component security architecture** with strict privilege separation:

```text
DaemonEye/
├── procmond/             # Privileged Process Collector
├── daemoneye-agent/      # User-Space Orchestrator
├── daemoneye-cli/        # Command-Line Interface
├── daemoneye-lib/        # Shared Library Components
├── collector-core/       # Collector SDK (EventSource trait, runtime, IPC)
└── daemoneye-eventbus/   # Embedded broker for cross-process pub/sub and RPC
```

> This repository contains the Community tier. Commercial tiers (fleet management, GUI, federation, kernel-level collectors) extend this foundation and are sold separately, not in this repo. See evilbitlabs.io for commercial details.

## Component Responsibilities

### procmond/ (Privileged Collector)

- **Purpose**: Minimal privileged component for process data collection
- **Security**: Runs with elevated privileges, drops them immediately after init
- **Network**: No network access whatsoever
- **Database**: Write-only access to audit ledger
- **Communication**: IPC server for receiving simple detection tasks from daemoneye-agent

### daemoneye-agent/ (Detection Orchestrator)

- **Purpose**: User-space detection rule execution and alert dispatching
- **Security**: Minimal privileges, outbound-only network connections
- **Database**: Read/write access to event store, manages procmond lifecycle
- **Features**: SQL-based detection engine, multi-channel alerting, IPC client
- **Communication**: Translates complex SQL rules into simple protobuf tasks for procmond

### daemoneye-cli/ (Operator Interface)

- **Purpose**: User-friendly CLI for queries, exports, and configuration
- **Security**: No network access, read-only database operations
- **Features**: JSON/table output, color handling, shell completions

### daemoneye-lib/ (Shared Core)

- **Purpose**: Common functionality shared across all components
- **Always-on modules**: config, crypto, integrity, ipc, models, proto, storage, telemetry
- **Feature-gated modules**: alerting (`alerting`), collection (`process-collection`), detection (`detection-engine`), kernel (`kernel-monitoring`), network (`network-correlation`); the kernel and network modules back commercial-tier collectors and are gated off by default in this repo
- **Security**: Trait-based abstractions with security boundaries

### collector-core/ (Collector SDK)

- **Purpose**: SDK providing shared operational infrastructure for collectors (`EventSource` trait, `Collector` runtime, capability negotiation, lifecycle management, IPC contracts)
- **Contract**: Protobuf IPC (`ipc.proto`, `eventbus.proto`) — language-neutral boundary

### daemoneye-eventbus/ (Embedded Broker)

- **Purpose**: Cross-process pub/sub and RPC broker embedded inside daemoneye-agent
- **Features**: Topic hierarchy with wildcard subscriptions, correlation metadata, RPC patterns for collector lifecycle management
- **Transport**: Unix domain sockets (Linux/macOS), named pipes (Windows)

## Coding Standards

### Workspace Configuration

- **Edition**: Rust 2024 (MSRV: 1.85+)
- **Resolver**: Version 3 for enhanced dependency resolution
- **Lints**: `unsafe_code = "forbid"`, `warnings = "deny"`
- **Quality**: Zero-warnings policy enforced by CI
- **AI Restrictions**: Never remove clippy restrictions or allow linters marked as `deny` without explicit permission
- **Commit Message Style**: Always follow the commit message style in #\[[file:.github/commit-instructions.md]\].

### Module Organization

```rust,ignore
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
- **Network Access**: Strict outbound-only for daemoneye-agent, none for others
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
2. Environment variables (`DaemonEye_*`)
3. User configuration files (`~/.config/DaemonEye/`)
4. System configuration files (`/etc/DaemonEye/`)
5. Embedded defaults (lowest precedence)

## Database Schema Design

### Core Tables

- **processes**: Process snapshots with comprehensive metadata
- **scans**: Collection cycle metadata and statistics
- **detection_rules**: Rule definitions with versioning (rules translated to simple tasks for procmond)
- **alerts**: Generated alerts with execution context
- **alert_deliveries**: Delivery tracking with retry information
- **audit_ledger**: Tamper-evident cryptographic chain

### Access Patterns

- **Event Store**: redb with concurrent access and ACID transactions
- **Audit Ledger**: redb with write-only access for procmond
- **Detection Queries**: Read-only database connections for rule execution
- **Indexing**: Optimized for time-series queries and rule execution

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

- **System**: `/etc/DaemonEye/config.yaml`
- **User**: `~/.config/DaemonEye/config.yaml`
- **Service**: Platform-specific service definitions in `scripts/service/`

### Documentation Structure

- **Specifications**: `project_spec/` directory with comprehensive docs
- **API Documentation**: Generated from code with `cargo doc`
- **Operator Guide**: User-facing documentation in `docs/`
