---
title: DaemonEye Security and Performance Findings
date: 2026-04-04
category: best-practices
module: workspace-wide
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - Working on IPC transport or Unix domain sockets
  - Changing hash/serialization formats on integrity-verified data
  - Building pub-sub or fan-out event delivery systems
  - Truncating user-controlled strings to byte limits
  - Validating configuration paths and socket addresses
tags:
  - rust
  - security-hardening
  - serialization
  - concurrency
  - ipc
  - audit-integrity
---

# DaemonEye Security and Performance Findings

## Context

A comprehensive code review of DaemonEye (Rust security monitoring tool with privilege-separated architecture) surfaced security vulnerabilities, performance bottlenecks, and correctness issues across the workspace. This documents the concrete findings and their fixes.

## Guidance

### UTF-8 Byte Slicing Causes Remote DoS (CWE-135)

`CorrelationMetadata::new()` truncated oversized correlation IDs with `correlation_id[..256]`. A crafted multi-byte string (emoji, CJK) whose 256th byte falls mid-character panics the process.

**Fix**: Include only characters that fit entirely within the byte limit:

```rust
fn truncate_to_byte_boundary(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let end = s
        .char_indices()
        .take_while(|&(byte_pos, ch)| byte_pos + ch.len_utf8() <= max_len)
        .last()
        .map_or(0, |(byte_pos, ch)| byte_pos + ch.len_utf8());
    &s[..end]
}
```

Key detail: the condition is `byte_pos + ch.len_utf8() <= max_len` (character must fit entirely), not `byte_pos < max_len` (character starts before limit but may extend past it).

### Colon-Delimited Hash Input Is Ambiguous (CWE-345)

The audit entry hash used `format!("{}:{}:{}:{}:{}:{}", seq, ts, actor, action, payload_hash, prev_hash)`. If `actor = "a:b"` and `action = "c"`, the hash input is identical to `actor = "a"` and `action = "b:c"` — enabling hash collisions that defeat tamper detection.

**Fix**: Length-prefixed canonical encoding with format versioning:

```rust
fn encode_field(field: &str) -> String {
    format!("{}:{}", field.len(), field)
}
// v2:0:26:2026-04-04T00:00:00+00:00:5:actor:6:action:5:phash:
```

The `v2:` prefix enables `verify_integrity()` to try current format first, then fall back to legacy v1 (colon-delimited with epoch seconds) for pre-upgrade entries. Without this fallback, deploying the format change breaks all existing audit ledger verification.

### Unix Socket Created World-Writable (CWE-732)

`TransportServer::new()` created a Unix domain socket without restricting permissions. Default umask typically creates `0o644` or `0o666` sockets, allowing local unprivileged users to inject or intercept IPC events.

**Fix**: Set `0o600` on the socket after creation, `0o700` on the parent directory if newly created. The other IPC transport in `daemoneye-lib` already did this correctly — the inconsistency was between two transport implementations.

### Mutex Sequence Counter Serializes All Publishes

The broker's message sequence counter was `Arc<Mutex<u64>>`. Every `publish()` call locked the mutex, serializing all event delivery even though incrementing a counter is an atomic operation.

**Fix**: `Arc<AtomicU64>` with `fetch_add(1, Ordering::Relaxed)`. Same pattern applied to statistics counters that were behind a per-event `RwLock::write()` — replaced with atomic increments, flushed to stats struct only on read.

### Deep Clone Per Subscriber (50k clones/sec)

`BusEvent` contains `String` fields and `HashMap` metadata. It was deep-cloned for each subscriber: 5 subscribers at 10k events/sec = 50k deep clones/sec.

**Fix**: Wrap once in `Arc::new(bus_event)`, deliver `Arc::clone()` to each subscriber. Subscribers receive `Arc<BusEvent>` — zero-copy fan-out.

### Socket Path Validation Too Permissive (CWE-20)

`validate_socket_path()` only checked for NUL bytes and 255-char length. Missing: directory traversal (`..` components) and the actual OS limit — Unix `sockaddr_un.sun_path` is 108 bytes including NUL terminator, so the usable limit is 107, not 255.

**Fix**: Reject `..` via `std::path::Component::ParentDir` (OS-agnostic). Enforce 107-byte limit. For general config paths, the same `Component::ParentDir` check prevents directory traversal without fragile string matching.

### Command Sanitizer Misses --flag=value Syntax

`sanitize_command_line()` split on whitespace only, so `--password=secret123` passed through as a single unsanitized token.

**Fix**: Use `split_once('=')` to parse flag-value pairs, check the flag portion against the sensitive flags list. Note: `&token[..eq_pos]` triggers `clippy::string_slice` — `split_once` is the idiomatic alternative.

### Crossbeam Spin-Wait Burns CPU When Idle

The `HighPerformanceEventBus` routing thread used `try_recv()` + `Backoff::snooze()` in a loop, consuming CPU even with no events.

**Fix**: `recv_timeout(Duration::from_millis(10))` — blocks efficiently when idle, wakes for periodic shutdown checks.

### Silent Event Type Fallback Masks Corruption

Unknown WAL event types silently defaulted to `ProcessEventType::Start`. A corrupted `Stop` event reclassified as `Start` causes stale process tracking — the security monitoring system thinks a process is still running when it has exited.

**Fix**: `from_type_string()` now returns `Result<Self, Error>`. The WAL replay loop logs and skips unrecognized entries rather than misrouting them.

## Why This Matters

These findings affect a security monitoring tool where integrity, correctness, and availability are critical:

- **Hash ambiguity** defeats the tamper-evident audit log — the core security guarantee
- **Socket permissions** allow local privilege escalation on the IPC channel
- **UTF-8 panic** enables remote DoS via the correlation ID field
- **Silent fallback** masks data corruption in the process lifecycle tracker
- **Mutex contention** and deep clones create performance bottlenecks that could cause event loss under load

## When to Apply

- Building IPC transports with Unix domain sockets — always restrict permissions
- Designing hash inputs for integrity verification — use unambiguous encoding, version the format
- Truncating strings in Rust — always use `char_indices()`, never byte indexing on user input
- Fan-out delivery systems — wrap in `Arc` once, clone the pointer
- Deserializing typed enums — return `Result`, never silently default to a variant

## Examples

### Clippy Lints Triggered by These Fixes

| Lint                     | Situation                     | Fix                                                |
| ------------------------ | ----------------------------- | -------------------------------------------------- |
| `collapsible_if`         | Nested `if let` + `if`        | Use let-chain: `if let Some(x) = y && !x.exists()` |
| `string_slice`           | `&token[..pos]` on user input | Use `split_once('=')`                              |
| `items_after_statements` | `const` after `return`        | Move all `const` to function top                   |
| `pattern_type_mismatch`  | `if let Some(x) = &option`    | Use `if let Some(ref x) = option`                  |

## Related

- GitHub issue #42: Implement Tamper-Evident Audit Logging System with BLAKE3
- GitHub issue #50: Implement Comprehensive Security Testing Framework
- `.kiro/steering/security.md`: Security validation guidelines (source of truth)
