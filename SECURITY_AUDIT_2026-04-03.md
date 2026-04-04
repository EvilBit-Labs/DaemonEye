# DaemonEye Security Audit Report

**Date**: 2026-04-03 **Scope**: Full workspace -- 6 crates (collector-core, daemoneye-agent, daemoneye-cli, daemoneye-eventbus, daemoneye-lib, procmond) **Branch**: `todo_cleanups` at commit `16db9f4` **Classification**: READ-ONLY research audit

---

## Executive Summary

DaemonEye demonstrates strong security foundations: `unsafe_code = "forbid"` at the workspace level, comprehensive clippy lint configuration, overflow checks in all profiles, pinned action SHAs in the primary CI workflow, CRC32 integrity validation on IPC frames, and AST-based SQL validation for detection rules. The project uses `cargo deny` and `cargo audit` for dependency supply chain hardening.

However, the audit identified **3 Critical**, **5 High**, **6 Medium**, and **4 Low** severity findings. The most urgent issues are: a WAL durability gap that can lose audit trail entries on crash, a UTF-8 panic in the correlation metadata constructor, and a privilege separation violation where the CLI can write to the database despite the architecture mandating read-only access.

---

## Findings

### CRITICAL-01: WAL Missing fsync -- Audit Trail Durability Gap

| Attribute    | Value                                                                                                 |
| ------------ | ----------------------------------------------------------------------------------------------------- |
| **Severity** | Critical                                                                                              |
| **CWE**      | CWE-311 (Missing Encryption of Sensitive Data), CWE-755 (Improper Handling of Exceptional Conditions) |
| **File**     | `procmond/src/wal.rs`, lines 558-619                                                                  |

**Description**: The `WriteAheadLog::write()` and `write_with_type()` methods never call `flush()` or `sync_data()` on the file handle after writing entries. Tokio's `AsyncWriteExt::write_all()` writes to the OS buffer but does not guarantee persistence to disk. On system crash or power loss, WAL entries that have been "written" but not fsynced will be lost silently.

**Attack Scenario**: An attacker who can cause a process crash (e.g., via resource exhaustion or OOM kill) immediately after a sensitive process event is detected can erase the audit evidence. The WAL claims the event was persisted (returns the sequence number), but the data never reached stable storage.

**Proof**: Grepping for `flush`, `sync_data`, and `sync_all` in `procmond/src/wal.rs` returns zero matches.

**Remediation**:

1. Add `state.file.flush().await.map_err(WalError::Io)?;` after `write_all` in both `write()` and `write_with_type()`.
2. Add `state.file.sync_data().await.map_err(WalError::Io)?;` after flush for crash durability.
3. Consider a configurable sync mode (sync-per-write vs. periodic sync) to balance durability against throughput.

---

### CRITICAL-02: UTF-8 Byte Slicing Panic in CorrelationMetadata

| Attribute    | Value                                                       |
| ------------ | ----------------------------------------------------------- |
| **Severity** | Critical                                                    |
| **CWE**      | CWE-135 (Incorrect Calculation of Multi-Byte String Length) |
| **File**     | `daemoneye-eventbus/src/message.rs`, line 36                |

**Description**: `CorrelationMetadata::new()` truncates oversized correlation IDs using byte indexing:

```rust
let bounded_id = if correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
    correlation_id[..MAX_CORRELATION_ID_LENGTH].to_string()
} else {
    correlation_id
};
```

The `String::len()` method returns byte length, not character count. If `MAX_CORRELATION_ID_LENGTH` (256) falls on a multi-byte UTF-8 boundary (e.g., within a 2-, 3-, or 4-byte character), this will panic at runtime with `byte index N is not a char boundary`.

