//! Cryptographic integrity verification (binary hashing).
//!
//! Streaming multi-algorithm hash engine for executables and critical system
//! files. Produces a `HashResult` with cryptographically secure hashes
//! (SHA-256, BLAKE3 by default; SHA-3-256 behind `sha3-hashes`).
//!
//! # Design
//!
//! - **Streaming reads**: 256 KiB buffer, bounded memory regardless of file size.
//! - **Cooperative cancellation**: deadlines enforced via an `AtomicBool` flag
//!   checked inside the read loop. `tokio::time::timeout` alone cannot cancel
//!   `spawn_blocking` tasks — the blocking thread would keep running and hold
//!   its semaphore permit until the loop completes naturally. See
//!   <https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html>.
//! - **Size-threshold dispatch**: files smaller than `SPAWN_BLOCKING_THRESHOLD`
//!   are hashed inline on the current task (pure CPU, ~microseconds). Larger
//!   files go through `tokio::task::spawn_blocking`. This mirrors how
//!   `tokio::fs` makes the same tradeoff internally.
//! - **Bounded concurrency**: an `Arc<Semaphore>` sized from
//!   `std::thread::available_parallelism()` (clamped to `[2, 16]`) caps
//!   simultaneous hash operations across all callers. Both the inline
//!   enumeration path and the on-demand triggered path share a single engine
//!   instance so the combined load always respects the cap.
//! - **TOCTOU tagging at the engine boundary**: `(size, mtime)` are captured
//!   before and after the read. If they drift, the engine returns
//!   `HashError::Nonauthoritative` — a mid-read mutation is forensic
//!   evidence, not a successful hash. Callers that need detection signal
//!   should observe the `Nonauthoritative` error path specifically; callers
//!   that just want a usable hash get type-state safety because
//!   `HashResult` is unreachable in the error case.
//! - **Shared cache**: a `quick_cache::sync::Cache` keyed by
//!   `(PathBuf, SystemTime, u64)` is optionally held by the engine so that
//!   both the inline enumeration path (procmond) and the on-demand triggered
//!   path (`BinaryHasherCollector`) observe a consistent view of a file
//!   snapshot. Cross-path cache hits are expected to be rare in steady state;
//!   the primary value of sharing the engine is **single policy** (one
//!   concurrency cap, one algorithm list, one size limit), not throughput.
//!
//! # Algorithm policy
//!
//! - **Default**: SHA-256 + BLAKE3 (both cryptographically secure).
//! - **Opt-in via feature flags**: SHA-3-256 (`sha3-hashes`).
//! - **Deliberately unsupported**: SHA-1 and MD5. Per `DaemonEye`'s
//!   cryptographic standards (see `AGENTS.md` — "never SHA-1"), weak hashes
//!   are not compiled into the binary under any feature flag. Downstream
//!   code must still use `HashAlgorithm::is_cryptographically_secure` to
//!   gate trust decisions, so the API remains forward-compatible if a
//!   future secure algorithm is ever added.
//!
//! # Example
//!
//! ```no_run
//! use daemoneye_lib::integrity::{
//!     HashAlgorithm, HashComputer, HasherConfig, MultiAlgorithmHasher,
//! };
//! use std::path::Path;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let hasher = MultiAlgorithmHasher::new(HasherConfig::default())?;
//! let result = hasher.compute(Path::new("/bin/ls")).await?;
//! let sha256_hex = result.hashes.get(&HashAlgorithm::Sha256).ok_or("no sha256")?;
//! println!("sha256 = {sha256_hex}");
//! # Ok(())
//! # }
//! ```

pub mod auth;

use quick_cache::sync::Cache;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use sha2::{Digest as Sha2Digest, Sha256};

#[cfg(feature = "sha3-hashes")]
use sha3::Sha3_256;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Streaming read buffer size (256 KiB).
///
/// Aligns with Linux readahead defaults, macOS APFS I/O units, and Windows
/// NTFS sequential-scan behavior. Large enough to keep BLAKE3 SIMD paths
/// saturated; small enough that 16 concurrent operations stay well under
/// the 100 MiB workspace memory budget.
pub const BUFFER_SIZE: usize = 256 * 1024;

/// Size threshold below which files are hashed inline.
///
/// Files smaller than this are hashed on the current async task (pure CPU,
/// ~microseconds, no blocking-pool round-trip). Files at or above are routed
/// through `tokio::task::spawn_blocking`. Matches the heuristic that
/// `tokio::fs` uses internally for its own I/O dispatch decisions.
pub const SPAWN_BLOCKING_THRESHOLD: u64 = 256 * 1024;

/// Default maximum file size accepted for hashing (512 MiB). Larger files are
/// rejected with [`HashError::FileTooLarge`] to bound both memory and time.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// Default per-file hash deadline (10 seconds).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default bounded-cache capacity in entries.
pub const DEFAULT_CACHE_CAPACITY: usize = 10_000;

/// Lower bound on `max_concurrent`. Even on 1-vCPU hosts we allow 2 concurrent
/// operations so the triggered path isn't starved by the inline path.
pub const MIN_CONCURRENCY: usize = 2;

/// Upper bound on `max_concurrent`. I/O queue depth rarely benefits from more
/// than ~16 parallel hash operations on commodity storage.
pub const MAX_CONCURRENCY: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// HashAlgorithm
// ─────────────────────────────────────────────────────────────────────────────

/// Supported cryptographic hash algorithms.
///
/// **ORDER IS LOAD-BEARING**: `HashAlgorithm: Ord` derives its ordering from
/// the discriminant order, and [`HashResult::hashes`] is a `BTreeMap` whose
/// serialization depends on that ordering. Snapshot fixtures and the audit
/// ledger's BLAKE3 hash chain indirectly depend on stable serialization.
/// **Never reorder variants and never change an existing discriminant
/// value.** Appending new variants at the end with a fresh discriminant is
/// fine. Explicit discriminants make the ordering immutable even if someone
/// accidentally reshuffles the source lines.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum HashAlgorithm {
    /// SHA-256 — NIST FIPS 180-4. Cryptographically secure. Primary integrity
    /// hash; the default value stored in
    /// [`crate::models::process::ProcessRecord::executable_hash`].
    Sha256 = 0,
    /// BLAKE3 — modern, very fast, cryptographically secure. Enabled by
    /// default; already a workspace dependency via the audit ledger chain.
    Blake3 = 1,
    /// SHA3-256 — NIST FIPS 202. Cryptographically secure. 5–10× slower than
    /// SHA-256 on CPUs with SHA-NI. Behind the `sha3-hashes` feature.
    #[cfg(feature = "sha3-hashes")]
    Sha3_256 = 2,
}

