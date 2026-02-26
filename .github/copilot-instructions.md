# DaemonEye AI Coding Assistant Instructions

## Architecture Overview

DaemonEye is a **three-component security architecture** with strict privilege separation:

- **`procmond/`**: Privileged process collector (minimal attack surface, protobuf IPC)
- **`daemoneye-agent/`**: User-space orchestrator (detection engine, alert delivery)
- **`daemoneye-cli/`**: CLI interface (read-only database access, operator queries)
- **`daemoneye-lib/`**: Shared library (config, models, storage, detection, alerting, crypto, telemetry)

Security boundaries: Only `procmond` runs with elevated privileges; `daemoneye-agent` handles network/detection; `daemoneye-cli` is query-only.

## Essential Patterns

### Workspace Structure

- **Rust 2024 Edition** with MSRV 1.85+, pure workspace with independent crates
- **Zero warnings policy**: `cargo clippy --workspace -- -D warnings` must pass
- **Forbidden unsafe code**: `unsafe_code = "forbid"` enforced at workspace level
- **Linter restrictions**: Never remove clippy restrictions or allow linters marked as `deny` without explicit permission
- **Workspace members**: `procmond`, `daemoneye-agent`, `daemoneye-cli`, `daemoneye-lib`
- Use `just` for all development tasks (DRY composition with `@just <subrecipe>`)

### Error Handling

Always use `thiserror` for structured errors, `anyhow` for context:

```rust,ignore
use thiserror::Error;
use std::path::Path;

#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Database operation failed: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("I/O operation failed: {context}: {source}")]
    IoError {
        #[source]
        source: std::io::Error,
        context: String,
    },
}

struct ProcessConfig; // Example type

fn read_process_config(path: &Path) -> Result<ProcessConfig, CollectionError> {
    let content = std::fs::read_to_string(path).map_err(|e| CollectionError::IoError {
        source: e,
        context: format!("reading process config from {}", path.display()),
    })?;

    // Parse content...
    let _ = content; // placeholder to show content use
    Ok(ProcessConfig {})
}
```

### IPC Communication with interprocess and protobuf

**Transport Layer**: Use `interprocess` crate for cross-platform IPC communication:

```rust,ignore
use interprocess::local_socket::LocalSocketStream;
use daemoneye_lib::proto::{DetectionTask, DetectionResult};

// Unix Domain Sockets (Linux/macOS) or Named Pipes (Windows)
let stream = LocalSocketStream::connect("/tmp/daemoneye.sock")?;

// Protobuf message serialization with CRC32 checksums
let task = DetectionTask::new()
    .with_rule_id("suspicious_process")
    .with_query("SELECT * FROM processes WHERE name = 'malware.exe'")
    .build();

// Serialize and send with framing
let serialized = prost::Message::encode_to_vec(&task)?;
// Send with CRC32 and length prefixing for integrity
```

**Message Framing**: All IPC messages use length-delimited protobuf with CRC32 checksums for corruption detection and sequence numbers for ordering.

**Backpressure**: Credit-based flow control with configurable limits (default: 1000 pending records, max 10 concurrent tasks).

### Service Layer Pattern

Implement trait-based services for clear boundaries. For process collection we AVOID returning a gigantic `Vec` (which can cause unbounded memory growth on large fleets) and instead expose a backpressure‑friendly async stream so callers can incrementally consume results, short‑circuit, or apply their own batching:

```rust,ignore
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
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

struct ProcessRecord; // Example type

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
just lint                # Lint all components (fmt-check + clippy + lint-just)
just test                # Run all tests (cargo-nextest preferred)
just build               # Build all binaries with features

# Component building and running
just run-procmond        # Run procmond (with args)
just run-daemoneye-agent   # Run daemoneye-agent (with args)
just run-daemoneye-cli     # Run daemoneye-cli (with args)

# Workspace-specific builds
cargo build -p procmond      # Build procmond crate
cargo build -p daemoneye-agent # Build daemoneye-agent crate
cargo build -p daemoneye-cli   # Build daemoneye-cli crate
cargo build -p daemoneye-lib  # Build daemoneye-lib crate

# CI aggregate (recommended for CI)
just ci-check            # Run full CI pipeline (pre-commit + lint + test + build + audit + coverage + dist)
```

