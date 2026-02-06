# DaemonEye Development Guide

Operational commands and development workflows for **DaemonEye**.

**Source of Truth**: Technical requirements in [.kiro/steering/](./kiro/steering/) and [.kiro/specs/](./kiro/specs/).

---

## Quick Reference

### Task Runner (justfile)

```bash
just fmt          # Format all code
just fmt-check    # Check formatting (CI-friendly)
just lint         # Runs fmt-check + clippy + lint-just
just build        # Build all binaries with features
just check        # Quick check without build
just test         # Run all tests

just run-procmond [args]        # Run procmond
just run-daemoneye-cli [args]   # Run daemoneye-cli
just run-daemoneye-agent [args] # Run daemoneye-agent
```

### Core Commands

```bash
cargo build --workspace && cargo test --workspace
cargo clippy --workspace -- -D warnings
NO_COLOR=1 TERM=dumb cargo test --workspace  # Stable output
cargo bench                                   # Criterion benchmarks
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

# GitHub CI monitoring
gh run list --branch <branch> --limit 5       # Check CI status
gh run view <run-id> --log-failed             # View failed job logs
gh run view <run-id> --json status,jobs       # JSON status check
```

Commit style: [.github/commit-instructions.md](.github/commit-instructions.md)

---

## AI Assistant Rules

### Behavior Guidelines

1. **No Auto-Commits**: Never commit without explicit permission. Always present diffs for approval.
2. **Security-First**: All changes must maintain least privilege and undergo security review.
3. **Zero-Warnings Policy**: `cargo clippy -- -D warnings` with no exceptions.
4. **Operator-Centric**: Prioritize workflows efficient in contested/airgapped environments.
5. **Documentation**: Mermaid for diagrams, relative links, maintain link hygiene.
6. **Testing Required**: All code changes must include appropriate tests.
7. **Linter Restrictions**: Never remove clippy restrictions or `deny` attributes.
8. **File Size Limit**: Keep source files under 500-600 lines when possible.

### Rule Precedence

1. Project Rules (.cursor/rules/, AGENTS.md, GEMINI.md)
2. Steering Documents (.kiro/steering/)
3. Technical Specifications (.kiro/specs/)
4. Embedded defaults

---

## Architecture Overview

DaemonEye implements **three-component security architecture** with strict privilege separation:

### Components

| Component           | Privileges                  | Network       | Database            | Function                     |
| ------------------- | --------------------------- | ------------- | ------------------- | ---------------------------- |
| **procmond**        | Elevated (drops after init) | None          | Write-only (audit)  | Privileged process collector |
| **daemoneye-agent** | Minimal                     | Outbound-only | Read/write (events) | Detection orchestrator       |
| **daemoneye-cli**   | Minimal                     | None          | Read-only           | Operator CLI                 |
| **daemoneye-lib**   | N/A                         | N/A           | N/A                 | Shared library               |
| **collector-core**  | N/A                         | N/A           | N/A                 | Collector framework          |

### Deployment Tiers

- **Free**: Standalone (procmond + daemoneye-agent + daemoneye-cli)
- **Business**: + Security Center + Enterprise integrations ($199/site)
- **Enterprise**: + Kernel monitoring + Federated architecture + Advanced SIEM

```mermaid
flowchart LR
    subgraph "Host System"
        P[procmond] -->|IPC protobuf| A[daemoneye-agent]
        A -->|reads/writes| DB1[(Event Store)]
        P -->|writes| DB2[(Audit Ledger)]
        C[daemoneye-cli] -->|reads| DB1 & DB2
    end
    A -->|outbound alerts| EXT[(Alert Sinks)]
    A -.->|mTLS| SC[Security Center]
```

### Security Boundaries

- **Privilege Separation**: Only procmond runs elevated when necessary
- **IPC**: Protobuf over Unix sockets/named pipes with CRC32 validation
- **No Inbound Network**: Outbound-only for alert delivery
- **SQL Injection Prevention**: AST validation with sqlparser, prepared statements only

---

## Technology Stack

### Core

| Category     | Technology                      |
| ------------ | ------------------------------- |
| Language     | Rust 2024 Edition (MSRV: 1.91+) |
| Async        | Tokio                           |
| Database     | redb (pure Rust embedded)       |
| CLI          | clap v4 with derive             |
| Process Enum | sysinfo                         |
| Logging      | tracing ecosystem               |
| Config       | YAML/TOML via figment           |
| IPC          | interprocess + protobuf         |
| Event Bus    | crossbeam                       |
| Testing      | cargo-nextest, insta, criterion |

### Dependencies

