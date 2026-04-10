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
//! # TOCTOU defense (cap-std)
//!
//! As of P1 Phase 3, path authorization uses cap-std `Dir` handles
//! opened at construction time (before privilege drop). Each allowed
//! root is a `Dir` handle; requests are opened relative to the root's
//! fd via `Dir::open()`, which the kernel resolves atomically against
//! the handle's subtree. This eliminates the `canonicalize()` →
//! `File::open()` TOCTOU gap.
//!
//! On macOS, an additional `(st_dev, st_ino)` fingerprint is recorded
//! at `Dir::open_ambient_dir` time and verified before each open. This
//! detects bind-mount / volume swaps (not atomic, but raises attacker
//! cost above zero).
//!
//! # Known gaps (follow-up work)
//!
//! - **Windows junction / reparse-point rejection**: requires calling the
//!   Win32 `GetFileInformationByHandleEx` API via the `windows` crate; the
//!   current stdlib-only path relies on `symlink_metadata().is_symlink()`
//!   which does not cover all reparse-point types.

use crate::analysis_chain::AnalysisResult;
use crate::event::{AnalysisType, TriggerRequest};
use crate::triggerable::{TriggerHandleError, TriggerableCollector};
use cap_std::fs::Dir;
use daemoneye_lib::integrity::{HashComputer, HashError, MultiAlgorithmHasher, auth::AuthError};
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
// AllowedRoot — cap-std Dir handle + metadata
// ─────────────────────────────────────────────────────────────────────────────

/// An allowed root directory opened via cap-std.
///
/// Holds the display path (for logging), the kernel-level `Dir` handle
/// (for TOCTOU-safe opens), and on macOS a `(st_dev, st_ino)` fingerprint
/// recorded at open time to detect bind-mount / volume swaps.
///
/// **Lifecycle invariant**: `AllowedRoot::open` MUST be called before
/// procmond drops privileges. The `Dir` handle persists across privilege
/// drops and confines all subsequent opens to the root's subtree at the
/// kernel level.
pub struct AllowedRoot {
    /// Human-readable (canonical) path for logging.
    display: String,
    /// Original path before canonicalization (for `strip_prefix` on
    /// macOS where `/var` → `/private/var`).
    original: String,
    /// Capability handle opened via `Dir::open_ambient_dir`.
    handle: Dir,
    /// On macOS, `(st_dev, st_ino)` recorded at open time.
    #[cfg(target_os = "macos")]
    fingerprint: (u64, u64),
}

impl std::fmt::Debug for AllowedRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowedRoot")
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

impl AllowedRoot {
    /// Open a root directory via `Dir::open_ambient_dir`.
    ///
    /// On macOS, also records `(st_dev, st_ino)` for fingerprint
    /// verification.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the directory does not exist or
    /// cannot be opened.
    pub fn open(root: &Path) -> Result<Self, std::io::Error> {
        let original = root.display().to_string();
        // Canonicalize so strip_prefix works on macOS (/var → /private/var).
        let canonical_root = std::fs::canonicalize(root)?;
        let handle = Dir::open_ambient_dir(&canonical_root, cap_std::ambient_authority())?;

        #[cfg(target_os = "macos")]
        let fingerprint = {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(&canonical_root)?;
            (meta.dev(), meta.ino())
        };

        Ok(Self {
            display: canonical_root.display().to_string(),
            original,
            handle,
            #[cfg(target_os = "macos")]
            fingerprint,
        })
    }

    /// Borrow the cap-std `Dir` handle.
    #[must_use]
    pub const fn handle(&self) -> &Dir {
        &self.handle
    }

    /// Display path for logging.
    #[must_use]
    pub fn display_path(&self) -> &str {
        &self.display
    }