impl HashAlgorithm {
    /// Whether this algorithm may be used to make trust or integrity
    /// decisions. All currently-supported variants return `true`; the method
    /// is kept so downstream code gates trust behind an explicit check and
    /// so the return value can flip to `false` without an API break if a
    /// weak algorithm is ever (re-)introduced.
    #[must_use]
    pub const fn is_cryptographically_secure(self) -> bool {
        #[cfg_attr(not(feature = "sha3-hashes"), allow(clippy::match_same_arms))]
        match self {
            Self::Sha256 => true,
            Self::Blake3 => true,
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256 => true,
        }
    }

    /// Canonical lowercase wire name used in the protobuf `hash_algorithm`
    /// field at `daemoneye-lib/proto/common.proto`.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Blake3 => "blake3",
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256 => "sha3-256",
        }
    }

    /// Output length in hex characters. Useful for test vector assertions.
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Blake3 => 64,
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256 => 64,
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HashIntegrity
// ─────────────────────────────────────────────────────────────────────────────

/// Integrity tag on a successful [`HashResult`].
///
/// # Single-variant design (intentional)
///
/// After the Nonauthoritative type-state refactor (2026-04-09) this enum
/// intentionally has exactly one variant. The former `FileChanged` variant has
/// been removed and its semantics moved to [`HashError::Nonauthoritative`]:
/// mid-read mutations are now surfaced as an `Err` at the engine boundary
/// rather than flowing through the `Ok` path as a degraded integrity tag.
/// This means every `Ok(HashResult)` is [`HashIntegrity::Stable`] by
/// construction — callers no longer need to inspect the tag to decide whether
/// bytes are trustworthy.
///
/// # Forward-compatibility extension point
///
/// The enum is deliberately kept (rather than replaced by a unit struct or
/// removed entirely) to preserve room for future non-failure integrity
/// annotations — for example, a `SnapshotVerified` or `MerkleProofed` tag —
/// without requiring a breaking API change. `#[non_exhaustive]` reinforces
/// this intent: downstream `match` arms **must** include a wildcard arm so
/// that adding new variants here never breaks consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum HashIntegrity {
    /// File metadata was consistent before and after the hash read.
    Stable,
}

// ─────────────────────────────────────────────────────────────────────────────
// HashResult
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a hashing operation. Immutable value type.
///
/// Only cryptographically secure hashes are ever populated in [`Self::hashes`].
/// SHA-1 and MD5 are deliberately not supported by the engine, so the hash
/// map can be trusted at the type level to hold only algorithms approved by
/// `DaemonEye`'s cryptographic standards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashResult {
    /// Canonicalized file path (post-open, not raw input). May differ from
    /// the caller's request if the caller passed a relative path.
    pub file_path: PathBuf,
    /// File size in bytes as observed at open time.
    pub file_size: u64,
    /// File modification time as observed at open time.
    pub modified_time: SystemTime,
    /// Cryptographically secure hashes, hex-encoded. Safe for lifecycle
    /// diffing, detection rule evaluation, and audit ledger inclusion.
    pub hashes: BTreeMap<HashAlgorithm, String>,
    /// Integrity tag. Always [`HashIntegrity::Stable`] in the `Ok` path —
    /// mid-read mutations are returned as [`HashError::Nonauthoritative`]
    /// at the engine boundary, so a successful `HashResult` cannot carry
    /// non-authoritative bytes.
    pub integrity: HashIntegrity,
    /// Wall-clock time spent computing the hash.
    pub computation_time: Duration,
}