**Attack Scenario**: An attacker sends a crafted correlation ID containing multi-byte characters (e.g., emoji or CJK characters) positioned so that byte offset 256 falls mid-character. This causes a panic in the eventbus, crashing the daemoneye-agent process. Note: the workspace lint `panic = "deny"` prevents explicit `panic!()` calls but does not prevent runtime panics from invalid byte slicing — those are a distinct class of runtime error.

**Remediation**:

1. Replace byte slicing with `correlation_id.char_indices()` to find the safe truncation point:

```rust
let bounded_id = if correlation_id.len() > MAX_CORRELATION_ID_LENGTH {
    let end = correlation_id
        .char_indices()
        .take_while(|(i, _)| *i < MAX_CORRELATION_ID_LENGTH)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    correlation_id[..end].to_owned()
} else {
    correlation_id
};
```

2. Alternatively, use `correlation_id.floor_char_boundary(MAX_CORRELATION_ID_LENGTH)` when stabilized.

---

### CRITICAL-03: Privilege Separation Violation -- CLI Has Write Access

| Attribute    | Value                                                                      |
| ------------ | -------------------------------------------------------------------------- |
| **Severity** | Critical                                                                   |
| **CWE**      | CWE-269 (Improper Privilege Management), CWE-284 (Improper Access Control) |
| **File**     | `daemoneye-cli/src/main.rs`, line 42                                       |

**Description**: The architecture document explicitly states the CLI must have **read-only** database access. However, `daemoneye-cli/src/main.rs` calls `DatabaseManager::new(&database_path)` which invokes `Database::create()` (line 180 of `storage.rs`) and `initialize_schema()` -- both are write operations.

```rust
// daemoneye-cli/src/main.rs:42
let db_manager = storage::DatabaseManager::new(&database_path)?;
```

There is no `ReadOnlyDatabaseManager` type or read-only database accessor. The `DatabaseManager` struct exposes full read/write operations (`store_process`, `store_alert`, `store_rule`, etc.) to any caller that holds a reference.

**Attack Scenario**: If the CLI binary is compromised or a bug is introduced, it could write to or corrupt the event store database. This violates the principle of least privilege and breaks the security boundary between components.

**Remediation**:

1. Create a `ReadOnlyDatabaseManager` that wraps `Database::open()` (not `create`) and only exposes read methods.
2. Change `daemoneye-cli` to use `DatabaseManager::open()` instead of `::new()`.
3. Enforce at the type level: the CLI should never have access to write methods. Consider using trait-based access control (e.g., `ReadableStorage` vs `WritableStorage` traits).

---

### HIGH-01: Missing Workspace Lint Inheritance in Two Crates

| Attribute    | Value                                                        |
| ------------ | ------------------------------------------------------------ |
| **Severity** | High                                                         |
| **CWE**      | CWE-710 (Improper Adherence to Coding Standards)             |
| **Files**    | `collector-core/Cargo.toml`, `daemoneye-eventbus/Cargo.toml` |

**Description**: The workspace root defines security-critical lint rules including `unsafe_code = "forbid"`, `panic = "deny"`, `unwrap_used = "deny"`, and `await_holding_lock = "deny"`. However, `collector-core` and `daemoneye-eventbus` do not include `[lints] workspace = true` in their `Cargo.toml` files, meaning they do not inherit these security lints.

The remaining 4 crates (`daemoneye-agent`, `daemoneye-cli`, `daemoneye-lib`, `procmond`) all properly inherit workspace lints.

**Impact**: Code in these two crates can use `unsafe`, `.unwrap()`, `.panic!()`, and hold locks across await points without any compiler-level enforcement. Since `daemoneye-eventbus` handles the IPC transport layer and `collector-core` manages process collection, these are security-critical paths.

**Remediation**: Add to both `collector-core/Cargo.toml` and `daemoneye-eventbus/Cargo.toml`:

```toml
[lints]
workspace = true
```

---

### HIGH-02: Eventbus Transport Socket Created Without Restricted Permissions