| Category      | Crate     | Version |
| ------------- | --------- | ------- |
| Runtime       | tokio     | 1.0+    |
| Serialization | serde     | 1.0+    |
| CLI           | clap      | 4.0+    |
| Database      | redb      | 3.0+    |
| Process       | sysinfo   | 0.37+   |
| Logging       | tracing   | 0.1+    |
| Errors        | thiserror | 2.0+    |
| Errors        | anyhow    | 1.0+    |
| Crypto        | blake3    | 1.8+    |
| Crypto        | rs-merkle | 1.5+    |
| Testing       | insta     | 1.0+    |
| Benchmarks    | criterion | 0.7+    |

### OS Support

| OS      | Version                                       | Arch          | Status    |
| ------- | --------------------------------------------- | ------------- | --------- |
| Linux   | Ubuntu 20.04+ LTS, RHEL/CentOS 8+, Debian 11+ | x86_64, ARM64 | Primary   |
| macOS   | 14.0+ (Sonoma)                                | x86_64, ARM64 | Primary   |
| Windows | 10+, Server 2019+                             | x86_64, ARM64 | Primary   |
| Linux   | Alpine 3.16+, Amazon Linux 2+                 | x86_64, ARM64 | Secondary |
| FreeBSD | 13.0+                                         | x86_64, ARM64 | Secondary |

---

## Coding Standards

### Rust Requirements

- **Edition**: Rust 2024 (MSRV: 1.91+)
- **Linting**: `cargo clippy -- -D warnings` (zero warnings)
- **Format args**: Use `{variable}` inlined syntax in `format!`/`anyhow!` macros (`clippy::uninlined_format_args`)
- **If-else ordering**: Clippy prefers `==` checks first in if-else (`clippy::unnecessary_negation`)
- **map_err_ignore**: Name ignored variables in closures (`|_elapsed|` not `|_|`)
- **as_conversions**: Add `#[allow(clippy::as_conversions)]` with safety comment for intentional casts
- **Async in tracing macros**: Never `.await` inside `info!`/`debug!`/`warn!`/`error!` - causes `future_not_send`. Extract value first.
- **Safety**: `unsafe_code = "forbid"` at workspace level
- **Formatting**: `rustfmt` with 119 char line length
- **Rustdoc**: Escape brackets in paths like `/proc/\[pid\]/stat` to avoid broken link warnings
- **Errors**: `thiserror` for structured errors, `anyhow` for context
- **Async**: Async-first with Tokio
- **Logging**: Structured with `tracing`
- **Strictness**: `warnings = "deny"` at workspace; `allow` requires justification

### Pre-commit Requirements

1. `cargo clippy -- -D warnings` (zero warnings)
2. `cargo fmt --all --check` (formatting)
3. `cargo test --workspace` (all tests pass)
4. `just lint-just` (justfile syntax)
5. No new `unsafe` without approval
6. Benchmarks within acceptable ranges

**Gotchas**: Pre-commit runs `cargo fmt` which modifies files. If unstaged changes exist, commit may fail with "Stashed changes conflicted". Run `cargo fmt --all` before staging or reset unrelated unstaged files.

---

## Security Model

### Core Requirements

- Least privilege: Components run with minimal permissions
- Automatic privilege drop after initialization
- SQL injection prevention: AST validation, prepared statements only
- Credentials: Environment variables or OS keychain, never hardcoded
- No inbound network: Outbound-only for alerts
- Audit trail: Merkle tree with BLAKE3 integrity

### Enterprise Features

- mTLS with certificate chain validation
- SLSA Level 3 provenance, Cosign signatures
- Merkle tree with inclusion proofs
- Sandboxed detection engine (read-only DB)
- Query whitelist (SELECT only with approved functions)

### Integer Overflow Protection

```toml
[profile.release]
overflow-checks = true
lto = "thin"
codegen-units = 1
```

Use `checked_*`, `saturating_*`, or explicit `wrapping_*` for security-sensitive calculations.

### Safe Concurrency

- **Tokio primitives**: `Semaphore`, `mpsc`, `oneshot`, `watch`, `Notify`
- **Lock scope**: Never await while holding locks
- **Async locks**: `tokio::sync::Mutex/RwLock` for async; `std::sync` for sync-only
- **Bounded concurrency**: Use `Semaphore` with defined capacity
- **Linting**: Enable `clippy::await_holding_lock = "deny"`

### Cryptographic Standards

| Purpose    | Approved                             |
| ---------- | ------------------------------------ |
| Hashing    | BLAKE3, SHA2 (never SHA-1)           |
| Signatures | ed25519-dalek                        |
| AEAD       | chacha20poly1305, aes-gcm            |
| KDF        | HKDF-SHA256, Argon2id                |
| TLS        | rustls (TLS 1.2+ min, 1.3 preferred) |
| Entropy    | getrandom                            |
| Secrets    | secrecy, zeroize crates              |

### Input Validation

