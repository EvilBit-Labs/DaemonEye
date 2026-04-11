---
title: 'Binary Hashing P1 Blockers: Composition Root, Authorization Bypass, Serial Bottleneck, TOCTOU Race (Issue #40)'
category: security-issues
date: 2026-04-09
tags:
  - rust
  - binary-hashing
  - toctou
  - authorization
  - concurrency
  - composition-root
  - cap-std
  - newtype-pattern
  - type-state
  - cwe-135
module: procmond / daemoneye-lib (integrity) / collector-core (binary_hasher)
symptom: |
  (1) --compute-hashes flag was a silent no-op;
  (2) populate_executable_hashes hashed arbitrary paths without authorization;
  (3) executable hashing ran serially, bottlenecking collection;
  (4) BinaryHasherCollector used canonicalize() + File::open() with a TOCTOU race window.
root_cause: |-
  (1) No composition site in main.rs constructed Arc<MultiAlgorithmHasher> and injected it into collectors;
  (2) No authorization boundary at the hash engine — any path was accepted, FileChanged integrity variants passed through unchecked;
  (3) Sequential iteration over unique paths with no concurrency primitive;
  (4) canonicalize() resolves at one point in time, File::open() opens at a later point — attacker can swap target between the two calls.
---

# Binary Hashing P1 Blockers: Authorization, TOCTOU, Composition Root, Serial Bottleneck

## Problem

