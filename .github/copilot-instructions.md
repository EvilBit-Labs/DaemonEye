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
}
```

### Service Layer Pattern

Implement trait-based services for clear boundaries:

```rust
#[async_trait]
pub trait ProcessCollectionService: Send + Sync {
    async fn collect_processes(&self) -> Result<CollectionResult, CollectionError>;
}
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

### Privilege Separation

- `procmond`: Write-only audit ledger, IPC server for simple protobuf tasks
- `sentinelagent`: Read/write event store, translates SQL rules to protobuf
- `sentinelcli`: Read-only database access, no network

### Input Validation

All external input must be validated with detailed error messages:

```rust
pub async fn validate_detection_rule(rule: &str) -> Result<ParsedRule, ValidationError> {
    sqlparser::parser::Parser::parse_sql(&dialect, rule)
        .map_err(|e| ValidationError::InvalidSql { reason: e.to_string() })?;
}
```

## Key Dependencies & Patterns

- **Database**: SQLx with SQLite for event storage + audit ledger separation
- **CLI**: clap v4 with derive macros, support `--json` output + `NO_COLOR`
- **Async**: Tokio runtime with structured logging via `tracing`
- **Process enumeration**: `sysinfo` crate with platform-specific optimizations
- **IPC**: Custom protobuf over Unix sockets/named pipes between components

## Documentation Standards

- **Comprehensive rustdoc** for all public APIs with examples
- **Mermaid diagrams** for architecture (Prettier ignores Markdown)
- **Relative links** for cross-references, maintain link hygiene
- See `AGENTS.md` for complete project rules and `WARP.md` for operational commands