- Treat all external inputs as untrusted
- Validate early, reject with actionable errors
- Use typed parsers over regex
- Length limits on all variable-length inputs
- SQL: AST validation with `sqlparser`

### Newtype Safety

Use newtypes for domain constraints (ports vs PIDs vs timestamps). Implement `TryFrom` with validation.

### Dependency Security

- Commit `Cargo.lock`, use `resolver = "3"`
- Pin security-critical deps, avoid wildcards
- `default-features = false`, enable only needed
- Git deps: pin to commit SHA
- Tools: `cargo audit`, `cargo deny`, `cargo vet`

---

## Performance Budgets

| Metric         | Target                          |
| -------------- | ------------------------------- |
| CPU Usage      | < 5% sustained                  |
| Memory         | < 100 MB resident               |
| Process Enum   | < 5s for 10,000+ processes      |
| DB Writes      | > 1,000 records/sec             |
| Alert Latency  | < 100ms per rule                |
| Query Response | Sub-second for 100k+ events/min |

### Resource Management

- Bounded channels with backpressure
- Memory budget enforcement
- Cancellation tokens for graceful shutdown
- Circuit breakers for external dependencies
- Graceful degradation when constrained

---

## Code Organization

### Workspace Structure

```text
DaemonEye/
├── Cargo.toml              # Workspace root
├── procmond/               # Privileged collector
├── daemoneye-agent/        # Detection orchestrator
├── daemoneye-cli/          # Operator CLI
├── daemoneye-lib/          # Shared library
│   ├── proto/              # Protobuf definitions
│   └── src/
│       ├── alerting.rs     # Alert delivery
│       ├── config.rs       # Configuration
│       ├── crypto.rs       # Cryptographic functions
│       ├── detection.rs    # SQL detection engine
│       ├── models.rs       # Data structures
│       └── storage.rs      # Database (redb)
├── collector-core/         # Collector framework
├── tests/                  # Integration tests
└── .kiro/                  # Steering docs & specs
```

### Service Layer Pattern

```rust
#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    async fn collect_processes(&self) -> Result<CollectionResult, CollectionError>;
}

#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, Box<dyn Error + Send + Sync>>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}
```

---

## Testing Strategy

### Three-Tier Architecture

| Tier           | Scope                 | Tools                        |
| -------------- | --------------------- | ---------------------------- |
| Unit           | Individual components | tokio-test, trait mocking    |
| Integration    | Cross-component       | insta, predicates, real redb |
| E2E (Optional) | Full system workflows | Test data seeding            |

### Quality Tools

- **Runner**: cargo-nextest (parallel, failure isolation)
- **Coverage**: llvm-cov (target: >85%)
- **Property**: proptest for edge cases
- **Fuzz**: Security-critical components
- **Snapshot**: insta for CLI output
- **Cross-crate traits**: Import traits for method access (e.g., `use daemoneye_eventbus::rpc::RegistrationProvider;`)

### Test Environment

```bash
NO_COLOR=1 TERM=dumb cargo test --workspace           # Stable output
RUST_BACKTRACE=1 cargo test -p daemoneye-lib --nocapture  # Debug
cargo bench --baseline previous                        # Regression
```

### CI Matrix

- Platforms: Linux, macOS, Windows
- Rust: stable, beta, MSRV (1.91+)
- Architectures: x86_64, ARM64

---

## Database Design

### redb Tables

| Table            | Access                   | Component |
| ---------------- | ------------------------ | --------- |
| processes        | R/W                      | agent     |
| scans            | R/W                      | agent     |
| detection_rules  | R/W                      | agent     |
| alerts           | R/W                      | agent     |
| alert_deliveries | R/W                      | agent     |
| audit_ledger     | W (procmond), R (others) | procmond  |

### Core Data Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
    pub status: ProcessStatus,
    pub executable_hash: Option<String>, // SHA-256
    pub collection_time: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },
    #[error("Database operation failed: {0}")]
    DatabaseError(String),
    #[error("IPC communication failed: {0}")]
    IpcError(String),
}
```

---

## CLI & Configuration

### CLI Design (clap v4)

```rust
#[derive(Parser)]
#[command(name = "procmond", about = "Process monitoring daemon")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    #[arg(short, long, default_value = "info")]
    pub log_level: String,
}
```

- Use `--json` for machine-readable output
- Respect `NO_COLOR` and `TERM=dumb`
- Provide actionable error messages

### Configuration Precedence

1. Command-line flags
2. Environment variables (`PROCMOND_*`)
3. User config (`~/.config/procmond/config.yaml`)
4. System config (`/etc/procmond/config.yaml`)
5. Embedded defaults

---

## Alert System

- Trait-based plugin system (`AlertSink` trait)
- Concurrent delivery to multiple sinks
- Retry with exponential backoff
- Circuit breaking for failing sinks
- Delivery audit trail

---

## CI/CD & Reviews

### Continuous Integration

- GitHub Actions: Linux, macOS, Windows matrix
- Rust: stable, beta, MSRV (1.91+)
- Checks: fmt, clippy strict, tests, benchmarks
- Security: Dependency scanning, SLSA (Enterprise)

### Security Scanners

- **zizmor**: GitHub Actions permissions - add explicit `permissions:` block to workflows
- **CodeQL rust/cleartext-logging**: Don't interpolate sensitive values in assert messages

### Code Review

- Primary tool: coderabbit.ai
- Security focus, performance assessment
- Single maintainer: UncleSp1d3r

### Commit Standards

- Format: Conventional Commits
- Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`
- Breaking: `!` in header or `BREAKING CHANGE:` in footer

