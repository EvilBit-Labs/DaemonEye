# SentinelD Technical Stack

## Language & Runtime

- **Language**: Rust 2024 Edition (MSRV: 1.70+)
- **Safety**: `unsafe_code = "forbid"` at workspace level
- **Quality**: `warnings = "deny"` with zero-warnings policy
- **Async Runtime**: Tokio with full feature set for I/O and task management

## Core Dependencies

### Database Layer

- **SQLite**: rusqlite with bundled SQLite, WAL mode for performance
- **Features**: unlock_notify, hooks, column_decltype for enhanced functionality
- **Configuration**: Separate event store and audit ledger with different durability settings

### CLI Framework

- **clap v4**: Derive macros with shell completions (bash, zsh, fish, PowerShell)
- **Terminal**: Automatic color detection, NO_COLOR and TERM=dumb support

### Configuration Management

- **Hierarchical loading**: Embedded defaults → System files → User files → Environment → CLI flags
- **Formats**: YAML, JSON, TOML support via figment and config crates
- **Validation**: Comprehensive validation with detailed error messages

### Error Handling

- **Libraries**: thiserror for structured error types
- **Applications**: anyhow for error context and chains
- **Pattern**: Graceful degradation with detailed error context

### Logging & Observability

- **Structured Logging**: tracing ecosystem with JSON output
- **Metrics**: Optional Prometheus integration
- **Performance**: Built-in performance monitoring and resource tracking

## Build System & Commands

### Task Runner (justfile)

```bash
# Development workflow
just fmt          # Format code with rustfmt
just lint         # Run clippy with strict warnings (-D warnings)
just test         # Run all tests with cargo-nextest (unit + integration)
just build        # Build entire workspace

# Testing variants
just test-unit    # Run unit tests only
just test-integration  # Run integration tests only
just test-fuzz    # Run fuzz testing suite
just coverage     # Generate coverage report with tarpaulin

# Component execution
just run-procmond --once --verbose      # Run process monitor
just run-sentinelcli --help             # Run CLI interface  
just run-sentinelagent --config /path   # Run orchestrator agent
```

### Build Configuration

- **Profile**: Release builds with LTO, single codegen unit, stripped symbols
- **Cross-platform**: Static binaries with embedded SQLite
- **Packaging**: Platform-specific service files (systemd, launchd, Windows Service)

## Performance Requirements

- **CPU Usage**: <5% sustained during continuous monitoring
- **Memory Usage**: <100MB resident under normal operation
- **Process Enumeration**: <5 seconds for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: <100ms per detection rule execution

## Security Architecture

### SQL Injection Prevention

- **AST Validation**: sqlparser crate for query structure validation
- **Prepared Statements**: All queries use parameterized statements only
- **Sandboxed Execution**: Read-only database connections for detection engine
- **Query Whitelist**: Only SELECT statements with approved functions allowed

### Cryptographic Components

- **Hashing**: BLAKE3 for fast cryptographic hashing
- **Signatures**: Optional Ed25519 for audit chain signing
- **Integrity**: HMAC for message authentication
- **Chain Verification**: Tamper-evident audit logging with hash chains

### Resource Management

- **Bounded Channels**: Configurable capacity with backpressure policies
- **Memory Limits**: Cooperative yielding and memory budget enforcement
- **Timeout Support**: Cancellation tokens for graceful shutdown
- **Circuit Breakers**: Reliability patterns for external dependencies

## Cross-Platform Strategy

### Process Enumeration

- **Phase 1**: sysinfo crate for unified cross-platform baseline
- **Phase 2**: Platform-specific enhancements (eBPF, ETW, EndpointSecurity)
- **Fallback**: Graceful degradation when enhanced features unavailable

### Privilege Management

- **Linux**: CAP_SYS_PTRACE, immediate capability dropping
- **Windows**: SeDebugPrivilege, token restriction after init
- **macOS**: Minimal entitlements, sandbox compatibility

## Testing Strategy

### Test Runner & Framework

- **Test Runner**: cargo-nextest for faster, more reliable test execution
- **Async Testing**: tokio-test for async runtime testing utilities
- **CLI Testing**: insta for snapshot testing of CLI outputs and behavior
- **Integration Testing**: assert_cmd and predicates for comprehensive CLI validation

### Testing Approach

- **Unit Testing**: Algorithms and core logic only, minimal scope
- **Integration Testing**: Primary testing approach with minimal mocking for realistic scenarios
- **Property Testing**: proptest for generative testing of edge cases and invariants
- **Fuzz Testing**: Extensive fuzzing for security-critical components (SQL parser, config validation)
- **Performance Testing**: Criterion benchmarks with regression detection and CI integration

### Coverage & Quality

- **Target Coverage**: 90% line coverage across the codebase
- **Coverage Tools**: tarpaulin for coverage measurement and reporting
- **Snapshot Testing**: insta for deterministic CLI output validation
- **CI Matrix**: Test across Linux, macOS, Windows with multiple Rust versions (stable, beta, MSRV)