Four P1 block-merge defects in the binary hashing subsystem (issue #40) prevented the feature branch from merging:

1. **Todo #013 (CRITICAL)** — `--compute-hashes` was a silent no-op. No composition site constructed or injected a `MultiAlgorithmHasher`.
2. **Todo #011 (HIGH)** — `populate_executable_hashes` hashed every path without authorization checks. `HashIntegrity::FileChanged` results leaked through the `Ok` path.
3. **Todo #010 (HIGH)** — Hash pass ran serially: 300 unique executables x 50ms = 15s vs 1.9s with 8-way parallelism.
4. **Todo #002 (HIGH)** — `BinaryHasherCollector` used `canonicalize()` + `File::open()` with a TOCTOU race window between resolution and open.

## Root Cause

**#013**: `procmond/src/main.rs` parsed `--compute-hashes` but never constructed `Arc<MultiAlgorithmHasher>`. `ProcessEventSource` had no `hasher` field at all. The flag controlled nothing.

**#011**: The post-enumeration hash pass fed raw sysinfo paths directly to `hasher.compute()` with no allow-list, symlink rejection, traversal check, path length cap, or file-type validation. Additionally, `HashIntegrity::FileChanged` (mid-read mutation) was represented as a low-confidence variant in `Ok(HashResult)`, meaning any caller accessing `hash_result.hashes` bypassed the integrity tag.

**#010**: `populate_executable_hashes` iterated unique paths with sequential `.await` calls. The `Arc<Semaphore>` inside `MultiAlgorithmHasher` was designed for parallelism but unused on this code path.

**#002**: Two separate syscalls (`canonicalize` then `open`) with an exploitable window between them. An attacker controlling a process path could swap a file between the two calls.

## Solution

### Phase 1A: Type-state at the engine boundary

Deleted `HashIntegrity::FileChanged` from the `Ok` path. When `hash_sync` detects `(size, mtime)` drift between pre-read and post-read snapshots, it returns `Err(HashError::Nonauthoritative { path })`. An `Ok(HashResult)` is now always authoritative by construction.

**Files**: `daemoneye-lib/src/integrity/mod.rs`, `collector-core/src/binary_hasher.rs`

### Phase 1B: Composition root wiring

`procmond/src/main.rs` constructs exactly one `Arc<MultiAlgorithmHasher>` when `--compute-hashes` is set. Cloned into `ProcessEventSource` and `ProcmondMonitorCollector` via `with_hasher()` builders. Startup assertions prevent silent regression:

```rust
assert_eq!(
    cli.compute_hashes,
    collector.hasher().is_some(),
    "--compute-hashes={} but collector hasher={:?}; wiring broken",
    cli.compute_hashes, collector.hasher().is_some()
);
```

Integration test `hash_composition.rs` asserts `Arc::ptr_eq` across both holders.

**Files**: `procmond/src/main.rs`, `procmond/src/event_source.rs`, `procmond/src/monitor_collector.rs`

### Phase 2: Authorization layer + parallel hash pass

Created `daemoneye-lib/src/integrity/auth.rs` with shared predicates:

- `MAX_EXECUTABLE_PATH_LEN = 4096` (corrected from 107 — the prior value conflated `sockaddr_un.sun_path` with filesystem `PATH_MAX`)
- `check_path_length` — byte-length only via `as_os_str().len()`, never slices (CWE-135)
- `check_no_traversal` — rejects `..` components
- `check_regular_file`, `check_size` — pre-open gates
- `bytes_safe_display` — truncates at valid UTF-8 char boundaries using `take_while`, never string slicing

Created `procmond/src/hash_pass.rs`:

- `KernelResolvedExe` newtype with private constructor — only sysinfo's `Process::exe()` output can reach `authorize_kernel_path`
- `populate_hashes` using `futures::stream::buffer_unordered(engine.max_concurrent())`
- `HashPassStats` with `auth_failures`, `nonauthoritative`, `io_failures` counters

**Files**: `daemoneye-lib/src/integrity/auth.rs`, `procmond/src/hash_pass.rs`

### Phase 3: cap-std TOCTOU-safe opens

Added `AllowedRoot` struct to `collector-core/src/binary_hasher.rs`:

- cap-std `Dir` handle opened at construction time (before privilege drop)
- On macOS: `(st_dev, st_ino)` fingerprint for bind-mount detection
- Stores both canonical and original paths (macOS `/var` -> `/private/var`)

`authorize_confined_path` free function opens files relative to `Dir` handles — the kernel resolves the path atomically against the handle's subtree, eliminating the TOCTOU gap.

Pinned `cap-std = "=4.0.2"` and `cap-fs-ext = "=4.0.2"` in workspace.

**Files**: `collector-core/src/binary_hasher.rs`, `Cargo.toml`, `collector-core/Cargo.toml`

### Phase 4: Telemetry

`#[instrument]` span on `populate_hashes`. Info-level coverage stats emitted per pass. Startup log includes algorithm list.

## Key Gotchas

**macOS `/var` -> `/private/var`**: `std::fs::canonicalize("/var/folders/...")` returns `/private/var/folders/...`. Process paths from sysinfo may carry either form. `AllowedRoot` stores both and tries both in `strip_prefix`. Failing to handle this causes `PathNotAllowed` for all macOS temp-dir executables.

**cap-std escape detection by string match**: cap-std does not expose a typed error for escapes. The sentinel `"a path led outside of the filesystem"` is stored as `CAP_STD_ESCAPE_MESSAGE` and regression-tested. Any upstream version bump that changes this message silently breaks escape detection.

**cap-std absolute symlink rejection**: `Dir::open()` on a relative path that is an absolute symlink may be rejected because the absolute target escapes the Dir sandbox. Use relative symlinks in tests. In production, `symlink_metadata` pre-check (when `follow_symlinks=false`) catches this before the open.

**CWE-135 byte-safety**: `MAX_EXECUTABLE_PATH_LEN` is compared against `path.as_os_str().len()` (byte length). `bytes_safe_display` uses char-aware truncation. Never use `&str[..n]` indexing on paths. Prior learning documented in `docs/solutions/best-practices/rust-security-batch-cleanup-patterns-2026-04-04.md`.

**`tokio::time::timeout` cannot cancel `spawn_blocking`**: The hash engine uses an `AtomicBool` cancellation flag inside the blocking read loop. Timeout alone would let the thread continue holding its semaphore permit.

## Prevention Strategies

### Newtype authorization receipts

The most transferable pattern: a newtype with a private constructor that can only be obtained by passing through a specific gate. The downstream function accepts only the newtype, making the gate mandatory by construction.

```rust
pub struct KernelResolvedExe(PathBuf);
impl KernelResolvedExe {
    pub(crate) const fn from_sysinfo_exe(path: PathBuf) -> Self { Self(path) }
}
// hash_one() only accepts &KernelResolvedExe — cannot be called with raw &Path
```

### Startup invariant assertions

Every feature flag that controls a behavior path must have a corresponding assertion that verifies the behavioral consequence is wired, not merely that the flag was parsed.

### Handle-based filesystem access

Treat `canonicalize()` + `open()` as a code smell. Replace with `Dir` handle opened at startup; all subsequent operations are relative to the handle. The kernel enforces confinement.

### Architecture review checklist additions

1. For every feature flag: "What startup assertion verifies it is wired?"
2. For every collection with I/O: "Why is sequential acceptable, or where is `buffer_unordered`?"
3. For every `canonicalize()` call: "Is there an `open()` following it? Replace with handle-based access."
4. For every path length comparison: "Byte length or char count, and which limit applies?"
5. For every path prefix match: "Are both original and canonical forms tried?"

## Cross-References

- CWE-135 learning: `docs/solutions/best-practices/rust-security-batch-cleanup-patterns-2026-04-04.md`
- Engine TOCTOU-safe entry point: `daemoneye-lib/src/integrity/mod.rs` — `MultiAlgorithmHasher::compute_from_file`
- Cap-std TOCTOU defense module docs: `collector-core/src/binary_hasher.rs` module header
- Shared auth predicates module: `daemoneye-lib/src/integrity/auth.rs`
- Kernel-resolved hash pass: `procmond/src/hash_pass.rs`

The original in-flight plan and todo tracker used during this PR lived under `docs/plans/` and `todos/`, both of which are gitignored — consult the PR description and the commit log of issue #40 for the full historical context if you need it.