Always follow the commit message style in `commit-instructions.md`.

### Testing Standards

- **Primary approach**: Integration tests with insta for snapshot testing and predicates for validation
- **Stable output**: Use `NO_COLOR=1 TERM=dumb` for CI-friendly testing
- **Async testing**: `#[tokio::test]` with tokio-test utilities
- **Performance**: Criterion benchmarks with regression detection

## Security-First Patterns

### SQL-to-IPC Architecture

- SQL detection rules are **never executed directly** against processes
- `sqlparser` extracts collection requirements from SQL AST to generate protobuf tasks
- `daemoneye-agent` translates complex SQL queries into simple collection tasks for `procmond`
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

  - Credit-based flow control: procmond grants collection credits to daemoneye-agent
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

- Dedicated Unix user/group: `daemoneye-proc:daemoneye-proc`
- Audit ledger: `open(path, O_WRONLY|O_APPEND|O_CREAT, 0600)` with owner-only write
- Drop capabilities: `CAP_SYS_ADMIN`, `CAP_SYS_PTRACE` after init, retain minimal set
- SELinux/AppArmor: Block read syscalls on audit ledger files
- Seccomp filter: Whitelist only required syscalls (write, openat, close)

**daemoneye-agent (Detection Engine):**

- Dedicated Unix user/group: `daemoneye-agent:daemoneye-agent`
- Event store: Standard R/W access with `open(path, O_RDWR)`
- Mount options: `/var/lib/daemoneye` with `nodev,nosuid,noexec`
- Network: Outbound-only via iptables rules, no listening sockets
- Filesystem ACLs: Read access to audit ledger, no write permissions

**daemoneye-cli (Query Interface):**

- Dedicated Unix user/group: `daemoneye-cli:daemoneye-cli`
- Database access: `open(path, O_RDONLY)` only
- Seccomp filter: Block write, unlink, rename syscalls on database files
- No network capabilities, filesystem read-only except for temp output files
- Group membership: Add to `daemoneye-agent` group for IPC communication only

### Input Validation

All external input must be validated with detailed error messages:

