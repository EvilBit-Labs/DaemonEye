# SentinelD AI Coding Assistant Instructions

## Architecture Overview

SentinelD is a **three-component security architecture** with strict privilege separation:

- **`procmond/`**: Privileged process collector (minimal attack surface, protobuf IPC)
- **`sentinelagent/`**: User-space orchestrator (detection engine, alert delivery)
- **`sentinelcli/`**: CLI interface (read-only database access, operator queries)
- **`sentinel-lib/`**: Shared library (config, models, storage, detection, alerting, crypto, telemetry)

Security boundaries: Only `procmond` runs with elevated privileges; `sentinelagent` handles network/detection; `sentinelcli` is query-only.

## Essential Patterns

### Workspace Structure

- **Rust 2024 Edition** with MSRV 1.85+, workspace resolver "3"
- **Zero warnings policy**: `cargo clippy -- -D warnings` must pass
- **Forbidden unsafe code**: `unsafe_code = "forbid"` at workspace level
- Use `just` for all development tasks (DRY composition with `@just <subrecipe>`)

### Error Handling

Always use `thiserror` for structured errors, `anyhow` for context:

```rust
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Database operation failed: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("I/O operation failed: {0}")]
    IoError(#[from] std::io::Error),
}

// Adding context manually with anyhow when #[from] isn't sufficient
use anyhow::Context;

fn read_process_config(path: &Path) -> Result<ProcessConfig, CollectionError> {
    let content = std::fs::read_to_string(path)
        .context(format!("Failed to read process config from {}", path.display()))
        .map_err(|e| CollectionError::IoError(
            std::io::Error::new(std::io::ErrorKind::Other, e)
        ))?;

    // Parse content...
    Ok(ProcessConfig {})
}
```

### Service Layer Pattern

Implement trait-based services for clear boundaries. For process collection we AVOID returning a gigantic `Vec` (which can cause unbounded memory growth on large fleets) and instead expose a backpressure‑friendly async stream so callers can incrementally consume results, short‑circuit, or apply their own batching:

```rust
use futures::stream::BoxStream;
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },
    #[error("Collection timed out before completion")]
    Timeout,
    #[error("Enumeration failed: {0}")]
    EnumerationError(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ProcessStream = BoxStream<'static, Result<ProcessRecord, CollectionError>>;

#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    /// Return an async stream of individual `ProcessRecord` items.
    /// The implementation MUST:
    /// - Respect the optional `deadline`; once exceeded, yield a single `Err(CollectionError::Timeout)` and terminate.
    /// - Avoid buffering all processes in memory simultaneously (bounded internal buffering only).
    /// - Propagate per‑item errors without aborting the entire stream unless fatal (timeout or system enumeration failure).
    fn stream_processes(&self, deadline: Option<Instant>) -> ProcessStream;
}

// Example consumption pattern:
// let collector = SysinfoProcessCollector::new();
// let mut stream = collector.stream_processes(Some(Instant::now() + Duration::from_secs(5)));
// while let Some(item) = stream.next().await { match item { Ok(proc) => { /* handle */ }, Err(e) => { /* log / break */ } }}
```

## Critical Workflows

### Development Commands

```bash
just lint          # Runs fmt-check + clippy + lint-just (required before commit)
just test          # Run all tests with cargo-nextest
just build         # Build entire workspace
just run-procmond  # Execute individual components
```

### Testing Standards

- **Primary approach**: Integration tests with assert_cmd/predicates for CLI validation
- **Stable output**: Use `NO_COLOR=1 TERM=dumb` for CI-friendly testing
- **Async testing**: `#[tokio::test]` with tokio-test utilities
- **Performance**: Criterion benchmarks with regression detection

## Security-First Patterns

### SQL-to-IPC Architecture

- SQL detection rules are **never executed directly** against processes
- `sqlparser` extracts collection requirements from SQL AST to generate protobuf tasks
- `sentinelagent` translates complex SQL queries into simple collection tasks for `procmond`
- `procmond` may overcollect data (granularity limitations), then SQL runs against stored data
- This ensures privilege separation: only `procmond` touches live processes, SQL stays in userspace

**IPC Transport Limits and Rules:**

- **Task Generation Caps**:
  - Max 100 tasks per SQL query (default: 50)
  - Max 16KB per protobuf message (default: 8KB)
  - Max 10-second IPC timeout per request (default: 5s)
  - Behavior on exceedance: Reject query with `ValidationError::ComplexityExceeded`

- **Message Framing**:
  - Length-delimited protobuf messages with varint prefixes
  - Messages MUST include sequence numbers for ordering
  - Mandatory CRC32 checksums for corruption detection

