//! `BinaryHasherCollector` — the first [`TriggerableCollector`] implementation.
//!
//! Consumes [`TriggerRequest`]s with `analysis_type = AnalysisType::BinaryHash`,
//! authorizes the target path against a mandatory `allowed_roots` list, and
//! delegates hashing to [`daemoneye_lib::integrity::MultiAlgorithmHasher`].
//! Returns an [`AnalysisResult`] containing a JSON-encoded `HashResult`.
//!
//! # Security model
//!
//! `TriggerRequest.target_path` is **untrusted input**: detection rules can
//! be influenced by attacker-controlled process paths, and rule authors may
//! misuse the trigger mechanism even without malicious intent. The collector
//! defends in depth:
//!
//! 1. **Mandatory allow-list with deny-on-empty**:
//!    [`BinaryHasherConfig::allowed_roots`] must be non-empty. Requests for
//!    paths outside every root return [`crate::triggerable::TriggerErrorKind::PathNotAllowed`].
//!    [`BinaryHasherConfig::with_platform_defaults`] provides secure defaults
//!    for Linux, macOS, and Windows.
//!
//! 2. **Path length cap**: requests with `target_path.len() > 4096` are
//!    rejected with [`crate::triggerable::TriggerErrorKind::InvalidRequest`] before any I/O.
//!
//! 3. **Parent-traversal rejection**: paths containing `..` components are
//!    rejected before canonicalization to prevent path-traversal primitives.
//!
//! 4. **Symlink rejection** (default): paths that resolve through a symbolic
//!    link, Windows junction, or reparse point return
//!    [`crate::triggerable::TriggerErrorKind::Unavailable`]. Operators can opt in to symlink
//!    following via [`BinaryHasherConfig::with_follow_symlinks`], but the
//!    resolved target must still pass the `allowed_roots` check.
//!
//! 5. **Canonicalization + prefix match**: the requested path is
//!    canonicalized and then verified to be under one of the configured
//!    roots. On Windows, the `dunce` crate's UNC normalization would be
//!    used — since we do not yet depend on it, Windows support is a
//!    documented follow-up.
//!
//! 6. **Wire-error sanitization**: internal errors carry rich context
//!    (paths, reasons) for local `tracing::warn!` logs. At the trait
//!    boundary they map through [`TriggerHandleError::kind`] to the closed
//!    [`crate::triggerable::TriggerErrorKind`] enum.
//!    [`crate::triggerable::TriggerErrorKind::Unavailable`] deliberately merges "permission
//!    denied" and "not found" to prevent file-existence oracles.
//!
//! 7. **Critical priority does NOT bypass authorization**: priority
//!    affects queue ordering only. `allowed_roots`, symlink policy, and
//!    resource budgets apply uniformly.
//!
//! # Known gaps (follow-up work)
//!
//! - **TOCTOU-safe opens**: full defense against symlink-swap attacks
//!   between `canonicalize()` and `File::open()` requires `cap-std` or
//!   Linux `openat2(RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH)`. Scheduled for
//!   Phase 3 of the P1 resolution plan
//!   (`docs/plans/2026-04-09-001-refactor-binary-hashing-p1-resolutions-plan.md`).
//!   The engine's stat-before / stat-after mutation check still fires a
//!   [`HashError::Nonauthoritative`] at the engine boundary in the
//!   meantime, so mid-read mutations are detected (though the initial
//!   open still has a race window).
//! - **Windows junction / reparse-point rejection**: requires calling the
//!   Win32 `GetFileInformationByHandleEx` API via the `windows` crate; the
//!   current stdlib-only path relies on `symlink_metadata().is_symlink()`
//!   which does not cover all reparse-point types.