---

## Contribution Checklists

### Adding a Detection Rule

Files: `daemoneye-lib/src/detection.rs`, `daemoneye-lib/src/models.rs`, `daemoneye-agent/src/rules/`, `tests/integration/`

- [ ] Define rule structure with SQL and metadata
- [ ] Implement AST validation (sqlparser)
- [ ] Unit tests with mock data
- [ ] Integration test via agent
- [ ] Validate alert delivery via CLI
- [ ] `cargo clippy -- -D warnings` passes
- [ ] Performance test if complex
- [ ] Rustdoc documentation

### Adding a CLI Option

Files: `daemoneye-cli/src/main.rs`, `daemoneye-lib/src/config.rs`, `tests/integration/`

- [ ] Update clap derive structures
- [ ] Configuration handling in lib
- [ ] Help text and defaults
- [ ] Insta snapshot tests
- [ ] Test short/long forms
- [ ] Validate with `NO_COLOR=1 TERM=dumb`
- [ ] Update shell completions
- [ ] Rustdoc documentation

### Optimizing Database Write Path

Files: `daemoneye-lib/src/storage.rs`, `procmond/src/collector.rs`, `benches/`

- [ ] Establish criterion baseline
- [ ] Implement optimization
- [ ] Batch processing if applicable
- [ ] Validate >1000 records/sec
- [ ] Test with 10k+ processes
- [ ] Memory profiling
- [ ] ACID guarantees maintained
- [ ] Document characteristics

---

## Code Generation Guidelines

When generating code:

01. Use Rust 2024 Edition
02. Comprehensive error handling with thiserror
03. Async/await with Tokio
04. Tests with insta for snapshots
05. Service layer pattern with traits
06. Logging with tracing
07. Workspace inheritance for deps
08. Performance considerations
09. Security best practices
10. Rustdoc for public APIs

---

## Source-of-Truth Map

| Section             | Source                                                                                                           |
| ------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Architecture        | [.kiro/steering/structure.md](./.kiro/steering/structure.md)                                                     |
| Technology          | [.kiro/steering/tech.md](./.kiro/steering/tech.md)                                                               |
| Product             | [.kiro/steering/product.md](./.kiro/steering/product.md)                                                         |
| Core Requirements   | [.kiro/specs/DaemonEye-core-monitoring/requirements.md](./.kiro/specs/DaemonEye-core-monitoring/requirements.md) |
| Business Features   | [.kiro/specs/business-tier-features/requirements.md](./.kiro/specs/business-tier-features/requirements.md)       |
| Enterprise Features | [.kiro/specs/enterprise-tier-features/requirements.md](./.kiro/specs/enterprise-tier-features/requirements.md)   |
| Development         | [.kiro/steering/development.md](./.kiro/steering/development.md)                                                 |

### Cross-References

- Cursor IDE: [.cursor/rules/](./.cursor/rules/)
- Security: [.cursor/rules/security-standards.mdc](./.cursor/rules/security-standards.mdc)
- Rust: [.cursor/rules/rust-standards.mdc](./.cursor/rules/rust-standards.mdc)

---

## Glossary

| Term   | Definition                                             |
| ------ | ------------------------------------------------------ |
| AST    | Abstract Syntax Tree (SQL validation)                  |
| BLAKE3 | Cryptographic hash for audit trails                    |
| CEF    | Common Event Format                                    |
| eBPF   | Extended Berkeley Packet Filter                        |
| ETW    | Event Tracing for Windows                              |
| IPC    | Inter-Process Communication                            |
| mTLS   | Mutual Transport Layer Security                        |
| redb   | Pure Rust embedded database                            |
| SLSA   | Supply-chain Levels for Software Artifacts             |
| STIX   | Structured Threat Information eXpression               |
| TAXII  | Trusted Automated eXchange of Intelligence Information |

---

**Remember**: DaemonEye is security-focused. Prioritize security, performance, and reliability. When in doubt, choose the more secure and observable approach.