| Attribute    | Value                                                           |
| ------------ | --------------------------------------------------------------- |
| **Severity** | High                                                            |
| **CWE**      | CWE-732 (Incorrect Permission Assignment for Critical Resource) |
| **File**     | `daemoneye-eventbus/src/transport.rs`, lines 166-213            |

**Description**: `TransportServer::new()` creates a Unix domain socket via `ListenerOptions::new().name(name).create_tokio()` but never sets file permissions on the resulting socket. The default umask typically creates sockets with world-readable/writable permissions (e.g., 0o755 or 0o777).

By contrast, the `daemoneye-lib/src/ipc/interprocess_transport.rs` correctly restricts socket permissions to `0o600` (owner-only) and its parent directory to `0o700`. The eventbus transport has no such hardening.

**Attack Scenario**: A local unprivileged user on the same system can connect to the eventbus socket and inject or intercept process monitoring events, potentially injecting false positives to mask real threats or causing denial of service.

**Remediation**:

1. After creating the listener, set socket permissions to `0o600`:

```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(&socket_path, perms)?;
}
```

2. Set the parent directory to `0o700` if it is newly created.

---

### HIGH-03: Storage Layer Entirely Stubbed -- Detection Pipeline Operates on Phantom Data

| Attribute    | Value                                                          |
| ------------ | -------------------------------------------------------------- |
| **Severity** | High                                                           |
| **CWE**      | CWE-754 (Improper Check for Unusual or Exceptional Conditions) |
| **File**     | `daemoneye-lib/src/storage.rs`, lines 216-384                  |

**Description**: Every `DatabaseManager` method that should persist or retrieve data is stubbed:

- All `store_*` methods open a write transaction but do nothing and return `Ok(())`.
- All `get_*` methods open a read transaction but return `Ok(None)` or `Ok(Vec::new())`.
- `initialize_schema()` begins a transaction but creates no tables.
- `cleanup_old_data()` always returns `Ok(0)`.

This means the entire detection pipeline -- from rule storage to alert delivery -- operates on phantom data. Alerts are generated but never persisted. Process records are "stored" but immediately lost.

**Impact**: Any downstream component that relies on stored data (the detection engine, the CLI, audit trail verification) is receiving empty results, creating a false sense of security. The system appears to be monitoring but is not retaining evidence.

**Remediation**: Implement the storage operations for Task 8 as documented in the code TODOs. Until then, add explicit warnings or error returns to prevent callers from silently succeeding without actual persistence.

---

### HIGH-04: Unchecked `as` Casts in collector-core

| Attribute    | Value                                                                                                                        |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| **Severity** | High                                                                                                                         |
| **CWE**      | CWE-681 (Incorrect Conversion between Numeric Types), CWE-190 (Integer Overflow)                                             |
| **Files**    | `collector-core/src/trigger.rs`, `collector-core/src/performance.rs`, `collector-core/src/transport.rs` (multiple locations) |

**Description**: The `collector-core` crate contains numerous bare `as` casts without `#[allow(clippy::as_conversions)]` annotations or safety comments. Because `collector-core` does not inherit workspace lints (see HIGH-01), the `as_conversions = "warn"` lint is not enforced. Examples:

- `trigger.rs:1486`: `(self.max_queue_size as f32 * self.backpressure_threshold) as usize` -- if the product exceeds `usize::MAX`, this silently truncates.
- `trigger.rs:1776`: `elapsed.as_micros() as u64` -- `as_micros()` returns `u128`, truncation to `u64` is lossy for durations > 585,000 years (unlikely but contract-violating).
- `trigger.rs:2440`: `i as u32` -- loop index cast without bounds check.
- `performance.rs:490`: `(0.95 * sorted_samples.len() as f64).ceil() as usize` -- floating point to usize cast with potential NaN/infinity issues.

**Remediation**:

