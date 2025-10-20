# DaemonEye Technical Stack

## Language & Runtime

- **Language**: Rust 2024 Edition (MSRV: 1.87+)
- **Safety**: `unsafe_code = "forbid"` at workspace level with comprehensive linting
- **Quality**: `warnings = "deny"` with zero-warnings policy enforced by CI
- **Async Runtime**: Tokio with full feature set for I/O and task management
- **Collection Framework**: collector-core for extensible event source management

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
just test-ci      # Run tests with nextest for CI
just test-fast    # Run only fast unit tests
just coverage     # Generate coverage report with llvm-cov

# Benchmarking
just bench        # Run all benchmarks
just bench-process # Run process collection benchmarks
just bench-database # Run database operation benchmarks

# Component execution
just run-procmond --interval 30 --enhanced-metadata    # Run process monitor
just run-daemoneye-cli --format json                   # Run CLI interface
just run-daemoneye-agent --log-level debug             # Run orchestrator agent
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

### SQL-to-IPC Architecture

- **Detection Rule Processing**: SQL detection rules are never executed directly against live processes
- **AST Analysis**: sqlparser extracts collection requirements from SQL AST to generate protobuf tasks
- **Task Translation**: daemoneye-agent translates complex SQL queries into simple collection tasks for procmond
- **Overcollection Strategy**: procmond may overcollect data due to granularity limitations, then SQL runs against stored data
- **Privilege Separation**: Only procmond touches live processes; SQL execution remains in userspace
- **Query Validation**: Only SELECT statements with approved functions allowed in detection engine
- **Prepared Statements**: All database queries use parameterized statements only

### Cryptographic Components

- **Audit Ledger**: Certificate Transparency-style append-only log with Merkle tree structure
- **Hashing**: BLAKE3 for fast cryptographic hashing and Merkle tree construction
- **Merkle Trees**: `rs-merkle` for efficient inclusion/exclusion proofs and batch verification
- **Signatures**: Optional Ed25519 for audit entry signing and periodic root hash attestation
- **Integrity**: HMAC for message authentication and tamper detection
- **Verification**: Logarithmic proof sizes for efficient audit trail validation
- **Airgap Support**: Periodic root hash checkpoints for manual external verification

### Resource Management

- **Bounded Channels**: Configurable capacity with backpressure policies
- **Memory Limits**: Cooperative yielding and memory budget enforcement
- **Timeout Support**: Cancellation tokens for graceful shutdown
- **Circuit Breakers**: Reliability patterns for external dependencies

## Cross-Platform Strategy

### OS Support Matrix

| OS          | Version              | Architecture  | Status    | Notes                          |
| ----------- | -------------------- | ------------- | --------- | ------------------------------ |
| **Linux**   | Ubuntu 20.04+ LTS    | x86_64, ARM64 | Primary   | Full feature support           |
| **Linux**   | RHEL/CentOS 8+       | x86_64, ARM64 | Primary   | Full feature support           |
| **Linux**   | Alma/Rocky Linux 8+  | x86_64, ARM64 | Primary   | Full feature support           |
| **Linux**   | Debian 11+ LTS       | x86_64, ARM64 | Primary   | Full feature support           |
| **macOS**   | 14.0+ (Sonoma)       | x86_64, ARM64 | Primary   | Native process monitoring      |
| **Windows** | Windows 10+          | x86_64, ARM64 | Primary   | Service deployment[^5]         |
| **Windows** | Windows Server 2019+ | x86_64        | Primary   | Enterprise features[^6]        |
| **Windows** | Windows Server 2022  | x86_64, ARM64 | Primary   | Enterprise standard            |
| **Windows** | Windows 11           | x86_64, ARM64 | Primary   | Full feature support           |
| **Linux**   | Alpine 3.16+         | x86_64, ARM64 | Secondary | Container deployments          |
| **Linux**   | Amazon Linux 2+      | x86_64, ARM64 | Secondary | Cloud deployments              |
| **Linux**   | Ubuntu 18.04         | x86_64, ARM64 | Secondary | Best-effort support[^1][^8]    |
| **Linux**   | RHEL 7               | x86_64        | Secondary | Best-effort support[^2][^8]    |
| **macOS**   | 12.0+ (Monterey)     | x86_64, ARM64 | Secondary | Best-effort support[^7]        |
| **FreeBSD** | 13.0+                | x86_64, ARM64 | Secondary | pfSense/OPNsense ecosystem[^9] |

**Testing Policy**: We test against the current and one previous major version of Windows and macOS to ensure compatibility with enterprise environments. macOS 14.0+ (Sonoma and later) are Primary support.

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

[^5]: Windows 10: EOL October 14, 2025. Organizations should plan migration to Windows 11.

[^6]: Windows Server 2019: EOL January 9, 2029. Consider upgrading to Windows Server 2022.

[^1]: Ubuntu 18.04 (EOL April 2023): Legacy support for long-lived server deployments. Limited testing and no guaranteed compatibility.

[^8]: **Enterprise Tier**: No eBPF kernel monitoring (requires kernel 5.4+). Graceful degradation to userspace monitoring with reduced threat detection capabilities.

[^2]: RHEL 7 (EOL June 2024): Enterprise legacy support for organizations with extended support contracts. Compatibility not guaranteed.

[^7]: macOS 12.0 (Monterey): EOL September 16, 2024. Legacy support for organizations with older Mac hardware. **Enterprise Tier**: Limited EndpointSecurity framework features compared to macOS 14.0+. Reduced code signing bypass detection and advanced threat monitoring capabilities.

[^9]: **FreeBSD 13.0+**: pfSense/OPNsense ecosystem support. **Enterprise Tier**: No eBPF kernel monitoring (FreeBSD uses classic BPF). Full process and network monitoring via sysinfo crate and FreeBSD audit system.
