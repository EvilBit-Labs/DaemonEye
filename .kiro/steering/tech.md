# DaemonEye Technical Stack

## Language & Runtime

- **Language**: Rust 2024 Edition (MSRV: 1.70+)
- **Safety**: `unsafe_code = "forbid"` at workspace level
- **Quality**: `warnings = "deny"` with zero-warnings policy
- **Async Runtime**: Tokio with full feature set for I/O and task management

## Core Dependencies

### Database Layer

- **redb**: Pure Rust embedded database for optimal performance and security
- **Features**: Concurrent access, ACID transactions, zero-copy deserialization
- **Configuration**: Separate event store and audit ledger with different durability settings

### CLI Framework

- **clap v4**: Derive macros with shell completions (bash, zsh, fish, PowerShell)
- **Terminal**: Automatic color detection, NO_COLOR and TERM=dumb support

### IPC Communication

- **Protocol**: Custom protobuf over Unix sockets (Linux/macOS) and named pipes (Windows)
- **Features**: Async message handling, automatic reconnection with exponential backoff
- **Security**: Connection authentication and optional encryption
- **Scope**: CLI-to-agent communication (daemoneye-cli ↔ daemoneye-agent)

### Event Bus (daemoneye-eventbus)

- **Foundation**: Cross-platform IPC with embedded broker architecture
- **Transport**: Unix domain sockets (Linux/macOS) and named pipes (Windows)
- **Features**: Topic-based pub/sub messaging, wildcard subscriptions, backpressure handling
- **Performance**: 10,000+ messages/second throughput, sub-millisecond latency
- **Scope**: Multi-process coordination (daemoneye-agent ↔ collector-core components)
- **Location**: Embedded broker runs within daemoneye-agent, clients connect from collector-core components

### Internal Event Distribution (collector-core)

- **Foundation**: `crossbeam` crate for high-performance concurrent event distribution
- **Features**: Lock-free channels, efficient backoff strategies, bounded concurrency
- **Performance**: Optimized for high-throughput event processing with minimal contention
- **Scope**: Internal in-process message distribution within collector-core components

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
just run-daemoneye-cli --help             # Run CLI interface
just run-daemoneye-agent --config /path   # Run orchestrator agent
```

### Build Configuration

- **Profile**: Release builds with LTO, single codegen unit, stripped symbols
- **Cross-platform**: Static binaries with embedded SQLite
- **Packaging**: Platform-specific service files (systemd, launchd, Windows Service)

## Performance Requirements

- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Process Enumeration**: \<5 seconds for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: \<100ms per detection rule execution

## Security Architecture

### SQL Injection Prevention

- **AST Validation**: sqlparser crate for query structure validation
- **Prepared Statements**: All queries use parameterized statements only
- **Sandboxed Execution**: Read-only database connections for detection engine
- **Query Whitelist**: Only SELECT statements with approved functions allowed

### Cryptographic Components

- **Hashing**: BLAKE3 for fast cryptographic hashing
- **Signatures**: Optional Ed25519 for audit entry signing
- **Integrity**: HMAC for message authentication
- **Verification**: Certificate Transparency-style Merkle tree with inclusion proofs

### Resource Management

- **Bounded Channels**: Configurable capacity with backpressure policies
- **Memory Limits**: Cooperative yielding and memory budget enforcement
- **Timeout Support**: Cancellation tokens for graceful shutdown
- **Circuit Breakers**: Reliability patterns for external dependencies

## Cross-Platform Strategy

### Process Enumeration

- **Phase 1**: sysinfo crate for unified cross-platform baseline
- **Phase 2**: Platform-specific enhancements (eBPF, ETW, EndpointSecurity)
- **Phase 3**: Kernel-level real-time monitoring (Enterprise tier)
- **Fallback**: Graceful degradation when enhanced features unavailable

### Kernel Monitoring (Enterprise Tier)

- **Linux**: eBPF programs for real-time process and syscall monitoring
- **Windows**: ETW integration for kernel events and registry monitoring
- **macOS**: EndpointSecurity framework for process and file system events
- **Network**: Platform-specific network event correlation

### Privilege Management

- **Linux**: CAP_SYS_PTRACE, immediate capability dropping
- **Windows**: SeDebugPrivilege, token restriction after init
- **macOS**: Minimal entitlements, sandbox compatibility

### Enterprise Security Features

- **Authentication**: mTLS with certificate chain validation
- **Code Signing**: SLSA Level 3 provenance, Cosign signatures
- **Compliance**: NIST, ISO 27001, CIS framework mappings
- **Threat Intelligence**: STIX/TAXII integration, quarterly rule packs

## Testing Strategy

### Test Runner & Framework

- **Test Runner**: cargo-nextest for faster, more reliable test execution
- **Async Testing**: tokio-test for async runtime testing utilities
- **CLI Testing**: insta for snapshot testing of CLI outputs and behavior
- **Integration Testing**: insta for snapshot testing and predicates for validation
- **Property Testing**: proptest for generative testing of edge cases and invariants

### Testing Approach

- **Unit Testing**: Algorithms and core logic only, minimal scope
- **Integration Testing**: Primary testing approach with minimal mocking for realistic scenarios
- **Property Testing**: proptest for generative testing of edge cases and invariants
- **Fuzz Testing**: Extensive fuzzing for security-critical components (SQL parser, config validation)
- **Performance Testing**: Criterion benchmarks with regression detection and CI integration

### Coverage & Quality

- **Target Coverage**: >85% code coverage across the codebase
- **Coverage Tools**: llvm-cov for coverage measurement and reporting
- **Snapshot Testing**: insta for deterministic CLI output validation
- **CI Matrix**: Test across Linux, macOS, Windows with multiple Rust versions (stable, beta, MSRV)