1. First, fix HIGH-01 to enable lint inheritance for `collector-core`.
2. Replace bare `as` casts with `u32::try_from(i).unwrap_or(0)`, `.min(u64::MAX.into())`, or `.saturating_*` methods.
3. Add `#[allow(clippy::as_conversions)]` with safety comments for intentional casts.

---

### HIGH-05: Silent Event Type Fallback to Start

| Attribute    | Value                                                |
| ------------ | ---------------------------------------------------- |
| **Severity** | High                                                 |
| **CWE**      | CWE-393 (Return of Wrong Status Code)                |
| **File**     | `procmond/src/event_bus_connector.rs`, lines 175-186 |

**Description**: The `from_type_string()` method defaults unknown event types to `Start`:

```rust
fn from_type_string(s: &str) -> Self {
    match s {
        "start" => Self::Start,
        "stop" => Self::Stop,
        "modify" => Self::Modify,
        _ => {
            warn!(event_type = s, "Unknown event type, defaulting to Start");
            Self::Start
        }
    }
}
```

A `Stop` event that is corrupted or arrives with an unknown type string will be silently reclassified as a `Start` event. This can cause the detection engine to miss process termination events, maintain stale process records, and generate false alerts about processes that no longer exist.

**Remediation**:

1. Return a `Result<Self, WalError>` or a dedicated `Unknown` variant instead of silently defaulting.
2. If backward compatibility requires a default, use a distinct `Unknown` variant that the detection engine can filter or flag separately.

---

### MEDIUM-01: EventBus Transport Authentication Is Optional and Unenforced

| Attribute    | Value                                                       |
| ------------ | ----------------------------------------------------------- |
| **Severity** | Medium                                                      |
| **CWE**      | CWE-306 (Missing Authentication for Critical Function)      |
| **File**     | `daemoneye-eventbus/src/transport.rs`, lines 62-63, 128-129 |

**Description**: `SocketConfig` has an `auth_token: Option<String>` field and `ClientConfig` has a matching `auth_token: Option<String>`, but authentication is only used during health checks (lines 758-767). The `accept()`, `send()`, and `receive()` methods do not validate any authentication token. A client can connect, send messages, and receive data without presenting any credentials.

**Remediation**: Implement mandatory token-based authentication as part of the connection handshake. Reject unauthenticated connections in `accept()`.

---

### MEDIUM-02: Audit Ledger Hash Chain Uses Timestamp Seconds -- Collision Window

| Attribute    | Value                                      |
| ------------ | ------------------------------------------ |
| **Severity** | Medium                                     |
| **CWE**      | CWE-328 (Use of Weak Hash)                 |
| **File**     | `daemoneye-lib/src/crypto.rs`, lines 72-83 |

**Description**: The audit entry hash is computed over a formatted string that includes `timestamp.timestamp()` (Unix seconds). Two audit entries created within the same second with identical actor, action, and payload will produce identical entry hashes, potentially allowing one to be substituted for the other without detection.

```rust
let entry_data = format!(
    "{}:{}:{}:{}:{}:{}",
    sequence, timestamp.timestamp(), actor, action,
    payload_hash, previous_hash.as_deref().unwrap_or("")
);
```

**Remediation**: Use `timestamp.timestamp_nanos_opt()` or `timestamp.to_rfc3339()` for sub-second precision. Alternatively, include a random nonce in the hash computation.

---

### MEDIUM-03: Merkle Tree Inclusion Proof Is Unimplemented

| Attribute    | Value                                                    |
| ------------ | -------------------------------------------------------- |
| **Severity** | Medium                                                   |
| **CWE**      | CWE-345 (Insufficient Verification of Data Authenticity) |
| **File**     | `daemoneye-lib/src/crypto.rs`, lines 127-131             |

**Description**: `AuditLedger::generate_inclusion_proof()` is documented as providing "Merkle tree-based verification with logarithmic proof sizes" but returns an empty `Vec`:

```rust
pub const fn generate_inclusion_proof(_index: usize) -> Vec<String> {
    vec![]
}
```

