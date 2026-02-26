---
inclusion: always
---

# DaemonEye Development Guidelines

## AI Assistant Rules

### Critical Constraints

- **Never auto-commit**: Always present changes for approval before committing
- **Security-critical system**: Maintain principle of least privilege in all changes
- **Zero-warnings policy**: All code must pass `cargo clippy -- -D warnings`
- **No unsafe code**: Never use `unsafe` blocks; rely on safe external crates only
- **File size limit**: Keep source files under 500-600 lines; split larger files
- **Linter preservation**: Never remove `#[deny(...)]` or `-D warnings` without explicit permission

### Code Quality Standards

- **Testing mandatory**: All code changes require appropriate tests
- **Documentation required**: Public APIs need comprehensive rustdoc with examples
- **Performance awareness**: Consider CPU/memory impact; target \<5% CPU, \<100MB RAM
- **Error handling**: Use `thiserror` for libraries, `anyhow` for applications
- **Async patterns**: Use `tokio::sync` primitives; avoid holding locks across `.await`

## Architecture Patterns

### Component Structure

- **procmond**: Privileged collector, minimal attack surface, IPC server
- **daemoneye-agent**: User-space orchestrator, manages procmond lifecycle
- **daemoneye-cli**: Read-only interface, no network access
- **daemoneye-lib**: Shared components with trait-based abstractions

### Required Dependencies

- **Database**: `redb` for embedded storage with ACID transactions
- **IPC**: `interprocess` for cross-platform communication
- **CLI**: `clap` v4 with derive macros and shell completions
- **Async**: `tokio` with full features for I/O and task management
- **Serialization**: `serde` with `prost` for protobuf messages

### Code Organization

```rust,ignore
// Standard module structure
pub mod alerting; // Multi-channel alert delivery
pub mod config; // Configuration management
pub mod crypto; // Cryptographic audit functions
pub mod detection; // SQL-based detection engine
pub mod models; // Core data structures
pub mod storage; // Database abstractions
```

## Development Commands

### Essential Workflows

```bash
# Quality checks (run before any commit)
just lint           # Format + clippy + justfile validation
just test           # Full test suite with stable output
just build          # Build all workspace components

# Component testing
just run-procmond --once --verbose
just run-daemoneye-cli --help
just run-daemoneye-agent --config /path/to/config

# Coverage and performance
cargo llvm-cov --workspace --lcov --output-path lcov.info
cargo bench --baseline previous
```

## Coding Conventions

### Error Handling Patterns

```rust,ignore
// Libraries: Use thiserror for structured errors
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },
    #[error("Database operation failed: {0}")]
    DatabaseError(String),
}

// Applications: Use anyhow for error context
use anyhow::{Context, Result};
fn main() -> Result<()> {
    collect_processes().context("Failed to collect process data")?;
    Ok(())
}
```

### Configuration Management

```rust,ignore
// Hierarchical loading order (highest to lowest precedence):
// 1. CLI flags  2. Environment vars  3. User config  4. System config  5. Defaults
use figment::{
    Figment,
    providers::{Env, Format, Yaml},
};

#[derive(serde::Deserialize)]
struct Config {
    #[serde(with = "humantime_serde")]
    scan_interval: Duration,
    database_path: PathBuf,
}
```

### Database Patterns

```rust,ignore
// Use redb with strongly-typed tables
use redb::TableDefinition;
const PROCESSES_TABLE: TableDefinition<u64, ProcessInfo> =
    TableDefinition::new("processes");

// ACID transactions for consistency
let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_table(PROCESSES_TABLE)?;
    table.insert(process.id, &process)?;
}
write_txn.commit()?;
```

## Security Requirements

### Input Validation

```rust,ignore
// Validate all external inputs at trust boundaries
use clap::Parser;
#[derive(Parser)]
struct Args {
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=65535))]
    port: u16,
}

// SQL injection prevention with AST validation
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

fn validate_query(sql: &str) -> Result<(), SecurityError> {
    let ast = Parser::parse_sql(&SQLiteDialect {}, sql)?;
    // Only allow SELECT statements with approved functions (including AUTO JOIN)
    Ok(())
}
```

### Privilege Management

- **procmond**: Runs with elevated privileges, drops immediately after init
- **daemoneye-agent**: User-space only, outbound network connections only
- **daemoneye-cli**: No network access, read-only database operations
- Use `CAP_SYS_PTRACE` on Linux, `SeDebugPrivilege` on Windows

### Cryptographic Standards

```rust,ignore
// Use approved libraries only
use blake3::Hasher; // Fast cryptographic hashing
use ed25519_dalek::Keypair; // Digital signatures
use secrecy::SecretString; // Secure secret handling
use zeroize::Zeroize; // Memory zeroing
```

## Testing Strategy

### Test Organization

```rust,ignore
// Unit tests: Individual components only
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_collection() {
        // Test with mock data, minimal scope
    }
}

// Integration tests: Cross-component interaction
// Use insta for snapshot testing, predicates for validation
use insta::assert_snapshot;
use predicates::prelude::*;
```

### Test Execution Environment

```bash
# Stable output required for all tests
NO_COLOR=1 TERM=dumb cargo test --workspace

# Component-specific testing
cargo test -p daemoneye-lib --nocapture
cargo test -p procmond --features test-utils

# Property testing for edge cases
cargo test --features proptest
```

## Performance Guidelines

### Resource Targets

- **CPU Usage**: \<5% sustained during monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Process Enumeration**: \<5s for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: \<100ms per detection rule execution

### Optimization Patterns

```rust,ignore
// Bounded channels with backpressure
use tokio::sync::mpsc;
let (tx, rx) = mpsc::channel(1000); // Configurable capacity

// Memory budgets with cooperative yielding
if memory_usage > budget {
    tokio::task::yield_now().await;
}

// Circuit breakers for external dependencies
use circuit_breaker::CircuitBreaker;
let breaker = CircuitBreaker::new(5, Duration::from_secs(60));
```

## Documentation Requirements

### Rustdoc Standards

````rust
/// Collects process information with security validation.
///
/// # Security
/// Requires elevated privileges on startup, drops immediately after init.
///
/// # Performance
/// Targets <5s enumeration for 10,000+ processes.
///
/// # Examples
/// ```no_run
/// let collector = ProcessCollector::new()?;
/// let processes = collector.collect().await?;
/// ```
///
/// # Errors
/// Returns `CollectionError::PermissionDenied` if privileges insufficient.
pub async fn collect_processes() -> Result<Vec<ProcessInfo>, CollectionError> {
    // Implementation
}
````

### Commit Message Format

```
feat(detection): add SQL-based rule engine

- Implement AST validation with sqlparser crate
- Add sandboxed execution with resource limits
- Include comprehensive test coverage

Closes #123
```