impl HashResult {
    /// Returns the SHA-256 hex string if present. Convenience for the
    /// inline enumeration path, which stores only SHA-256 in the protobuf
    /// `executable_hash` field.
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.hashes.get(&HashAlgorithm::Sha256).map(String::as_str)
    }

    /// Returns the BLAKE3 hex string if present.
    #[must_use]
    pub fn blake3(&self) -> Option<&str> {
        self.hashes.get(&HashAlgorithm::Blake3).map(String::as_str)
    }

    /// Whether the result is safe to use for integrity or lifecycle diff
    /// decisions. Returns `false` when the file mutated during the read.
    #[must_use]
    pub const fn is_authoritative(&self) -> bool {
        matches!(self.integrity, HashIntegrity::Stable)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HashError
// ─────────────────────────────────────────────────────────────────────────────

/// Errors returned by [`HashComputer::compute`] and related methods.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HashError {
    /// The filesystem refused to open or read the file.
    #[error("permission denied: {path}")]
    PermissionDenied {
        /// Path that could not be opened.
        path: PathBuf,
    },
    /// The file did not exist at open time.
    #[error("file not found: {path}")]
    FileNotFound {
        /// Path that could not be opened.
        path: PathBuf,
    },
    /// The file exceeded the configured maximum size.
    #[error("file too large: {size} bytes exceeds limit of {limit} bytes")]
    FileTooLarge {
        /// Observed file size.
        size: u64,
        /// Configured limit.
        limit: u64,
    },
    /// The hash operation exceeded its deadline. Cooperative cancellation
    /// detected the timeout on its next read-loop iteration.
    #[error("hash operation timed out")]
    Timeout,
    /// The hash operation was cancelled by the caller.
    #[error("hash operation cancelled")]
    Cancelled,
    /// A lower-level I/O error occurred during read.
    #[error("I/O error hashing {path}: {source}")]
    Io {
        /// Path being read when the error occurred.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// `spawn_blocking` task panicked or was cancelled externally.
    #[error("blocking task join failed: {0}")]
    Join(String),
    /// Caller supplied a configuration that could not be satisfied.
    #[error("invalid hasher configuration: {0}")]
    InvalidConfig(String),
    /// The file was mutated between the open-time and close-time metadata
    /// snapshots. The hashed bytes are a mid-read sample and are
    /// **not authoritative** — they must never be trusted for integrity
    /// decisions or stamped into a lifecycle diff. Receiving this error is
    /// a detection signal: a binary was modified while being hashed.
    ///
    /// This variant is returned by [`HashComputer::compute`] at the engine
    /// boundary (not by a display-layer accessor) so type-state guarantees
    /// that a successful `Ok(HashResult)` always carries stable bytes.
    #[error("file mutated during hash: {path}")]
    Nonauthoritative {
        /// The path that was being hashed when the mid-read mutation was
        /// detected.
        path: PathBuf,
    },
}

impl HashError {
    /// Maps an `io::Error` at a specific path into a structured `HashError`.
    ///
    /// Only `NotFound` and `PermissionDenied` get dedicated variants — every
    /// other `ErrorKind` is bundled into [`HashError::Io`]. The wildcard arm
    /// is deliberate: `io::ErrorKind` is `#[non_exhaustive]` upstream and
    /// holds dozens of variants we do not want to treat individually. The
    /// upstream `#[non_exhaustive]` attribute also means future kinds will
    /// flow through this arm automatically without a silent regression.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "io::ErrorKind is #[non_exhaustive] and has many irrelevant variants"
    )]
    fn from_io(path: &Path, err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::FileNotFound {
                path: path.to_path_buf(),
            },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied {
                path: path.to_path_buf(),
            },
            _ => Self::Io {
                path: path.to_path_buf(),
                source: err,
            },
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HasherConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`MultiAlgorithmHasher`].
///
/// All fields have sensible defaults via [`Default`]. Callers may override
/// with the `with_*` builder methods, which return `Self` by value.
#[derive(Debug, Clone)]
pub struct HasherConfig {
    /// Algorithms to compute for every hash request. SHA-256 is always
    /// included even if absent from this list. BLAKE3 is included by default
    /// in [`HasherConfig::default`].
    pub algorithms: Vec<HashAlgorithm>,
    /// Maximum number of concurrent hash operations across all callers.
    /// Defaults to `clamp(available_parallelism(), 2, 16)`.
    pub max_concurrent: usize,
    /// Maximum accepted file size. Files larger than this are rejected with
    /// [`HashError::FileTooLarge`] before any bytes are read.
    pub max_file_size: u64,
    /// Per-file hash deadline. Enforced cooperatively via an `AtomicBool`
    /// cancel flag checked inside the streaming read loop.
    pub timeout_per_file: Duration,
    /// Streaming read buffer size. Defaults to [`BUFFER_SIZE`].
    pub buffer_size: usize,
    /// Bounded-cache capacity in entries. Zero disables the cache.
    pub cache_capacity: usize,
}

impl Default for HasherConfig {
    fn default() -> Self {
        let max_concurrent = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(MIN_CONCURRENCY)
            .clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
        Self {
            algorithms: vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3],
            max_concurrent,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            timeout_per_file: DEFAULT_TIMEOUT,
            buffer_size: BUFFER_SIZE,
            cache_capacity: DEFAULT_CACHE_CAPACITY,
        }
    }
}

impl HasherConfig {
    /// Builder: replace the algorithm list entirely.
    #[must_use]
    pub fn with_algorithms(mut self, algorithms: Vec<HashAlgorithm>) -> Self {
        self.algorithms = algorithms;
        self
    }

    /// Builder: set `max_concurrent`, clamped to `[MIN_CONCURRENCY, MAX_CONCURRENCY]`.
    #[must_use]
    pub fn with_max_concurrent(mut self, n: usize) -> Self {
        self.max_concurrent = n.clamp(MIN_CONCURRENCY, MAX_CONCURRENCY);
        self
    }

    /// Builder: set the maximum file size accepted for hashing.
    #[must_use]
    pub const fn with_max_file_size(mut self, bytes: u64) -> Self {
        self.max_file_size = bytes;
        self
    }