The `rs_merkle` crate is listed as a dependency but is never used. The audit ledger is a simple hash chain, not a Merkle tree. This means tamper detection requires verifying the entire chain (O(n)) rather than an inclusion proof (O(log n)).

**Remediation**: Either implement the Merkle tree using `rs_merkle` as designed, or update documentation to accurately reflect the current hash chain implementation.

---

### MEDIUM-04: Configuration Validation Is Minimal

| Attribute    | Value                                        |
| ------------ | -------------------------------------------- |
| **Severity** | Medium                                       |
| **CWE**      | CWE-20 (Improper Input Validation)           |
| **File**     | `daemoneye-lib/src/config.rs`, lines 663-683 |

**Description**: `validate_config()` only checks three fields:

- `scan_interval_ms != 0`
- `batch_size != 0`
- `retention_days != 0`

It does not validate:

- `database.path` for path traversal or dangerous characters
- `logging.level` for valid log level values
- `alerting.sinks[].sink_type` for known sink types
- `broker.socket_path` for path injection
- `broker.max_connections` for unreasonably high values (DoS)
- `database.max_size_mb` for unreasonably high values
- File permissions on config file itself

**Remediation**: Add comprehensive validation for all security-relevant configuration fields. Validate path strings, clamp numeric ranges, and reject unknown sink types.

---

### MEDIUM-05: Command-Line Sanitizer Does Not Handle `=` Syntax

| Attribute    | Value                                       |
| ------------ | ------------------------------------------- |
| **Severity** | Medium                                      |
| **CWE**      | CWE-200 (Exposure of Sensitive Information) |
| **File**     | `procmond/src/security.rs`, lines 371-392   |

**Description**: `sanitize_command_line()` splits on whitespace and checks if the previous token was a sensitive flag. However, it does not handle the common `--flag=value` syntax:

```text
app --password=secret123 --verbose
```

This passes through unsanitized because `--password=secret123` is a single token that does not exactly match `--password`.

**Remediation**: Add parsing for `=`-separated flag-value pairs:

```rust
} else if let Some((flag, _value)) = token.split_once('=') {
    if SENSITIVE_FLAGS.iter().any(|f| flag.eq_ignore_ascii_case(f)) {
        result.push(&format!("{flag}={REDACTED}"));
    } else {
        result.push(token);
    }
}
```

---

### MEDIUM-06: Exponential Backoff Reconnect Uses Unchecked `as` Casts

| Attribute    | Value                                                |
| ------------ | ---------------------------------------------------- |
| **Severity** | Medium                                               |
| **CWE**      | CWE-681 (Incorrect Conversion between Numeric Types) |
| **File**     | `daemoneye-eventbus/src/transport.rs`, lines 545-554 |

**Description**: The reconnection backoff calculation uses chained `as` casts through floating point:

```rust
let delay = Duration::from_millis(
    (self.config.initial_reconnect_delay.as_millis() as f64
        * self.config.backoff_multiplier
            .powi(self.reconnect_attempts as i32)) as u64,
);
```

- `as_millis()` returns `u128`, cast to `f64` loses precision for large values.
- `reconnect_attempts as i32` overflows if attempts > `i32::MAX`.
- The final `as u64` can produce `u64::MAX` from `f64::INFINITY` or `0` from `NaN`.

**Remediation**: Use `u64::try_from()` with bounds checking, or `u128::min()` before the cast. Cap `reconnect_attempts` to prevent overflow of `powi`.

---

### LOW-01: GitHub Actions Not Pinned to SHA in Secondary Workflows

| Attribute    | Value                                                                                              |
| ------------ | -------------------------------------------------------------------------------------------------- |
| **Severity** | Low                                                                                                |
| **CWE**      | CWE-829 (Inclusion of Functionality from Untrusted Control Sphere)                                 |
| **Files**    | `.github/workflows/security.yml`, `audit.yml`, `docs.yml`, `codeql.yml`, `copilot-setup-steps.yml` |