- **Backpressure Semantics**:
  - Credit-based flow control: procmond grants collection credits to sentinelagent
  - Default credit limit: 1000 pending process records
  - ACK/NACK responses required for credit replenishment
  - Windowing: Max 10 concurrent collection tasks in-flight

- **DoS Prevention**:
  - Rate limiting: Max 100 queries/minute per detection rule
  - Memory bounds: 64MB total IPC buffer allocation
  - Timeout escalation: 5s → 10s → connection reset
  - Partial results: Return collected data on timeout, log incomplete status

### Privilege Separation with OS Enforcement

**Concrete Access Definitions:**
- **Write-only audit ledger**: Process can append (`O_WRONLY|O_APPEND`) but not read previous entries
- **Read-only CLI**: No write syscalls to modify store; only read operations (`O_RDONLY`)

**OS Enforcement Mechanisms:**

**procmond (Privileged Collector):**
- Dedicated Unix user/group: `sentineld-proc:sentineld-proc`
- Audit ledger: `open(path, O_WRONLY|O_APPEND|O_CREAT, 0600)` with owner-only write
- Drop capabilities: `CAP_SYS_ADMIN`, `CAP_SYS_PTRACE` after init, retain minimal set
- SELinux/AppArmor: Block read syscalls on audit ledger files
- Seccomp filter: Whitelist only required syscalls (write, openat, close)

**sentinelagent (Detection Engine):**
- Dedicated Unix user/group: `sentineld-agent:sentineld-agent`
- Event store: Standard R/W access with `open(path, O_RDWR)`
- Mount options: `/var/lib/sentineld` with `nodev,nosuid,noexec`
- Network: Outbound-only via iptables rules, no listening sockets
- Filesystem ACLs: Read access to audit ledger, no write permissions

**sentinelcli (Query Interface):**
- Dedicated Unix user/group: `sentineld-cli:sentineld-cli`
- Database access: `open(path, O_RDONLY)` only
- Seccomp filter: Block write, unlink, rename syscalls on database files
- No network capabilities, filesystem read-only except for temp output files
- Group membership: Add to `sentineld-agent` group for IPC communication only

### Input Validation

All external input must be validated with detailed error messages:

```rust
use sqlparser::{dialect::SQLiteDialect, parser::Parser, ast::Statement};

pub fn validate_detection_rule(rule: &str) -> Result<Vec<Statement>, ValidationError> {
    let dialect = SQLiteDialect {};
    let statements = Parser::parse_sql(&dialect, rule)
        .map_err(|e| ValidationError::InvalidSql { reason: e.to_string() })?;

    // Validation checks for security and complexity limits
    for statement in &statements {
        // 1. Only SELECT statements allowed
        if !matches!(statement, Statement::Query(_)) {
            return Err(ValidationError::ForbiddenStatement {
                statement_type: format!("{:?}", statement)
            });
        }

        // 2. Reject banned functions (e.g., file operations, network calls)
        validate_banned_functions(statement)?;

        // 3. Enforce complexity limits
        validate_query_complexity(statement)?;
    }

    Ok(statements)
}

fn validate_banned_functions(statement: &Statement) -> Result<(), ValidationError> {
    const BANNED_FUNCTIONS: &[&str] = &[
        "load_extension", "fts3_tokenizer", "zipfile", "fsdir",
        "readfile", "writefile", "edit", "shell"
    ];

    // Check for banned function names in SQL AST
    // Implementation would recursively walk the AST checking function calls
    Ok(())
}fn validate_query_complexity(statement: &Statement) -> Result<(), ValidationError> {
    // Enforce limits: max 5 joins, max 3 nested subqueries, max 50 projections
    const MAX_JOINS: usize = 5;
    const MAX_SUBQUERIES: usize = 3;
    const MAX_PROJECTIONS: usize = 50;

    // Implementation would analyze AST structure and count complexity metrics
    // Return specific ValidationError for each limit violation
    Ok(())
}
```

## Key Dependencies & Patterns

- **Database**: redb pure Rust embedded database for event storage
- **Audit Ledger**: Certificate Transparency-style Merkle tree with BLAKE3 hashing for tamper-evident logging
- **Cryptographic Integrity**: `rs-merkle` for efficient inclusion proofs, Ed25519 for optional signatures
- **CLI**: clap v4 with derive macros, support `--json` output + `NO_COLOR`
- **Async**: Tokio runtime with structured logging via `tracing`
- **Process enumeration**: `sysinfo` crate with platform-specific optimizations
- **IPC**: Custom protobuf over Unix sockets/named pipes between components

## Documentation Standards

- **Comprehensive rustdoc** for all public APIs with examples
- **Mermaid diagrams** for architecture (Prettier ignores Markdown)
- **Relative links** for cross-references, maintain link hygiene
- See `AGENTS.md` for complete project rules and `WARP.md` for operational commands
