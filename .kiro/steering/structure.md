# SentinelD Project Structure

## Workspace Organization

SentinelD follows a **three-component security architecture** with strict privilege separation:

```text
SentinelD/
├── procmond/         # Privileged Process Collector
├── sentinelagent/    # User-Space Orchestrator  
├── sentinelcli/      # Command-Line Interface
├── sentinel-lib/     # Shared Library Components
└── project_spec/     # Project Documentation
```

## Component Responsibilities

### procmond/ (Privileged Collector)

- **Purpose**: Minimal privileged component for process data collection
- **Security**: Runs with elevated privileges, drops them immediately after init
- **Network**: No network access whatsoever
- **Database**: Write-only access to event store and audit ledger

### sentinelagent/ (Detection Orchestrator)

- **Purpose**: User-space detection rule execution and alert dispatching
- **Security**: Minimal privileges, outbound-only network connections
- **Database**: Read-only access to event store, write access for alerts
- **Features**: SQL-based detection engine, multi-channel alerting

### sentinelcli/ (Operator Interface)

- **Purpose**: User-friendly CLI for queries, exports, and configuration
- **Security**: No network access, read-only database operations
- **Features**: JSON/table output, color handling, shell completions

### sentinel-lib/ (Shared Core)

- **Purpose**: Common functionality shared across all components
- **Modules**: config, models, storage, detection, alerting, crypto, telemetry
- **Security**: Trait-based abstractions with security boundaries

## Coding Standards

### Workspace Configuration

- **Edition**: Rust 2024 (MSRV: 1.70+)
- **Resolver**: Version 3 for enhanced dependency resolution
- **Lints**: `unsafe_code = "forbid"`, `warnings = "deny"`
- **Quality**: Zero-warnings policy enforced by CI

### Module Organization

```rust
// Library structure pattern
pub mod config;      // Configuration management
pub mod models;      // Core data structures  
pub mod storage;     // Database abstractions
pub mod detection;   // SQL-based detection engine
pub mod alerting;    // Multi-channel alert delivery
pub mod crypto;      // Cryptographic audit functions
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
- **CLI Tests**: assert_cmd for command-line interface testing
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

- **process_snapshots**: High-volume process data with performance optimization
- **scan_metadata**: Collection context and statistics
- **detection_rules**: Versioned rule definitions with validation
- **alerts**: Detection results with execution metadata
- **audit_ledger**: Tamper-evident cryptographic chain

### Access Patterns

- **Event Store**: WAL mode with NORMAL sync for performance
- **Audit Ledger**: WAL mode with FULL sync for durability
- **Detection Queries**: Read-only connections with prepared statements
- **Indexing**: Optimized for time-series queries and rule execution

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