    /// Builder: set the per-file timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_per_file = timeout;
        self
    }

    /// Builder: set the bounded cache capacity (0 = disabled).
    #[must_use]
    pub const fn with_cache_capacity(mut self, n: usize) -> Self {
        self.cache_capacity = n;
        self
    }

    /// Validate the configuration, returning a descriptive error on failure.
    ///
    /// # Errors
    ///
    /// Returns [`HashError::InvalidConfig`] if any field is out of range.
    pub fn validate(&self) -> Result<(), HashError> {
        if self.algorithms.is_empty() {
            return Err(HashError::InvalidConfig(
                "algorithms list must not be empty".to_owned(),
            ));
        }
        if self.max_concurrent == 0 {
            return Err(HashError::InvalidConfig(
                "max_concurrent must be greater than zero".to_owned(),
            ));
        }
        if self.max_file_size == 0 {
            return Err(HashError::InvalidConfig(
                "max_file_size must be greater than zero".to_owned(),
            ));
        }
        if self.buffer_size == 0 {
            return Err(HashError::InvalidConfig(
                "buffer_size must be greater than zero".to_owned(),
            ));
        }
        if self.timeout_per_file.is_zero() {
            return Err(HashError::InvalidConfig(
                "timeout_per_file must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HashComputer trait
// ─────────────────────────────────────────────────────────────────────────────

/// Abstract interface for a streaming, multi-algorithm hash engine.
///
/// Implementations must be `Send + Sync` so a single engine can be shared via
/// `Arc` between the inline enumeration path and the on-demand triggered
/// collector.
///
/// This trait uses native `async fn` + `#[allow(async_fn_in_trait)]` per
/// workspace convention (see `EventSource` at `collector-core/src/source.rs`).
#[allow(async_fn_in_trait)]
pub trait HashComputer: Send + Sync {
    /// Compute hashes for the file at `path`, respecting the configured
    /// deadline, concurrency cap, and file-size limit.
    ///
    /// # Errors
    ///
    /// Returns [`HashError`] on I/O failure, timeout, cancellation,
    /// oversized files, or permission errors.
    async fn compute(&self, path: &Path) -> Result<HashResult, HashError>;

    /// Return the algorithms this computer will emit on every successful
    /// `compute` call (excluding legacy algorithms which land in a separate
    /// bucket on `HashResult`).
    fn supported_algorithms(&self) -> &[HashAlgorithm];

    /// Optional health check. The default implementation is a no-op.
    ///
    /// # Errors
    ///
    /// Implementations may return [`HashError`] to signal unhealthy state.
    async fn health_check(&self) -> Result<(), HashError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Enum-dispatched multi-hasher
// ─────────────────────────────────────────────────────────────────────────────

/// Enum wrapper over the concrete per-algorithm hashers.
///
/// We deliberately do **not** use `Box<dyn DynDigest>` here: enabling BLAKE3's
/// `traits-preview` feature to expose `digest::Digest` creates method-resolution
/// conflicts with existing `sha2::Digest` usage elsewhere in the workspace.
/// Enum dispatch is strictly simpler, avoids the conflict, and has negligible
/// overhead (one match per 256 KiB chunk).
enum HasherKind {
    Sha256(Box<Sha256>),
    Blake3(Box<blake3::Hasher>),
    #[cfg(feature = "sha3-hashes")]
    Sha3_256(Box<Sha3_256>),
}

impl HasherKind {
    fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Sha256 => Self::Sha256(Box::new(Sha256::new())),
            HashAlgorithm::Blake3 => Self::Blake3(Box::new(blake3::Hasher::new())),
            #[cfg(feature = "sha3-hashes")]
            HashAlgorithm::Sha3_256 => Self::Sha3_256(Box::new(Sha3_256::new())),
        }
    }

    const fn algorithm(&self) -> HashAlgorithm {
        match *self {
            Self::Sha256(_) => HashAlgorithm::Sha256,
            Self::Blake3(_) => HashAlgorithm::Blake3,
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256(_) => HashAlgorithm::Sha3_256,
        }
    }

    fn update(&mut self, data: &[u8]) {
        match *self {
            Self::Sha256(ref mut h) => Sha2Digest::update(h.as_mut(), data),
            Self::Blake3(ref mut h) => {
                // Inherent method returns &mut Self for chaining; we discard.
                let _ = h.update(data);
            }
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256(ref mut h) => {
                use sha3::Digest;
                h.as_mut().update(data);
            }
        }
    }

    fn finalize_hex(self) -> String {
        match self {
            Self::Sha256(h) => bytes_to_hex(Sha2Digest::finalize(*h).as_slice()),
            Self::Blake3(h) => h.finalize().to_hex().to_string(),
            #[cfg(feature = "sha3-hashes")]
            Self::Sha3_256(h) => {
                use sha3::Digest;
                bytes_to_hex((*h).finalize().as_slice())
            }
        }
    }
}

/// Encode a byte slice as a lowercase hex string.
///
/// Used instead of `format!("{:x}", ...)` because sha2 0.11 returns
/// `hybrid_array::Array<u8, ...>` which does not implement `LowerHex`.
fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &b in bytes {
        // u8 >> 4 and u8 & 0x0f are both in [0, 16), so indexing a 16-element
        // array is guaranteed in-bounds at the type level.
        let hi = (b >> 4) & 0x0f;
        let lo = b & 0x0f;
        // Debug-only invariant check: both nibbles must be < 16 so that the
        // get() calls below never hit the fallback branch.
        debug_assert!(
            usize::from(hi) < HEX_CHARS.len() && usize::from(lo) < HEX_CHARS.len(),
            "hex nibble out of range"
        );
        // `HEX_CHARS[idx]` where idx < 16 cannot panic; use get() + fallback
        // for belt-and-braces under clippy::indexing_slicing = "warn".
        out.push(char::from(*HEX_CHARS.get(usize::from(hi)).unwrap_or(&b'0')));
        out.push(char::from(*HEX_CHARS.get(usize::from(lo)).unwrap_or(&b'0')));
    }
    out
}

/// Fan-out structure holding one hasher per configured algorithm.
struct HasherSet {
    hashers: Vec<HasherKind>,
}

impl HasherSet {
    fn new(algorithms: &[HashAlgorithm]) -> Self {
        let mut seen: Vec<HashAlgorithm> = Vec::with_capacity(algorithms.len().saturating_add(1));
        // SHA-256 is always present.
        seen.push(HashAlgorithm::Sha256);
        for a in algorithms {
            if !seen.contains(a) {
                seen.push(*a);
            }
        }
        Self {
            hashers: seen.into_iter().map(HasherKind::new).collect(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        for h in &mut self.hashers {
            h.update(data);
        }
    }

    fn finalize_into(self) -> BTreeMap<HashAlgorithm, String> {
        let mut secure: BTreeMap<HashAlgorithm, String> = BTreeMap::new();
        for h in self.hashers {
            let algo = h.algorithm();
            // All engine-supported algorithms are cryptographically secure
            // by construction (SHA-1 and MD5 are not compiled in). We still
            // assert it at runtime as a defence-in-depth invariant so any
            // future addition of a weak algorithm to the engine would trip
            // this check before producing a trusted-looking result.
            debug_assert!(
                algo.is_cryptographically_secure(),
                "HasherSet must only produce cryptographically secure hashes"
            );
            let hex = h.finalize_hex();
            let _ = secure.insert(algo, hex);
        }
        secure
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MultiAlgorithmHasher
// ─────────────────────────────────────────────────────────────────────────────

// NOTE: cache key uses (path, modified_time, file_size). SystemTime
// resolution is platform-dependent — nanoseconds on Linux and APFS,
// 100ns on Windows, but historically 1s on HFS+. On a 1s-resolution
// filesystem, two writes to the same file within one second with
// identical size could return a stale cached hash. APFS has been the
// default on macOS since 10.13, so this is low risk for executables
// in practice, but callers that hash short-lived intermediates on
// older filesystems should disable the cache (cache_capacity = 0).
type CacheKey = (PathBuf, SystemTime, u64);

/// The default [`HashComputer`] implementation.
///
/// Single instance intended to be held behind `Arc` and shared between the
/// inline enumeration path (procmond) and the on-demand triggered path
/// (`BinaryHasherCollector`). Sharing the same instance guarantees **single
/// policy**: one concurrency cap, one algorithm list, one size/timeout
/// contract — no matter which caller invokes `compute`. Cross-path cache
/// hits are a small side benefit in steady state but are not the primary
/// motivation.
///
/// # Statelessness invariant
///
/// `MultiAlgorithmHasher` holds **no per-path state that bleeds between
/// callers**. The bounded cache is keyed by `(path, mtime, size)` triples
/// and is monotonic — a lookup never depends on who inserted the entry.
/// No telemetry, rate limiter, or error log is keyed by path at the engine
/// level. This matters because the same `Arc<MultiAlgorithmHasher>` backs
/// both the kernel-sourced path (procmond process enumeration) and the
/// untrusted triggered path (rule-initiated hash requests). If the engine
/// were stateful across calls, a triggered-path caller could probe for
/// sensitive file existence via timing side-channels on the shared `Arc`.
/// Any future addition to this struct must preserve the invariant or scope
/// its state per-call via an explicit context argument.
pub struct MultiAlgorithmHasher {
    config: HasherConfig,
    permits: Arc<Semaphore>,
    cache: Option<Arc<Cache<CacheKey, Arc<HashResult>>>>,
}

impl MultiAlgorithmHasher {
    /// Construct a new hasher from a configuration.
    ///
    /// # Errors
    ///
    /// Returns [`HashError::InvalidConfig`] if `config.validate()` fails.
    pub fn new(config: HasherConfig) -> Result<Self, HashError> {
        config.validate()?;
        let permits = Arc::new(Semaphore::new(config.max_concurrent));
        let cache =
            (config.cache_capacity > 0).then(|| Arc::new(Cache::new(config.cache_capacity)));
        Ok(Self {
            config,
            permits,
            cache,
        })
    }

    /// Return the configured max concurrency.
    #[must_use]
    pub const fn max_concurrent(&self) -> usize {
        self.config.max_concurrent
    }

    /// Compute hashes with an explicit deadline. Most callers should use the
    /// trait method [`HashComputer::compute`] instead, which derives the
    /// deadline from `config.timeout_per_file`.
    ///
    /// This path opens `path` ambiently via `std::fs::File::open`.
    /// **TOCTOU-sensitive callers** (e.g. `BinaryHasherCollector`) must
    /// use [`MultiAlgorithmHasher::compute_from_file_with_deadline`]
    /// instead, passing a file descriptor they authorized through a
    /// cap-std `Dir` handle.
    ///
    /// # Errors
    ///
    /// Returns [`HashError`] on I/O failure, timeout, cancellation, or
    /// oversized files.
    pub async fn compute_with_deadline(
        &self,
        path: &Path,
        deadline: Instant,
    ) -> Result<HashResult, HashError> {
        // Fast-fail on non-files BEFORE attempting to open. On Windows,
        // `std::fs::File::open` on a directory returns `PermissionDenied`
        // (CreateFile without FILE_FLAG_BACKUP_SEMANTICS), which would map
        // to `HashError::PermissionDenied` — misleading for callers
        // expecting "not a regular file". Pre-checking with
        // `symlink_metadata` lets us surface `HashError::Io` with a clear
        // message on every platform.
        //
        // Note: a second `fstat` still runs inside
        // `compute_from_file_with_deadline` against the opened handle, so
        // the authoritative size/mtime used for the cache key and the
        // mid-read-mutation check are the file we actually read — not the
        // path we stat'd first. This opening stat is a fast-fail only.
        let pre_meta = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|e| HashError::from_io(path, e))?;
        if !pre_meta.is_file() {
            return Err(HashError::Io {
                path: path.to_path_buf(),
                source: io::Error::other("path is not a regular file"),
            });
        }

        // Open the file by path; delegate to the file-based entry point.
        // This is the ambient-authority path used by procmond's
        // kernel-resolved exe hashing (which has already gone through
        // `authorize_kernel_path` and does not need cap-std confinement).
        let file = std::fs::File::open(path).map_err(|e| HashError::from_io(path, e))?;
        self.compute_from_file_with_deadline(path, file, deadline)
            .await
    }

    /// Compute hashes from an **already-opened** file handle, with an
    /// explicit deadline.
    ///
    /// TOCTOU-safe callers (e.g. `BinaryHasherCollector`) that authorize
    /// a path via cap-std `Dir::open` pass the resulting file descriptor
    /// here so the hash reads the inode that was authorized — not a path
    /// that may have been swapped between authorization and hashing.
    ///
    /// `path` is used **only** for the [`HashResult::file_path`] field,
    /// the cache key, and error context; it is never used to (re-)open
    /// the file.
    ///
    /// # Errors
    ///
    /// Returns [`HashError`] on I/O failure, timeout, cancellation, or
    /// oversized files.
    pub async fn compute_from_file_with_deadline(
        &self,
        path: &Path,
        file: std::fs::File,
        deadline: Instant,
    ) -> Result<HashResult, HashError> {
        // 1. fstat the handle (not the path) so mtime/size reflect the
        //    inode we will actually hash.
        let metadata = file.metadata().map_err(|e| HashError::from_io(path, e))?;

        if !metadata.is_file() {
            return Err(HashError::Io {
                path: path.to_path_buf(),
                source: io::Error::other("path is not a regular file"),
            });
        }

        let file_size = metadata.len();
        if file_size > self.config.max_file_size {
            return Err(HashError::FileTooLarge {
                size: file_size,
                limit: self.config.max_file_size,
            });
        }
        let modified_time = metadata
            .modified()
            .map_err(|e| HashError::from_io(path, e))?;

        // 2. Cache lookup. Cache key uses the file's fstat-derived
        //    (mtime, size) so reopening by path cannot evade a hit.
        let key: CacheKey = (path.to_path_buf(), modified_time, file_size);
        if let Some(cache) = self.cache.as_ref()
            && let Some(hit) = cache.get(&key)
        {
            debug!(path = ?path, "integrity cache hit");
            return Ok((*hit).clone());
        }

        // 3. Acquire permit BEFORE any hashing work begins.
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|err| HashError::InvalidConfig(format!("semaphore closed: {err}")))?;

        // 4. Size-based dispatch. Both branches use cooperative cancellation
        //    even though the inline branch has no timeout racer — it keeps
        //    the signature uniform and lets the inner loop's deadline check
        //    catch a clock-already-expired case for small files too.
        let algorithms = self.config.algorithms.clone();
        let buffer_size = self.config.buffer_size;
        let max_file_size = self.config.max_file_size;
        let cancel = Arc::new(AtomicBool::new(false));
        let path_owned = path.to_path_buf();

        let hash_outcome = if file_size < SPAWN_BLOCKING_THRESHOLD {
            // Small file: hash inline on the current task.
            hash_sync(
                file,
                &path_owned,
                deadline,
                &cancel,
                &algorithms,
                buffer_size,
                max_file_size,
            )
        } else {
            // Large file: spawn_blocking with cooperative cancellation.
            // The `File` is moved into the blocking task so it keeps
            // ownership of the fd. `std::fs::File` is `Send + 'static`.
            let cancel_for_task = Arc::clone(&cancel);
            let path_for_task = path_owned.clone();
            let algorithms_for_task = algorithms.clone();

            let join = tokio::task::spawn_blocking(move || {
                hash_sync(
                    file,
                    &path_for_task,
                    deadline,
                    &cancel_for_task,
                    &algorithms_for_task,
                    buffer_size,
                    max_file_size,
                )
            });

            let sleep = tokio::time::sleep_until(deadline.into());
            tokio::pin!(sleep);
            tokio::pin!(join);

            tokio::select! {
                () = &mut sleep => {
                    cancel.store(true, Ordering::Relaxed);
                    // Reap the join handle so the permit releases and the
                    // blocking thread is returned to the pool.
                    match (&mut join).await {
                        Ok(r) => r,
                        Err(e) => Err(HashError::Join(e.to_string())),
                    }
                }
                r = &mut join => {
                    match r {
                        Ok(inner) => inner,
                        Err(e) => Err(HashError::Join(e.to_string())),
                    }
                }
            }
        };

        drop(permit);
        let hash_result = hash_outcome?;

        if let Some(cache) = self.cache.as_ref() {
            cache.insert(key, Arc::new(hash_result.clone()));
        }
        Ok(hash_result)
    }

    /// Compute hashes from an already-opened file handle, using the
    /// engine's configured per-file timeout.
    ///
    /// TOCTOU-safe entry point used by callers that authorize a path via
    /// cap-std. See [`MultiAlgorithmHasher::compute_from_file_with_deadline`]
    /// for details.
    ///
    /// # Errors
    ///
    /// Returns [`HashError`] on I/O failure, timeout, cancellation, or
    /// oversized files.
    pub async fn compute_from_file(
        &self,
        path: &Path,
        file: std::fs::File,
    ) -> Result<HashResult, HashError> {
        let deadline = Instant::now()
            .checked_add(self.config.timeout_per_file)
            .ok_or_else(|| HashError::InvalidConfig("deadline overflow".to_owned()))?;
        self.compute_from_file_with_deadline(path, file, deadline)
            .await
    }
}

impl HashComputer for MultiAlgorithmHasher {
    async fn compute(&self, path: &Path) -> Result<HashResult, HashError> {
        // `Instant + Duration` is infallible in release builds and saturates
        // in debug — use `checked_add` so we never panic on overflow.
        let deadline = Instant::now()
            .checked_add(self.config.timeout_per_file)
            .ok_or_else(|| HashError::InvalidConfig("deadline overflow".to_owned()))?;
        self.compute_with_deadline(path, deadline).await
    }

    fn supported_algorithms(&self) -> &[HashAlgorithm] {
        &self.config.algorithms
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Synchronous inner hash routine
// ─────────────────────────────────────────────────────────────────────────────

/// Synchronous streaming hash with cooperative cancellation.
///
/// This function is the only place in the engine that actually reads from
/// disk. It is called:
///
/// 1. Directly (inline) for files smaller than `SPAWN_BLOCKING_THRESHOLD`.
/// 2. From inside `tokio::task::spawn_blocking` for larger files.
///
/// The caller passes an already-opened `std::fs::File`. The public entry
/// points [`MultiAlgorithmHasher::compute_with_deadline`] (path-based) and
/// [`MultiAlgorithmHasher::compute_from_file_with_deadline`] (fd-based)
/// both funnel into this routine. TOCTOU-safe callers (e.g.
/// `BinaryHasherCollector`) pass a file descriptor obtained via cap-std so
/// the hash reads the inode that was authorized, not a path that may have
/// been swapped.
///
/// `path` is used **only** for the `HashResult::file_path` field and for
/// error context. It is never used to (re-)open the file.
///
/// The caller passes an `Arc<AtomicBool>` cancel flag. The outer async driver
/// flips this flag when a deadline expires; the loop observes it on the next
/// iteration and returns `HashError::Cancelled` or `HashError::Timeout`.
fn hash_sync(
    mut file: std::fs::File,
    path: &Path,
    deadline: Instant,
    cancel: &AtomicBool,
    algorithms: &[HashAlgorithm],
    buffer_size: usize,
    max_file_size: u64,
) -> Result<HashResult, HashError> {
    let start = Instant::now();

    let meta_before = file.metadata().map_err(|e| HashError::from_io(path, e))?;

    if !meta_before.is_file() {
        return Err(HashError::Io {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file"),
        });
    }

    let file_size_before = meta_before.len();
    let modified_before = meta_before
        .modified()
        .map_err(|e| HashError::from_io(path, e))?;

    if file_size_before > max_file_size {
        return Err(HashError::FileTooLarge {
            size: file_size_before,
            limit: max_file_size,
        });
    }

    let mut hashers = HasherSet::new(algorithms);
    let mut buf = vec![0_u8; buffer_size];
    let mut total_read: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(HashError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(HashError::Timeout);
        }
        if total_read > max_file_size {
            return Err(HashError::FileTooLarge {
                size: total_read,
                limit: max_file_size,
            });
        }

        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(HashError::from_io(path, e)),
        };
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or_else(|| HashError::Io {
            path: path.to_path_buf(),
            source: io::Error::other("read returned oversized count"),
        })?;
        // usize → u64 is a widening conversion on all supported targets
        // (and saturating_add prevents any overflow regardless).
        let n_u64 = u64::try_from(n).unwrap_or(u64::MAX);
        total_read = total_read.saturating_add(n_u64);
        hashers.update(chunk);
    }

    let meta_after = file.metadata().map_err(|e| HashError::from_io(path, e))?;
    let size_after = meta_after.len();
    let modified_after = meta_after.modified().ok();

    if size_after != file_size_before || modified_after != Some(modified_before) {
        warn!(
            path = ?path,
            "file metadata changed during hash; returning Nonauthoritative"
        );
        return Err(HashError::Nonauthoritative {
            path: path.to_path_buf(),
        });
    }

    let hashes = hashers.finalize_into();

    Ok(HashResult {
        file_path: path.to_path_buf(),
        file_size: file_size_before,
        modified_time: modified_before,
        hashes,
        integrity: HashIntegrity::Stable,
        computation_time: start.elapsed(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    // ── Algorithm metadata ──────────────────────────────────────────────

    #[test]
    fn algorithm_security_classification() {
        assert!(HashAlgorithm::Sha256.is_cryptographically_secure());
        assert!(HashAlgorithm::Blake3.is_cryptographically_secure());
        #[cfg(feature = "sha3-hashes")]
        assert!(HashAlgorithm::Sha3_256.is_cryptographically_secure());
    }

    #[test]
    fn algorithm_wire_names() {
        assert_eq!(HashAlgorithm::Sha256.wire_name(), "sha256");
        assert_eq!(HashAlgorithm::Blake3.wire_name(), "blake3");
        assert_eq!(HashAlgorithm::Sha256.to_string(), "sha256");
    }

    #[test]
    fn algorithm_hex_lengths() {
        assert_eq!(HashAlgorithm::Sha256.hex_len(), 64);
        assert_eq!(HashAlgorithm::Blake3.hex_len(), 64);
    }

    // ── Config validation ───────────────────────────────────────────────

    #[test]
    fn default_config_validates() {
        HasherConfig::default().validate().unwrap();
    }

    #[test]
    fn default_config_defaults_to_sha256_and_blake3() {
        let cfg = HasherConfig::default();
        assert!(cfg.algorithms.contains(&HashAlgorithm::Sha256));
        assert!(cfg.algorithms.contains(&HashAlgorithm::Blake3));
    }

    #[test]
    fn default_config_concurrency_clamped() {
        let cfg = HasherConfig::default();
        assert!(cfg.max_concurrent >= MIN_CONCURRENCY);
        assert!(cfg.max_concurrent <= MAX_CONCURRENCY);
    }

    #[test]
    fn config_builder_clamps_concurrency() {
        let high = HasherConfig::default().with_max_concurrent(9999);
        assert_eq!(high.max_concurrent, MAX_CONCURRENCY);
        let low = HasherConfig::default().with_max_concurrent(0);
        assert_eq!(low.max_concurrent, MIN_CONCURRENCY);
    }

    #[test]
    fn empty_algorithms_fails_validation() {
        let cfg = HasherConfig::default().with_algorithms(vec![]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn zero_max_file_size_fails_validation() {
        let cfg = HasherConfig::default().with_max_file_size(0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn zero_timeout_fails_validation() {
        let cfg = HasherConfig::default().with_timeout(Duration::ZERO);
        assert!(cfg.validate().is_err());
    }

    // ── Known-answer tests (NIST + BLAKE3 official) ─────────────────────

    /// NIST FIPS 180-4: SHA-256 of empty string.
    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// NIST FIPS 180-4: SHA-256 of "abc".
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    /// BLAKE3 official test vector: empty input.
    const BLAKE3_EMPTY: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    /// BLAKE3 official test vector: "abc".
    const BLAKE3_ABC: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";

    async fn hash_bytes(bytes: &[u8]) -> HashResult {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), bytes).unwrap();
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        hasher.compute(tmp.path()).await.unwrap()
    }

    #[tokio::test]
    async fn sha256_empty_matches_nist_vector() {
        let r = hash_bytes(b"").await;
        assert_eq!(r.sha256().unwrap(), SHA256_EMPTY);
    }

    #[tokio::test]
    async fn sha256_abc_matches_nist_vector() {
        let r = hash_bytes(b"abc").await;
        assert_eq!(r.sha256().unwrap(), SHA256_ABC);
    }

    #[tokio::test]
    async fn blake3_empty_matches_official_vector() {
        let r = hash_bytes(b"").await;
        assert_eq!(r.blake3().unwrap(), BLAKE3_EMPTY);
    }

    #[tokio::test]
    async fn blake3_abc_matches_official_vector() {
        let r = hash_bytes(b"abc").await;
        assert_eq!(r.blake3().unwrap(), BLAKE3_ABC);
    }

    #[tokio::test]
    async fn large_file_spans_spawn_blocking_threshold() {
        // 512 KiB > 256 KiB threshold — takes the spawn_blocking path.
        let bytes = vec![0_u8; 512 * 1024];
        let r = hash_bytes(&bytes).await;
        assert_eq!(r.sha256().unwrap().len(), 64);
        assert_eq!(r.blake3().unwrap().len(), 64);
        assert!(r.is_authoritative());
        assert_eq!(r.file_size, 512 * 1024);
    }

    #[tokio::test]
    async fn deterministic_repeated_hash_yields_same_result() {
        let bytes = b"deterministic repeatability test payload";
        let r1 = hash_bytes(bytes).await;
        let r2 = hash_bytes(bytes).await;
        assert_eq!(r1.hashes, r2.hashes);
    }

    #[tokio::test]
    async fn byte_change_produces_different_hash() {
        let r1 = hash_bytes(b"payload-a").await;
        let r2 = hash_bytes(b"payload-b").await;
        assert_ne!(r1.hashes, r2.hashes);
    }

    // ── Error paths ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn missing_file_returns_file_not_found() {
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let err = hasher
            .compute(Path::new("/definitely/does/not/exist/xyz"))
            .await
            .unwrap_err();
        assert!(matches!(err, HashError::FileNotFound { .. }));
    }

    #[tokio::test]
    async fn oversized_file_returns_too_large() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), vec![0_u8; 4096]).unwrap();
        let cfg = HasherConfig::default().with_max_file_size(1024);
        let hasher = MultiAlgorithmHasher::new(cfg).unwrap();
        let err = hasher.compute(tmp.path()).await.unwrap_err();
        let HashError::FileTooLarge { size, limit } = err else {
            panic!("expected FileTooLarge, got {err:?}");
        };
        assert_eq!(size, 4096);
        assert_eq!(limit, 1024);
    }

    #[tokio::test]
    async fn directory_rejected() {
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = hasher.compute(tmp.path()).await.unwrap_err();
        assert!(matches!(err, HashError::Io { .. }));
    }

    #[tokio::test]
    async fn integrity_marked_stable_on_unmodified_file() {
        let r = hash_bytes(b"stable file").await;
        assert_eq!(r.integrity, HashIntegrity::Stable);
        assert!(r.is_authoritative());
    }

    // ── Cache behavior ──────────────────────────────────────────────────

    #[tokio::test]
    async fn cache_returns_identical_result_on_second_call() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"cache test").unwrap();
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let r1 = hasher.compute(tmp.path()).await.unwrap();
        let r2 = hasher.compute(tmp.path()).await.unwrap();
        assert_eq!(r1.hashes, r2.hashes);
        assert_eq!(r1.file_size, r2.file_size);
    }

    #[tokio::test]
    async fn cache_disabled_still_works() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"no cache").unwrap();
        let cfg = HasherConfig::default().with_cache_capacity(0);
        let hasher = MultiAlgorithmHasher::new(cfg).unwrap();
        let r = hasher.compute(tmp.path()).await.unwrap();
        assert!(r.sha256().is_some());
    }

    // ── Concurrency + timeout ───────────────────────────────────────────

    #[tokio::test]
    async fn timeout_fires_on_impossible_deadline() {
        // Use a large file to ensure we take the spawn_blocking path and
        // the read loop gets a chance to observe the timeout.
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), vec![0_u8; 2 * 1024 * 1024]).unwrap();
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let deadline = Instant::now(); // Already expired.
        let err = hasher
            .compute_with_deadline(tmp.path(), deadline)
            .await
            .unwrap_err();
        assert!(matches!(err, HashError::Timeout | HashError::Cancelled));
    }

    #[tokio::test]
    async fn concurrent_hashes_all_succeed() {
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());
        let mut tasks = Vec::new();
        for i in 0_u32..8 {
            let hasher_clone = Arc::clone(&hasher);
            tasks.push(tokio::spawn(async move {
                let tmp = NamedTempFile::new().unwrap();
                fs::write(tmp.path(), format!("payload-{i}").as_bytes()).unwrap();
                hasher_clone.compute(tmp.path()).await
            }));
        }
        for t in tasks {
            let r = t.await.unwrap().unwrap();
            assert_eq!(r.sha256().unwrap().len(), 64);
        }
    }

    // ── compute_from_file (TOCTOU-safe entry point) ─────────────────────

    #[tokio::test]
    async fn compute_from_file_matches_compute_by_path() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"toctou-safe hash").unwrap();
        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();

        let by_path = hasher.compute(tmp.path()).await.unwrap();
        let file = std::fs::File::open(tmp.path()).unwrap();
        let by_file = hasher.compute_from_file(tmp.path(), file).await.unwrap();

        assert_eq!(by_path.hashes, by_file.hashes);
        assert_eq!(by_path.file_size, by_file.file_size);
    }

    #[tokio::test]
    async fn compute_from_file_hashes_authorized_inode_even_if_path_swapped() {
        // Simulates the TOCTOU scenario: authorize and open a file, then
        // replace the path's contents before hashing. The engine must
        // hash the inode we opened, not the new contents.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target");
        fs::write(&path, b"original authorized content").unwrap();

        // Open BEFORE the swap (this is what cap-std does during authorization).
        let file = std::fs::File::open(&path).unwrap();

        // Attacker swaps the path contents (new inode via rename-over).
        let decoy = dir.path().join("decoy");
        fs::write(&decoy, b"attacker-injected content").unwrap();
        std::fs::rename(&decoy, &path).unwrap();

        let hasher = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let result = hasher.compute_from_file(&path, file).await.unwrap();

        // The hash must match the ORIGINAL content we held an fd to.
        let expected = MultiAlgorithmHasher::new(HasherConfig::default()).unwrap();
        let control_tmp = NamedTempFile::new().unwrap();
        fs::write(control_tmp.path(), b"original authorized content").unwrap();
        let control = expected.compute(control_tmp.path()).await.unwrap();
        assert_eq!(
            result.sha256(),
            control.sha256(),
            "compute_from_file must hash the held inode, not re-open by path"
        );
    }

    // ── BTreeMap iteration order is stable ──────────────────────────────

    #[test]
    fn btreemap_iteration_is_deterministic() {
        let mut m1: BTreeMap<HashAlgorithm, String> = BTreeMap::new();
        let _ = m1.insert(HashAlgorithm::Blake3, "b".to_owned());
        let _ = m1.insert(HashAlgorithm::Sha256, "a".to_owned());
        let keys: Vec<_> = m1.keys().copied().collect();
        // Order derives from HashAlgorithm discriminant: Sha256 < Blake3.
        assert_eq!(keys, vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3]);
    }
}