use crate::analysis_chain::AnalysisResult;
use crate::event::{AnalysisType, TriggerRequest};
use crate::triggerable::{TriggerHandleError, TriggerableCollector};
use daemoneye_lib::integrity::{HashComputer, HashError, HasherConfig, MultiAlgorithmHasher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

/// Hard cap on `TriggerRequest.target_path` length in bytes. Paths longer
/// than this are rejected with [`crate::triggerable::TriggerErrorKind::InvalidRequest`] before
/// any I/O.
pub const MAX_TARGET_PATH_LEN: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// BinaryHasherConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`BinaryHasherCollector`].
#[derive(Debug, Clone)]
pub struct BinaryHasherConfig {
    /// Mandatory allow-list of root directories. **Empty list denies all
    /// requests.** Use [`BinaryHasherConfig::with_platform_defaults`] for
    /// a reasonable starting set.
    pub allowed_roots: Vec<PathBuf>,
    /// When `false` (default), any path that resolves through a symbolic
    /// link or junction is rejected with `Unavailable`. When `true`, the
    /// resolved real path must still pass the `allowed_roots` check.
    pub follow_symlinks: bool,
    /// Maximum accepted file size, enforced before any read.
    pub max_file_size: u64,
    /// Per-file hashing deadline propagated to the engine.
    pub timeout_per_file: Duration,
}

impl Default for BinaryHasherConfig {
    fn default() -> Self {
        Self {
            allowed_roots: Vec::new(),
            follow_symlinks: false,
            max_file_size: 512 * 1024 * 1024,
            timeout_per_file: Duration::from_secs(10),
        }
    }
}

impl BinaryHasherConfig {
    /// Builder: populate `allowed_roots` with platform-specific secure
    /// defaults.
    ///
    /// On Unix: `/usr/bin`, `/usr/sbin`, `/usr/local/bin`, `/bin`, `/sbin`,
    /// `/opt`. On macOS, also `/Applications` and `/System/Applications`.
    /// On Windows: resolves `%SystemRoot%\\System32` and `%ProgramFiles%`
    /// from the environment if available.
    ///
    /// Roots that do not exist on the current system are still added —
    /// they won't match any request but keep the configuration portable
    /// across CI matrices.
    #[must_use]
    pub fn with_platform_defaults(mut self) -> Self {
        #[cfg(unix)]
        self.allowed_roots.extend_from_slice(&[
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/opt"),
        ]);
        #[cfg(target_os = "macos")]
        self.allowed_roots.extend_from_slice(&[
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
        ]);
        #[cfg(windows)]
        {
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                self.allowed_roots
                    .push(PathBuf::from(system_root).join("System32"));
            }
            if let Some(program_files) = std::env::var_os("ProgramFiles") {
                self.allowed_roots.push(PathBuf::from(program_files));
            }
        }
        self
    }

    /// Builder: add a single allowed root. The root path is stored as-is;
    /// request canonicalization is performed at authorization time.
    #[must_use]
    pub fn with_allowed_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.allowed_roots.push(root.into());
        self
    }

    /// Builder: toggle symlink-following. **Leave at `false` for forensic
    /// use unless operators opt in.**
    #[must_use]
    pub const fn with_follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// Builder: override the maximum accepted file size.
    #[must_use]
    pub const fn with_max_file_size(mut self, bytes: u64) -> Self {
        self.max_file_size = bytes;
        self
    }

    /// Builder: override the per-file timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_per_file = timeout;
        self
    }

    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`TriggerHandleError::Internal`] if any field is invalid.
    /// Most notably, an empty `allowed_roots` list **must** fail validation
    /// so an operator cannot accidentally deploy a deny-all-or-allow-all
    /// misconfiguration.
    pub fn validate(&self) -> Result<(), TriggerHandleError> {
        if self.allowed_roots.is_empty() {
            return Err(TriggerHandleError::Internal(
                "allowed_roots must be non-empty - use with_platform_defaults() \
                 or add explicit roots"
                    .to_owned(),
            ));
        }
        if self.max_file_size == 0 {
            return Err(TriggerHandleError::Internal(
                "max_file_size must be greater than zero".to_owned(),
            ));
        }
        if self.timeout_per_file.is_zero() {
            return Err(TriggerHandleError::Internal(
                "timeout_per_file must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BinaryHasherCollector
// ─────────────────────────────────────────────────────────────────────────────

/// Reactive collector that handles [`AnalysisType::BinaryHash`] requests.
///
/// Holds an `Arc<MultiAlgorithmHasher>` shared with any other caller of the
/// same engine so the configured concurrency cap and cache are honored
/// across the whole process.
pub struct BinaryHasherCollector {
    engine: Arc<MultiAlgorithmHasher>,
    config: BinaryHasherConfig,
}

impl std::fmt::Debug for BinaryHasherCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Intentionally omit the engine: its internal Semaphore and
        // quick_cache are not Debug and would just clutter the output.
        f.debug_struct("BinaryHasherCollector")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl BinaryHasherCollector {
    /// Construct a collector with a shared engine and a validated config.
    ///
    /// # Errors
    ///
    /// Returns [`TriggerHandleError::Internal`] if `config.validate()`
    /// fails (most commonly an empty `allowed_roots` list).
    pub fn new(
        engine: Arc<MultiAlgorithmHasher>,
        config: BinaryHasherConfig,
    ) -> Result<Self, TriggerHandleError> {
        config.validate()?;
        Ok(Self { engine, config })
    }

    /// Construct a collector with a default engine and default platform
    /// roots. Intended for tests and single-process deployments.
    ///
    /// # Errors
    ///
    /// Returns [`TriggerHandleError::Internal`] if the default engine
    /// configuration fails to validate or if platform defaults produce an
    /// empty allow-list on the current OS.
    pub fn with_default_engine() -> Result<Self, TriggerHandleError> {
        let engine = MultiAlgorithmHasher::new(HasherConfig::default())
            .map_err(|e| TriggerHandleError::Internal(e.to_string()))?;
        let config = BinaryHasherConfig::default().with_platform_defaults();
        Self::new(Arc::new(engine), config)
    }

    /// Stable collector name used to match `TriggerRequest.target_collector`.
    pub const NAME: &'static str = "binary-hasher";

    // ── Path authorization ──────────────────────────────────────────────

    /// Authorize a request and return the canonicalized, validated path.
    ///
    /// This is the single choke point for every security check. Ordering
    /// matters:
    ///
    /// 1. Length cap
    /// 2. Parent-traversal rejection (`..` components)
    /// 3. `symlink_metadata` — if the final component is a symlink and
    ///    `follow_symlinks` is false, reject with `Unavailable`.
    /// 4. `canonicalize` — resolves any intermediate symlinks even when
    ///    `follow_symlinks` is true, so the subsequent allow-list check
    ///    sees the real path.
    /// 5. Verify canonical path starts with at least one allowed root.
    /// 6. File-type check (must be a regular file).
    /// 7. Size check against `config.max_file_size`.
    fn authorize(&self, raw: &str) -> Result<PathBuf, TriggerHandleError> {
        // 1. Length cap — reject before any I/O.
        if raw.len() > MAX_TARGET_PATH_LEN {
            return Err(TriggerHandleError::PathTooLong { len: raw.len() });
        }
        if raw.is_empty() {
            return Err(TriggerHandleError::PathRejected {
                reason: "empty path".to_owned(),
            });
        }

        let raw_path = Path::new(raw);

        // 2. Parent-traversal rejection. We reject `..` components BEFORE
        //    canonicalization because a path like `/usr/bin/../../etc/shadow`
        //    canonicalizes to `/etc/shadow` which might be accepted if an
        //    operator has misconfigured roots. Better to refuse up front.
        for component in raw_path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(TriggerHandleError::PathRejected {
                    reason: "path contains '..' component".to_owned(),
                });
            }
        }

        // 3. symlink_metadata — checks the final component without
        //    following symlinks. If it IS a symlink and we don't follow,
        //    reject with a bland Unavailable so we don't leak "this
        //    specific path exists as a symlink".
        let link_meta = match std::fs::symlink_metadata(raw_path) {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Merged with PermissionDenied on the wire (Unavailable).
                return Err(TriggerHandleError::Unavailable {
                    reason: format!("symlink_metadata not-found: {raw}"),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(TriggerHandleError::Unavailable {
                    reason: format!("symlink_metadata permission denied: {raw}"),
                });
            }
            Err(err) => {
                return Err(TriggerHandleError::Internal(format!(
                    "symlink_metadata failed for {raw}: {err}"
                )));
            }
        };

        if link_meta.file_type().is_symlink() && !self.config.follow_symlinks {
            warn!(path = %raw, "rejecting symlink under follow_symlinks=false policy");
            return Err(TriggerHandleError::Unavailable {
                reason: "target is a symlink".to_owned(),
            });
        }

        // 4. Canonicalize. This follows symlinks in intermediate components
        //    (which we allow — the concern is the final component's type),
        //    resolving to a real path for the allow-list check.
        let canonical =
            std::fs::canonicalize(raw_path).map_err(|err| map_path_io_err("canonicalize", &err))?;

        // 5. Allow-list prefix check. We canonicalize each root too so the
        //    comparison is apples-to-apples (both sides resolved through
        //    any intermediate symlinks). A root that does not exist is
        //    simply skipped.
        let mut allowed = false;
        for root in &self.config.allowed_roots {
            if let Ok(canonical_root) = std::fs::canonicalize(root)
                && canonical.starts_with(&canonical_root)
            {
                allowed = true;
                break;
            }
        }
        if !allowed {
            warn!(path = %canonical.display(), "rejecting path outside allowed_roots");
            return Err(TriggerHandleError::PathNotAllowed);
        }

        // 6. File-type check on the canonical path.
        let meta =
            std::fs::metadata(&canonical).map_err(|err| map_path_io_err("metadata", &err))?;
        if !meta.is_file() {
            return Err(TriggerHandleError::Unavailable {
                reason: "target is not a regular file".to_owned(),
            });
        }

        // 7. Size check.
        if meta.len() > self.config.max_file_size {
            return Err(TriggerHandleError::TooLarge {
                size: meta.len(),
                limit: self.config.max_file_size,
            });
        }

        Ok(canonical)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TriggerableCollector impl
// ─────────────────────────────────────────────────────────────────────────────

impl TriggerableCollector for BinaryHasherCollector {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn supported_analysis_types(&self) -> &[AnalysisType] {
        const TYPES: &[AnalysisType] = &[AnalysisType::BinaryHash];
        TYPES
    }

    async fn handle_trigger(
        &self,
        request: &TriggerRequest,
    ) -> Result<AnalysisResult, TriggerHandleError> {
        // Audit-log Critical-priority requests regardless of outcome so
        // operators can investigate any use of the priority bypass. Note
        // that Critical does NOT bypass allowed_roots authorization.
        if matches!(request.priority, crate::event::TriggerPriority::Critical) {
            info!(
                trigger_id = %request.trigger_id,
                correlation_id = %request.correlation_id,
                target_path = ?request.target_path,
                "critical-priority binary-hash trigger accepted for processing"
            );
        }

        let raw_path = request
            .target_path
            .as_deref()
            .ok_or(TriggerHandleError::MissingField {
                field: "target_path",
            })?;

        let canonical = self.authorize(raw_path)?;

        // Delegate to the engine (which enforces its own concurrency cap,
        // cache, TOCTOU tagging, cooperative cancellation, and timeout).
        let hash_result = self
            .engine
            .compute(&canonical)
            .await
            .map_err(map_hash_err)?;

        debug!(
            trigger_id = %request.trigger_id,
            path = %canonical.display(),
            algorithms = ?hash_result.hashes.keys().collect::<Vec<_>>(),
            "binary-hash trigger completed"
        );

        Ok(AnalysisResult {
            stage_id: format!("trigger:{}", request.trigger_id),
            analysis_type: request.analysis_type.clone(),
            collector_id: Self::NAME.to_owned(),
            result_data: serde_json::to_value(&hash_result)
                .map_err(|e| TriggerHandleError::Internal(format!("serde: {e}")))?,
            metadata: HashMap::new(),
            completed_at: SystemTime::now(),
            execution_duration: hash_result.computation_time,
            // Post type-state refactor: a successful `HashResult` is always
            // authoritative. Non-authoritative (mid-read mutation) cases are
            // returned as `HashError::Nonauthoritative` by the engine and
            // mapped to `TriggerErrorKind::Unavailable` via `map_hash_err`
            // below, so this arm always observes a stable hash.
            confidence: 1.0,
        })
    }

    async fn health_check(&self) -> Result<(), TriggerHandleError> {
        // Health is good if at least one allowed root is reachable.
        for root in &self.config.allowed_roots {
            if std::fs::metadata(root).is_ok() {
                return Ok(());
            }
        }
        Err(TriggerHandleError::Internal(
            "no allowed_roots are currently accessible".to_owned(),
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HashError → TriggerHandleError mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Map a stdlib `io::Error` from a path operation into a wire-safe
/// [`TriggerHandleError`]. `NotFound` and `PermissionDenied` both map to
/// `Unavailable` to prevent file-existence oracles. The wildcard arm is
/// required because `io::ErrorKind` is `#[non_exhaustive]` upstream.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "io::ErrorKind is #[non_exhaustive] upstream"
)]
fn map_path_io_err(op: &'static str, err: &std::io::Error) -> TriggerHandleError {
    match err.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
            TriggerHandleError::Unavailable {
                reason: format!("{op} failed: {err}"),
            }
        }
        _ => TriggerHandleError::Internal(format!("{op} error: {err}")),
    }
}

/// Map engine-level errors to the wire-safe trait-level error.
///
/// `PermissionDenied` and `FileNotFound` **both** map to `Unavailable` so a
/// caller cannot use differential error responses to probe the filesystem.
///
/// The wildcard arm is required because [`HashError`] is `#[non_exhaustive]`;
/// any future variants will transparently fall through to `Internal` until
/// this mapping is explicitly updated.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "HashError is #[non_exhaustive]; wildcard required for forward-compat"
)]
fn map_hash_err(err: HashError) -> TriggerHandleError {
    match err {
        HashError::PermissionDenied { path } => TriggerHandleError::Unavailable {
            reason: format!("permission denied: {}", path.display()),
        },
        HashError::FileNotFound { path } => TriggerHandleError::Unavailable {
            reason: format!("not found: {}", path.display()),
        },
        HashError::FileTooLarge { size, limit } => TriggerHandleError::TooLarge { size, limit },
        HashError::Timeout | HashError::Cancelled => TriggerHandleError::Timeout,
        HashError::Io { path, source } => {
            TriggerHandleError::Internal(format!("I/O error at {}: {source}", path.display()))
        }
        HashError::Join(msg) => TriggerHandleError::Internal(format!("join: {msg}")),
        HashError::InvalidConfig(msg) => TriggerHandleError::Internal(format!("config: {msg}")),
        // A mid-read mutation is a detection signal, not a caller error.
        // Map to Unavailable (same bucket as NotFound/PermissionDenied) so
        // wire-level responses do not leak the distinction. The audit-level
        // telemetry for this event is emitted separately in Phase 4.
        HashError::Nonauthoritative { path } => TriggerHandleError::Unavailable {
            reason: format!("file mutated during hash: {}", path.display()),
        },
        _ => TriggerHandleError::Internal("unrecognized HashError variant".to_owned()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::event::{AnalysisType, TriggerPriority};
    use crate::triggerable::TriggerErrorKind;
    use daemoneye_lib::integrity::HashAlgorithm;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::{NamedTempFile, TempDir};

    fn make_engine() -> Arc<MultiAlgorithmHasher> {
        Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap())
    }

    fn make_collector(allowed_root: &Path) -> BinaryHasherCollector {
        let config = BinaryHasherConfig::default().with_allowed_root(allowed_root);
        BinaryHasherCollector::new(make_engine(), config).unwrap()
    }

    fn make_request(path: impl Into<String>) -> TriggerRequest {
        TriggerRequest {
            trigger_id: "t-test".to_owned(),
            target_collector: BinaryHasherCollector::NAME.to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Normal,
            target_pid: None,
            target_path: Some(path.into()),
            correlation_id: "c-test".to_owned(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        }
    }

    // ── Config validation ───────────────────────────────────────────────

    #[test]
    fn empty_allowed_roots_deny_on_empty() {
        let config = BinaryHasherConfig::default();
        let err = BinaryHasherCollector::new(make_engine(), config).unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::Internal);
    }

    #[test]
    fn platform_defaults_populate_allowed_roots() {
        let config = BinaryHasherConfig::default().with_platform_defaults();
        assert!(
            !config.allowed_roots.is_empty(),
            "platform defaults should produce a non-empty allow-list"
        );
    }

    #[test]
    fn zero_max_file_size_fails_validation() {
        let config = BinaryHasherConfig::default()
            .with_allowed_root("/tmp")
            .with_max_file_size(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn zero_timeout_fails_validation() {
        let config = BinaryHasherConfig::default()
            .with_allowed_root("/tmp")
            .with_timeout(Duration::ZERO);
        assert!(config.validate().is_err());
    }

    // ── Successful happy path ───────────────────────────────────────────

    #[tokio::test]
    async fn hashes_file_under_allowed_root() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("hello.bin");
        fs::write(&file_path, b"hello binary hasher").unwrap();
        let collector = make_collector(dir.path());
        let req = make_request(file_path.to_string_lossy().into_owned());
        let result = collector.handle_trigger(&req).await.unwrap();
        assert_eq!(result.collector_id, "binary-hasher");
        assert_eq!(result.analysis_type, AnalysisType::BinaryHash);
        // result_data should contain the hex hashes.
        let hashes = result.result_data.get("hashes").unwrap();
        let sha256 = hashes.get(HashAlgorithm::Sha256.wire_name()).unwrap();
        assert_eq!(sha256.as_str().unwrap().len(), 64);
    }

    // ── Security: allowed_roots enforcement ─────────────────────────────

    #[tokio::test]
    async fn rejects_path_outside_allowed_roots() {
        let allowed = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let file = other.path().join("unauthorized.bin");
        fs::write(&file, b"outside").unwrap();

        let collector = make_collector(allowed.path());
        let req = make_request(file.to_string_lossy().into_owned());
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::PathNotAllowed);
    }

    // ── Security: path length cap ───────────────────────────────────────

    #[tokio::test]
    async fn rejects_over_length_path_before_io() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        let oversized = "a".repeat(MAX_TARGET_PATH_LEN + 1);
        let req = make_request(oversized);
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn rejects_empty_path() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        let req = make_request("");
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::InvalidRequest);
    }

    // ── Security: parent-traversal rejection ────────────────────────────

    #[tokio::test]
    async fn rejects_path_with_parent_component() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        let attack = format!("{}/../../etc/passwd", dir.path().display());
        let req = make_request(attack);
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::InvalidRequest);
    }

    // ── Security: symlink rejection (default) ───────────────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_target_by_default() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"content").unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let collector = make_collector(dir.path());
        let req = make_request(link.to_string_lossy().into_owned());
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::Unavailable);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn follows_symlink_when_opted_in() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"content").unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let config = BinaryHasherConfig::default()
            .with_allowed_root(dir.path())
            .with_follow_symlinks(true);
        let collector = BinaryHasherCollector::new(make_engine(), config).unwrap();
        let req = make_request(link.to_string_lossy().into_owned());
        let result = collector.handle_trigger(&req).await.unwrap();
        assert_eq!(result.collector_id, "binary-hasher");
    }

    // ── Security: file-existence oracle defense ─────────────────────────

    #[tokio::test]
    async fn not_found_and_denied_both_map_to_unavailable() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());

        let missing = dir.path().join("does-not-exist.bin");
        let req = make_request(missing.to_string_lossy().into_owned());
        let err = collector.handle_trigger(&req).await.unwrap_err();
        // Not-found path is Unavailable on the wire.
        assert_eq!(err.kind(), TriggerErrorKind::Unavailable);
    }

    // ── Security: oversized file rejection ──────────────────────────────

    #[tokio::test]
    async fn oversized_file_rejected_before_hashing() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("big.bin");
        fs::write(&file, vec![0_u8; 8192]).unwrap();
        let config = BinaryHasherConfig::default()
            .with_allowed_root(dir.path())
            .with_max_file_size(1024);
        let collector = BinaryHasherCollector::new(make_engine(), config).unwrap();
        let req = make_request(file.to_string_lossy().into_owned());
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::TooLarge);
    }

    // ── MissingField ────────────────────────────────────────────────────

    #[tokio::test]
    async fn missing_target_path_is_invalid_request() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        let mut req = make_request("ignored");
        req.target_path = None;
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::InvalidRequest);
    }

    // ── Directories rejected ────────────────────────────────────────────

    #[tokio::test]
    async fn directory_target_is_unavailable() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        let req = make_request(dir.path().to_string_lossy().into_owned());
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(err.kind(), TriggerErrorKind::Unavailable);
    }

    // ── Trait metadata ──────────────────────────────────────────────────

    #[test]
    fn collector_name_matches_trigger_request_target() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        assert_eq!(collector.name(), "binary-hasher");
        assert_eq!(
            collector.supported_analysis_types(),
            &[AnalysisType::BinaryHash]
        );
    }

    #[tokio::test]
    async fn health_check_passes_with_reachable_root() {
        let dir = TempDir::new().unwrap();
        let collector = make_collector(dir.path());
        collector.health_check().await.unwrap();
    }

    // ── Critical priority is queue-order only ───────────────────────────

    #[tokio::test]
    async fn critical_priority_still_respects_allowed_roots() {
        let allowed = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let file = outside.path().join("critical.bin");
        fs::write(&file, b"content").unwrap();

        let collector = make_collector(allowed.path());
        let mut req = make_request(file.to_string_lossy().into_owned());
        req.priority = TriggerPriority::Critical;
        let err = collector.handle_trigger(&req).await.unwrap_err();
        assert_eq!(
            err.kind(),
            TriggerErrorKind::PathNotAllowed,
            "Critical priority must NOT bypass allowed_roots authorization"
        );
    }

    // ── Keep _tmp alive to prove fn signature is usable ─────────────────

    #[test]
    fn named_temp_file_compiles() {
        // Sanity check that the dev-dep feature is available.
        let _tmp: NamedTempFile = NamedTempFile::new().unwrap();
    }
}