**Description**: The primary `ci.yml`, `release.yml`, and `benchmarks.yml` workflows correctly pin actions to SHA (e.g., `actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd`). However, the secondary workflows use mutable tags (e.g., `actions/checkout@v6`, `jdx/mise-action@v3`).

**Remediation**: Pin all action references to full SHA hashes across all workflows. Use Dependabot or Renovate to automatically update the SHAs.

---

### LOW-02: Default Encryption Disabled for Database

| Attribute    | Value                                          |
| ------------ | ---------------------------------------------- |
| **Severity** | Low                                            |
| **CWE**      | CWE-311 (Missing Encryption of Sensitive Data) |
| **File**     | `daemoneye-lib/src/config.rs`, line 260        |

**Description**: `DatabaseConfig::default()` sets `encryption_enabled: false`. Process metadata, executable hashes, and command lines stored in the database are sensitive. While redb does not natively support encryption, the config field suggests encryption was planned.

**Remediation**: Document the encryption roadmap. If encryption is not yet supported, either remove the field to avoid false expectations or implement it using an encryption layer over the redb file (e.g., filesystem-level encryption guidance in deployment docs).

---

### LOW-03: Eventbus Socket Path Uses `/tmp` by Default

| Attribute    | Value                                                                       |
| ------------ | --------------------------------------------------------------------------- |
| **Severity** | Low                                                                         |
| **CWE**      | CWE-379 (Creation of Temporary File in Directory with Insecure Permissions) |
| **File**     | `daemoneye-eventbus/src/transport.rs`, line 77                              |

**Description**: `SocketConfig::new()` constructs the default socket path as `/tmp/daemoneye-{instance_id}.sock`. The `/tmp` directory is world-writable, and without proper socket permissions (see HIGH-02), any local user can interact with the socket.

**Remediation**: Use a dedicated runtime directory (e.g., `/var/run/daemoneye/`) with restricted permissions, consistent with how `daemoneye-lib` handles IPC paths.

---

### LOW-04: `AuditLedger` Is Not Persistent

| Attribute    | Value                                           |
| ------------ | ----------------------------------------------- |
| **Severity** | Low                                             |
| **CWE**      | CWE-404 (Improper Resource Shutdown or Release) |
| **File**     | `daemoneye-lib/src/crypto.rs`, lines 101-104    |

**Description**: `AuditLedger` stores entries in an in-memory `Vec<AuditEntry>`. There is no serialization, persistence, or recovery mechanism. If the process restarts, the entire audit ledger is lost.

**Remediation**: Implement persistence to the WAL or database. The `verify_integrity()` method is useful but only works on the in-memory state.

---

## Positive Security Observations

01. **`unsafe_code = "forbid"`** at workspace level with `overflow-checks = true` in all profiles (dev, release, dist).
02. **AST-based SQL validation** using `sqlparser` with banned function lists, statement type restrictions (SELECT only), and structural limits (max joins, max columns).
03. **CRC32 integrity validation** on IPC frames with size limits and timeout enforcement.
04. **WAL file permissions** are correctly restricted to `0o600` with TOCTOU mitigation via `OpenOptions::mode()`.
05. **Comprehensive clippy lint configuration** including `unwrap_used = "deny"`, `panic = "deny"`, `await_holding_lock = "deny"`.
06. **`cargo deny`** configuration bans `openssl` in favor of `rustls`, denies yanked crates, and restricts to known registries.
07. **Pinned action SHAs** in primary CI workflow (`ci.yml`, `release.yml`, `benchmarks.yml`).
08. **Data sanitization** for command lines, environment variables, and file paths with redaction of sensitive patterns.
09. **No `unsafe` code** across the entire workspace (enforced by `forbid`).
10. **Socket permissions hardened** on the IPC transport layer in `daemoneye-lib` (0o600 socket, 0o700 directory).