    /// On macOS, verify the fingerprint still matches.
    #[cfg(target_os = "macos")]
    pub fn verify_fingerprint(&self) -> Result<(), AuthError> {
        use std::os::unix::fs::MetadataExt;
        let current =
            std::fs::metadata(Path::new(&self.display)).map_err(|source| AuthError::Io {
                path: PathBuf::from(&self.display),
                source,
            })?;
        let current_fp = (current.dev(), current.ino());
        if current_fp != self.fingerprint {
            return Err(AuthError::RootMountChanged {
                root: self.display.clone(),
            });
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// authorize_confined_path — cap-std gated authorization
// ─────────────────────────────────────────────────────────────────────────────

/// The well-known cap-std error message for path escapes.
///
/// Regression-tested to catch upstream message changes on cap-std
/// upgrades.
pub const CAP_STD_ESCAPE_MESSAGE: &str = "a path led outside of the filesystem";

/// Verify fingerprints for all allowed roots on macOS.
///
/// This MUST be called once before entering any per-file hash stream to
/// detect bind-mount / volume swaps for all roots up front, rather than
/// checking inside the per-file loop. Callers (e.g.
/// [`BinaryHasherCollector::authorize`]) should invoke this before
/// calling [`authorize_confined_path`].
///
/// On non-macOS platforms this is a no-op that always returns `Ok(())`.
///
/// # Errors
///
/// Returns [`AuthError::RootMountChanged`] if any root's `(st_dev, st_ino)`
/// no longer matches the fingerprint recorded at open time, or
/// [`AuthError::Io`] if the root metadata cannot be read.
pub fn verify_all_fingerprints(roots: &[AllowedRoot]) -> Result<(), AuthError> {
    #[cfg(target_os = "macos")]
    for root in roots {
        root.verify_fingerprint()?;
    }
    // Suppress unused-variable warning on non-macOS.
    #[cfg(not(target_os = "macos"))]
    let _ = roots;
    Ok(())
}

/// Authorize and open a target path via cap-std `Dir` handles.
///
/// Tries each allowed root in order. For each root:
/// 1. Strip the root prefix from the target to get a relative path.
/// 2. Open the relative path via `root.handle().open()`.
/// 3. Fetch metadata from the opened file handle (fstat — no second path stat).
///
/// Returns both the opened file and its metadata so callers can perform
/// file-type and size checks without issuing a second path-based stat(2).
///
/// If the path escapes (cap-std returns the escape error), this is
/// mapped to [`AuthError::CapStdEscape`]. If no root matches, returns
/// [`AuthError::PathNotAllowed`].
///
/// **Fingerprint pre-check**: On macOS, callers MUST call
/// [`verify_all_fingerprints`] once before entering the per-file stream.
/// `authorize_confined_path` does NOT re-verify fingerprints to avoid
/// redundant stat calls on every file in a batch.
///
/// # Errors
///
/// Returns [`AuthError`] on escape, I/O errors, or if the path is not
/// under any allowed root.
pub fn authorize_confined_path(
    target: &Path,
    roots: &[AllowedRoot],
    follow_symlinks: bool,
) -> Result<(cap_std::fs::File, cap_std::fs::Metadata), AuthError> {
    use daemoneye_lib::integrity::auth;

    // Pre-flight checks using shared predicates.
    auth::check_path_length(target)?;
    auth::check_no_traversal(target)?;

    for root in roots {
        // WHY two strip_prefix attempts?
        //
        // On macOS, `/var` is a symlink to `/private/var`. `AllowedRoot::open`
        // calls `std::fs::canonicalize` on the configured root path, so the
        // `Dir` handle is always opened against the real `/private/var/...`
        // path and `display` (the `canonical_root` here) will always start
        // with `/private/var`. However, process enumeration via `sysinfo` (or
        // other OS APIs) may return the *un-resolved* `/var/...` form for the
        // executable path, depending on how the kernel surfaces it.
        //
        // To bridge that gap we keep the pre-canonicalization path in
        // `AllowedRoot::original` and try stripping both prefixes. If either
        // matches we obtain a valid relative path and proceed.
        //
        // SECURITY: This dual-try is NOT a confinement bypass. `strip_prefix`
        // only computes the *relative* path that is handed to cap-std's
        // `Dir::open` / `Dir::open_with`. The confinement guarantee is
        // enforced entirely by the kernel: the `Dir` fd was opened at
        // `AllowedRoot::open` time (canonicalized, before any privilege drop),
        // and all subsequent `open` calls are resolved *relative to that fd*.
        // The kernel will refuse traversal outside the fd's subtree regardless
        // of which string prefix we stripped. cap-std additionally detects any
        // escape attempt and returns [`CAP_STD_ESCAPE_MESSAGE`], which we map
        // to [`AuthError::CapStdEscape`] below.
        //
        // The `original` field on `AllowedRoot` exists specifically for this
        // macOS `/var` ↔ `/private/var` compatibility case.
        let canonical_root = Path::new(root.display_path());
        let original_root = Path::new(&root.original);
        let relative = if let Ok(rel) = target.strip_prefix(canonical_root) {
            rel
        } else if let Ok(rel) = target.strip_prefix(original_root) {
            rel
        } else {
            continue;
        };

        // Open via cap-std. This is TOCTOU-safe: the kernel resolves
        // the path relative to the Dir handle's fd.
        let open_result = if follow_symlinks {
            // Use cap-fs-ext to explicitly follow symlinks on the
            // final component.
            use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
            let mut opts = cap_std::fs::OpenOptions::new();
            opts.read(true);
            opts.follow(FollowSymlinks::Yes);
            root.handle().open_with(relative, &opts)
        } else {
            // Check symlink_metadata first to reject symlinks before
            // opening.
            let meta_result = root.handle().symlink_metadata(relative);
            match meta_result {
                Ok(ref meta) if meta.is_symlink() => {
                    return Err(AuthError::SymlinkRejected {
                        path: target.to_path_buf(),
                    });
                }
                Err(ref err) => {
                    return Err(cap_std_err_to_auth(err, target));
                }
                Ok(_) => root.handle().open(relative),
            }
        };

        match open_result {
            Ok(file) => {
                let meta = file.metadata().map_err(|source| AuthError::Io {
                    path: target.to_path_buf(),
                    source,
                })?;
                return Ok((file, meta));
            }
            Err(ref err) if err.to_string().contains(CAP_STD_ESCAPE_MESSAGE) => {
                return Err(AuthError::CapStdEscape {
                    message: err.to_string(),
                });
            }
            Err(err) => {
                return Err(cap_std_err_to_auth(&err, target));
            }
        }
    }

    Err(AuthError::PathNotAllowed {
        path: target.to_path_buf(),
    })
}

/// Map a cap-std I/O error to [`AuthError`].
fn cap_std_err_to_auth(err: &std::io::Error, target: &Path) -> AuthError {
    if err.to_string().contains(CAP_STD_ESCAPE_MESSAGE) {
        AuthError::CapStdEscape {
            message: err.to_string(),
        }
    } else {
        AuthError::Io {
            path: target.to_path_buf(),
            source: std::io::Error::new(err.kind(), err.to_string()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BinaryHasherConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`BinaryHasherCollector`].
#[derive(Debug, Clone)]
pub struct BinaryHasherConfig {
    /// Mandatory allow-list of root directories (as paths).
    ///
    /// These are opened as cap-std `Dir` handles at construction time.
    /// **Empty list denies all requests.** Use
    /// [`BinaryHasherConfig::with_platform_defaults`] for secure defaults.
    pub allowed_roots: Vec<PathBuf>,
    /// When `false` (default), any path that resolves through a symbolic
    /// link or junction is rejected with `Unavailable`. When `true`, the
    /// resolved real path must still pass the `allowed_roots` check.
    pub follow_symlinks: bool,
    /// Maximum accepted file size, enforced before any read.
    pub max_file_size: u64,
    /// Per-file hashing deadline propagated to the engine.
    pub timeout_per_file: Duration,
    /// When `false` (default), startup aborts if any configured root
    /// fails to open. When `true`, roots that fail to open are skipped
    /// with a warning.
    pub allow_partial_roots: bool,
}

impl Default for BinaryHasherConfig {
    fn default() -> Self {
        Self {
            allowed_roots: Vec::new(),
            follow_symlinks: false,
            max_file_size: 512 * 1024 * 1024,
            timeout_per_file: Duration::from_secs(10),
            allow_partial_roots: false,
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
///
/// The `opened_roots` are cap-std `Dir` handles opened at construction
/// time (before privilege drop) and used for TOCTOU-safe path
/// resolution on every trigger.
pub struct BinaryHasherCollector {
    engine: Arc<MultiAlgorithmHasher>,
    config: BinaryHasherConfig,
    opened_roots: Vec<AllowedRoot>,
}

impl std::fmt::Debug for BinaryHasherCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryHasherCollector")
            .field("config", &self.config)
            .field("opened_roots", &self.opened_roots)
            .finish_non_exhaustive()
    }
}

impl BinaryHasherCollector {
    /// Construct a collector with a shared engine and a validated config.
    ///
    /// Opens each `allowed_root` as a cap-std `Dir` handle. In strict
    /// mode (default), any root that fails to open aborts construction.
    /// In partial mode (`allow_partial_roots = true`), failed roots are
    /// skipped with a warning.
    ///
    /// **Lifecycle invariant**: This MUST be called before procmond drops
    /// privileges, so the `Dir` handles inherit the elevated fd.
    ///
    /// # Errors
    ///
    /// Returns [`TriggerHandleError::Internal`] if `config.validate()`
    /// fails, or in strict mode if any root fails to open.
    pub fn new(
        engine: Arc<MultiAlgorithmHasher>,
        config: BinaryHasherConfig,
    ) -> Result<Self, TriggerHandleError> {
        config.validate()?;

        let mut opened_roots = Vec::with_capacity(config.allowed_roots.len());
        for root_path in &config.allowed_roots {
            match AllowedRoot::open(root_path) {
                Ok(root) => {
                    debug!(root = %root_path.display(), "opened allowed root");
                    opened_roots.push(root);
                }
                Err(err) => {
                    if config.allow_partial_roots {
                        warn!(
                            root = %root_path.display(),
                            error = %err,
                            "skipping allowed root (partial mode)"
                        );
                    } else {
                        return Err(TriggerHandleError::Internal(format!(
                            "failed to open allowed root {}: {err}",
                            root_path.display()
                        )));
                    }
                }
            }
        }

        if opened_roots.is_empty() {
            return Err(TriggerHandleError::Internal(
                "no allowed roots could be opened".to_owned(),
            ));
        }

        Ok(Self {
            engine,
            config,
            opened_roots,
        })
    }

    /// Stable collector name used to match `TriggerRequest.target_collector`.
    pub const NAME: &'static str = "binary-hasher";

    // ── Path authorization ──────────────────────────────────────────────

    /// Authorize a request via cap-std `Dir` handles.
    ///
    /// Delegates to [`authorize_confined_path`] for TOCTOU-safe opens,
    /// then performs the file-type and size checks.
    ///
    /// 1. On macOS, verify all root fingerprints once (before the per-file open).
    /// 2. Length cap + parent-traversal rejection (via shared predicates).
    /// 3. cap-std confined open (TOCTOU-safe path resolution).
    /// 4. File-type check (must be a regular file).
    /// 5. Size check against `config.max_file_size`.
    fn authorize(&self, raw: &str) -> Result<PathBuf, TriggerHandleError> {
        // Verify all root fingerprints once before entering the per-file
        // stream. On non-macOS this is a no-op.
        verify_all_fingerprints(&self.opened_roots).map_err(|err| map_auth_err(&err))?;

        if raw.len() > MAX_TARGET_PATH_LEN {
            return Err(TriggerHandleError::PathTooLong { len: raw.len() });
        }
        if raw.is_empty() {
            return Err(TriggerHandleError::PathRejected {
                reason: "empty path".to_owned(),
            });
        }

        let raw_path = Path::new(raw);

        // Delegate to cap-std authorization. Returns the opened file handle
        // and its metadata (via fstat) so we avoid a second path-based stat.
        let (_file, meta) =
            authorize_confined_path(raw_path, &self.opened_roots, self.config.follow_symlinks)
                .map_err(|err| map_auth_err(&err))?;

        // File-type and size checks use the metadata already obtained from
        // the open file handle — no second stat(2) call needed.
        if !meta.is_file() {
            return Err(TriggerHandleError::Unavailable {
                reason: "target is not a regular file".to_owned(),
            });
        }
        if meta.len() > self.config.max_file_size {
            return Err(TriggerHandleError::TooLarge {
                size: meta.len(),
                limit: self.config.max_file_size,
            });
        }

        // When following symlinks, canonicalize to the resolved path so
        // the engine hashes the real file (not the symlink). The cap-std
        // open already proved confinement.
        let resolved = if self.config.follow_symlinks {
            std::fs::canonicalize(raw_path).map_err(|err| map_path_io_err("canonicalize", &err))?
        } else {
            raw_path.to_path_buf()
        };

        Ok(resolved)
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
        // Health is good if at least one opened root is accessible.
        for root in &self.opened_roots {
            let root_path = Path::new(root.display_path());
            if std::fs::metadata(root_path).is_ok() {
                return Ok(());
            }
        }
        Err(TriggerHandleError::Internal(
            "no opened_roots are currently accessible".to_owned(),
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Map an [`AuthError`] to a wire-safe [`TriggerHandleError`].
///
/// Maps path-escape and not-allowed to `PathNotAllowed`, symlink
/// rejection and I/O to `Unavailable`, and everything else to
/// `Internal`. This prevents file-existence oracles.
#[allow(
    clippy::wildcard_enum_match_arm,
    clippy::pattern_type_mismatch,
    reason = "AuthError is #[non_exhaustive]; wildcard required for forward-compat"
)]
fn map_auth_err(err: &AuthError) -> TriggerHandleError {
    match err {
        AuthError::PathTooLong { len, .. } => TriggerHandleError::PathTooLong { len: *len },
        AuthError::PathTraversal { .. } => TriggerHandleError::PathRejected {
            reason: "path contains '..' component".to_owned(),
        },
        AuthError::CapStdEscape { .. } | AuthError::PathNotAllowed { .. } => {
            TriggerHandleError::PathNotAllowed
        }
        AuthError::SymlinkRejected { .. } => TriggerHandleError::Unavailable {
            reason: "target is a symlink".to_owned(),
        },
        AuthError::Io { .. } => TriggerHandleError::Unavailable {
            reason: format!("I/O error: {err}"),
        },
        _ => TriggerHandleError::Internal(format!("auth error: {err}")),
    }
}

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
    use daemoneye_lib::integrity::{HashAlgorithm, HasherConfig};
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
        // Use a relative symlink so cap-std doesn't reject it as an
        // escape (absolute symlink targets point outside the Dir sandbox).
        std::os::unix::fs::symlink(Path::new("real.bin"), &link).unwrap();

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