```rust
use sqlparser::{ast::Statement, dialect::SQLiteDialect, parser::Parser};

pub fn validate_detection_rule(rule: &str) -> Result<Vec<Statement>, ValidationError> {
    let dialect = SQLiteDialect {};
    let statements =
        Parser::parse_sql(&dialect, rule).map_err(|e| ValidationError::InvalidSql {
            reason: e.to_string(),
        })?;

    // Validation checks for security and complexity limits
    for statement in &statements {
        // 1. Only SELECT statements allowed
        if !matches!(statement, Statement::Query(_)) {
            return Err(ValidationError::ForbiddenStatement {
                statement_type: format!("{:?}", statement),
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
        "load_extension",
        "fts3_tokenizer",
        "zipfile",
        "fsdir",
        "readfile",
        "writefile",
        "edit",
        "shell",
    ];

    // (Omitted here – example focuses on complexity enforcement)
    Ok(())
}

fn validate_query_complexity(statement: &Statement) -> Result<(), ValidationError> {
    use sqlparser::ast::{
        Expr, Query, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins,
    };

    // Limits derived from DaemonEye security / DoS prevention policy
    const MAX_JOINS: usize = 5;
    const MAX_SUBQUERIES: usize = 3;
    const MAX_PROJECTIONS: usize = 50;

    // Helper: count joins inside a SELECT's FROM clause
    fn count_joins_in_from(from: &[TableWithJoins]) -> usize {
        from.iter().map(|twj| twj.joins.len()).sum()
    }

    // Helper: recurse through SetExpr collecting counts
    fn walk_setexpr(
        set_expr: &SetExpr,
        join_count: &mut usize,
        subquery_count: &mut usize,
        projection_count: &mut usize,
    ) {
        match set_expr {
            SetExpr::Select(boxed_select) => {
                let Select {
                    from,
                    projection,
                    selection,
                    group_by,
                    having,
                    ..
                } = &**boxed_select;
                *join_count += count_joins_in_from(from);
                *projection_count += projection.len();

                // Inspect table factors for derived tables (subqueries)
                for twj in from {
                    match &twj.relation {
                        TableFactor::Derived { subquery, .. }
                        | TableFactor::TableFunction {
                            expr: Expr::Subquery(subquery),
                            ..
                        } => {
                            *subquery_count += 1; // Count this subquery itself
                            walk_query(subquery, join_count, subquery_count, projection_count);
                        }
                        TableFactor::Table { .. }
                        | TableFactor::UNNEST { .. }
                        | TableFactor::Func { .. }
                        | TableFactor::NestedJoin { .. }
                        | TableFactor::Series { .. }
                        | TableFactor::JsonTable { .. } => {}
                        _ => {}
                    }
                }

                // Recurse into expressions that may hold subqueries
                if let Some(expr) = selection {
                    walk_expr(expr, join_count, subquery_count, projection_count);
                }
                for expr in group_by {
                    walk_expr(expr, join_count, subquery_count, projection_count);
                }
                if let Some(expr) = having {
                    walk_expr(expr, join_count, subquery_count, projection_count);
                }
                for item in projection {
                    if let SelectItem::ExprWithAlias { expr, .. } | SelectItem::UnnamedExpr(expr) =
                        item
                    {
                        walk_expr(expr, join_count, subquery_count, projection_count);
                    }
                }
            }
            SetExpr::Query(q) => {
                walk_query(q, join_count, subquery_count, projection_count);
            }
            SetExpr::SetOperation { left, right, .. } => {
                walk_setexpr(left, join_count, subquery_count, projection_count);
                walk_setexpr(right, join_count, subquery_count, projection_count);
            }
            SetExpr::Values(_) | SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Table(_) => {}
        }
    }

    // Helper: recurse into Query (which wraps a SetExpr and optional ORDER / LIMIT etc.)
    fn walk_query(
        query: &Query,
        join_count: &mut usize,
        subquery_count: &mut usize,
        projection_count: &mut usize,
    ) {
        walk_setexpr(&query.body, join_count, subquery_count, projection_count);
        // ORDER BY expressions
        for obe in &query.order_by {
            walk_expr(&obe.expr, join_count, subquery_count, projection_count);
        }
        // LIMIT / OFFSET expressions
        if let Some(limit) = &query.limit {
            walk_expr(limit, join_count, subquery_count, projection_count);
        }
        if let Some(offset) = &query.offset {
            walk_expr(&offset.value, join_count, subquery_count, projection_count);
        }
        if let Some(fetch) = &query.fetch {
            if let Some(expr) = &fetch.with_ties {
                walk_expr(expr, join_count, subquery_count, projection_count);
            }
        }
    }

    // Helper: walk expressions to discover embedded subqueries
    fn walk_expr(
        expr: &Expr,
        join_count: &mut usize,
        subquery_count: &mut usize,
        projection_count: &mut usize,
    ) {
        use sqlparser::ast::Expr::*;
        match expr {
            Subquery(q) | ScalarSubquery(q) => {
                *subquery_count += 1;
                walk_query(q, join_count, subquery_count, projection_count);
            }
            Exists {
                expr: box Subquery(q),
                ..
            } => {
                *subquery_count += 1;
                walk_query(q, join_count, subquery_count, projection_count);
            }
            InSubquery { subquery, .. } => {
                *subquery_count += 1;
                walk_query(subquery, join_count, subquery_count, projection_count);
            }
            BinaryOp { left, right, .. }
            | Like {
                expr: left,
                pattern: right,
                ..
            }
            | SimilarTo {
                expr: left,
                pattern: right,
                ..
            } => {
                walk_expr(left, join_count, subquery_count, projection_count);
                walk_expr(right, join_count, subquery_count, projection_count);
            }
            UnaryOp { expr, .. }
            | Cast { expr, .. }
            | TryCast { expr, .. }
            | Extract { expr, .. }
            | Collate { expr, .. }
            | Nested(expr) => {
                walk_expr(expr, join_count, subquery_count, projection_count);
            }
            Function(f) => {
                for arg in &f.args {
                    if let sqlparser::ast::FunctionArg::Unnamed(arg_expr) = arg {
                        if let sqlparser::ast::FunctionArgExpr::Expr(e) = arg_expr {
                            walk_expr(e, join_count, subquery_count, projection_count);
                        }
                    }
                }
            }
            Case {
                operand,
                conditions,
                results,
                else_result,
                ..
            } => {
                if let Some(op) = operand {
                    walk_expr(op, join_count, subquery_count, projection_count);
                }
                for c in conditions {
                    walk_expr(c, join_count, subquery_count, projection_count);
                }
                for r in results {
                    walk_expr(r, join_count, subquery_count, projection_count);
                }
                if let Some(er) = else_result {
                    walk_expr(er, join_count, subquery_count, projection_count);
                }
            }
            Tuple(exprs) | Array(exprs) => {
                for e in exprs {
                    walk_expr(e, join_count, subquery_count, projection_count);
                }
            }
            Between {
                expr, low, high, ..
            } => {
                walk_expr(expr, join_count, subquery_count, projection_count);
                walk_expr(low, join_count, subquery_count, projection_count);
                walk_expr(high, join_count, subquery_count, projection_count);
            }
            InList { expr, list, .. } => {
                walk_expr(expr, join_count, subquery_count, projection_count);
                for e in list {
                    walk_expr(e, join_count, subquery_count, projection_count);
                }
            }
            MapAccess { column, keys } => {
                walk_expr(column, join_count, subquery_count, projection_count);
                for k in keys {
                    walk_expr(k, join_count, subquery_count, projection_count);
                }
            }
            // Many other variants are leaf or literals; ignore safely.
            _ => {}
        }
    }

    // Only enforce for SELECT queries (we already filtered earlier) but stay defensive.
    if let Statement::Query(query) = statement {
        let mut joins = 0usize;
        let mut subs = 0usize;
        let mut projections = 0usize;
        walk_query(query, &mut joins, &mut subs, &mut projections);

        if joins > MAX_JOINS {
            return Err(ValidationError::ComplexityExceeded {
                metric: "joins".to_string(),
                limit: MAX_JOINS,
                actual: joins,
            });
        }
        if subs > MAX_SUBQUERIES {
            return Err(ValidationError::ComplexityExceeded {
                metric: "subqueries".to_string(),
                limit: MAX_SUBQUERIES,
                actual: subs,
            });
        }
        if projections > MAX_PROJECTIONS {
            return Err(ValidationError::ComplexityExceeded {
                metric: "projections".to_string(),
                limit: MAX_PROJECTIONS,
                actual: projections,
            });
        }
    }

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
- **IPC**: `interprocess` crate for cross-platform transport + protobuf for message serialization
- **Protobuf**: `prost` for code generation, CRC32 checksums for message integrity
- **Workspace**: Independent crates with shared dependencies via `workspace.dependencies`

## Documentation Standards

- **Comprehensive rustdoc** for all public APIs with examples
- **Mermaid diagrams** for architecture (Prettier ignores Markdown)
- **Relative links** for cross-references, maintain link hygiene
- See `AGENTS.md` for complete project rules and `.kiro/steering/development.md` for operational commands