---

## Summary Table

| ID          | Severity | CWE         | Component                          | Finding                                                            |
| ----------- | -------- | ----------- | ---------------------------------- | ------------------------------------------------------------------ |
| CRITICAL-01 | Critical | CWE-311/755 | procmond/wal.rs                    | WAL missing fsync -- audit trail data loss on crash                |
| CRITICAL-02 | Critical | CWE-135     | daemoneye-eventbus/message.rs      | UTF-8 byte slicing panic in CorrelationMetadata                    |
| CRITICAL-03 | Critical | CWE-269/284 | daemoneye-cli/main.rs              | CLI has write access to database (architecture requires read-only) |
| HIGH-01     | High     | CWE-710     | collector-core, daemoneye-eventbus | Missing `[lints] workspace = true` -- security lints unenforced    |
| HIGH-02     | High     | CWE-732     | daemoneye-eventbus/transport.rs    | Eventbus socket created without restricted permissions             |
| HIGH-03     | High     | CWE-754     | daemoneye-lib/storage.rs           | All storage methods stubbed -- detection pipeline on phantom data  |
| HIGH-04     | High     | CWE-681/190 | collector-core (multiple files)    | Unchecked `as` casts in security-critical arithmetic               |
| HIGH-05     | High     | CWE-393     | procmond/event_bus_connector.rs    | Unknown event types silently default to Start                      |
| MEDIUM-01   | Medium   | CWE-306     | daemoneye-eventbus/transport.rs    | Authentication optional and unenforced on IPC connections          |
| MEDIUM-02   | Medium   | CWE-328     | daemoneye-lib/crypto.rs            | Audit hash uses second-precision timestamps -- collision window    |
| MEDIUM-03   | Medium   | CWE-345     | daemoneye-lib/crypto.rs            | Merkle tree inclusion proof unimplemented (returns empty vec)      |
| MEDIUM-04   | Medium   | CWE-20      | daemoneye-lib/config.rs            | Configuration validation is minimal -- missing path/range checks   |
| MEDIUM-05   | Medium   | CWE-200     | procmond/security.rs               | Command sanitizer misses `--flag=value` syntax                     |
| MEDIUM-06   | Medium   | CWE-681     | daemoneye-eventbus/transport.rs    | Unchecked `as` casts in exponential backoff calculation            |
| LOW-01      | Low      | CWE-829     | .github/workflows/                 | GitHub Actions not pinned to SHA in 5 secondary workflows          |
| LOW-02      | Low      | CWE-311     | daemoneye-lib/config.rs            | Database encryption disabled by default                            |
| LOW-03      | Low      | CWE-379     | daemoneye-eventbus/transport.rs    | Eventbus socket defaults to `/tmp`                                 |
| LOW-04      | Low      | CWE-404     | daemoneye-lib/crypto.rs            | AuditLedger is memory-only with no persistence                     |

---

## Recommended Prioritization

**Immediate (before next release)**:

1. CRITICAL-02 -- Fix UTF-8 panic (single-line fix, prevents DoS)
2. HIGH-01 -- Add `[lints] workspace = true` to 2 crates (trivial fix, high impact)
3. HIGH-02 -- Set socket permissions on eventbus transport

**Short-term (next sprint)**: 4. CRITICAL-01 -- Add fsync to WAL writes 5. CRITICAL-03 -- Implement read-only database accessor for CLI 6. HIGH-05 -- Replace silent event type fallback with error/Unknown variant 7. MEDIUM-05 -- Fix `--flag=value` sanitization gap

**Medium-term (next milestone)**: 8. HIGH-03 -- Implement storage layer (Task 8) 9. HIGH-04 -- Audit and fix all bare `as` casts in collector-core 10. MEDIUM-01 through MEDIUM-06 -- Remaining medium findings 11. LOW-01 through LOW-04 -- Low severity items
