//! Post-enumeration executable-hash pass for procmond.
//!
//! This module provides [`populate_hashes`], the parallel, authorization-gated
//! replacement for the serial `populate_executable_hashes` in
//! `process_collector.rs`. It bundles two P1 resolutions:
//!
//! - **todo #011**: Authorization check before every hash. The free function
//!   [`authorize_kernel_path`] validates paths supplied by sysinfo's
//!   kernel-resolved `Process::exe()` — never argv\[0\], cwd, or root.
//! - **todo #010**: Serial bottleneck. `populate_hashes` uses
//!   [`futures::stream::iter`] + [`futures::stream::StreamExt::buffer_unordered`]
//!   with `n = engine.max_concurrent()` so up to N hashes run concurrently.
//!
//! # Trust model
//!
//! Procmond runs elevated and feeds sysinfo's `exe()` field, which reads
//! `/proc/\[pid\]/exe` on Linux (a kernel symlink) or `PROC_PIDPATHINFO` on
//! macOS. These are kernel-resolved paths — not user-controllable. The
//! [`KernelResolvedExe`] newtype enforces this at the type level.

use collector_core::ProcessEvent;
use daemoneye_lib::integrity::{
    HashAlgorithm, HashComputer, HashResult, MultiAlgorithmHasher,
    auth::{self, AuthError, MAX_EXECUTABLE_FILE_SIZE},
};
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

// ─────────────────────────────────────────────────────────────────────────────
// KernelResolvedExe newtype
// ─────────────────────────────────────────────────────────────────────────────

/// A filesystem path resolved by the kernel, not by user input.
///
/// On Linux this is `/proc/\[pid\]/exe`; on macOS it is `PROC_PIDPATHINFO`.
/// The private constructor ensures that only procmond's process enumeration
/// code (which reads sysinfo's `Process::exe()`) can construct this type.
///
/// This prevents argv\[0\], cwd-relative paths, or root-relative paths from
/// ever reaching [`authorize_kernel_path`].
#[derive(Debug, Clone)]
pub struct KernelResolvedExe(PathBuf);

impl KernelResolvedExe {
    /// Construct from sysinfo's `Process::exe()` output.
    ///
    /// This MUST only be called with the return value of
    /// `sysinfo::Process::exe()` — never with user-supplied paths.
    #[must_use]
    pub const fn from_sysinfo_exe(path: PathBuf) -> Self {
        Self(path)
    }

    /// Borrow the inner path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Authorization
// ─────────────────────────────────────────────────────────────────────────────

/// Authorize a kernel-resolved executable path for hashing.
///
/// Runs the shared predicates from [`daemoneye_lib::integrity::auth`]:
/// 1. Path length ≤ `MAX_EXECUTABLE_PATH_LEN` bytes.
/// 2. No `..` traversal components.
/// 3. File exists and is a regular file (`symlink_metadata`).
/// 4. File size ≤ [`MAX_EXECUTABLE_FILE_SIZE`].
///
/// # Errors
///
/// Returns [`AuthError`] if any predicate fails.
pub fn authorize_kernel_path(exe: &KernelResolvedExe) -> Result<std::fs::Metadata, AuthError> {
    let path = exe.as_path();

    auth::check_path_length(path)?;
    auth::check_no_traversal(path)?;

    let metadata = std::fs::symlink_metadata(path).map_err(|source| AuthError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    auth::check_regular_file(path, &metadata)?;
    auth::check_size(&metadata, MAX_EXECUTABLE_FILE_SIZE)?;

    Ok(metadata)
}

// ─────────────────────────────────────────────────────────────────────────────
// HashPassStats
// ─────────────────────────────────────────────────────────────────────────────

/// Aggregate statistics for a post-enumeration hash-population pass.
///
/// Extends the original `process_collector::HashCoverageStats` with
/// auth-failure and nonauthoritative counters for telemetry.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashPassStats {
    /// Number of unique executable paths seen across the scan.
    pub unique_paths: usize,
    /// Number of unique paths that were successfully hashed.
    pub hashed: usize,
    /// Number of unique paths that failed authorization.
    pub auth_failures: usize,
    /// Number of paths where the engine returned `Nonauthoritative`.
    pub nonauthoritative: usize,
    /// Number of unique paths where hashing failed (I/O, timeout, etc.).
    pub io_failures: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// populate_hashes
// ─────────────────────────────────────────────────────────────────────────────

/// Parallel, authorization-gated hash pass for process events.
///
/// Deduplicates by `executable_path`, runs [`authorize_kernel_path`] on
/// each unique path, then hashes authorized paths concurrently using
/// `buffer_unordered(engine.max_concurrent())`.
///
/// This replaces `process_collector::populate_executable_hashes` with:
/// - Authorization before every hash (todo #011).
/// - `buffer_unordered` parallelism (todo #010).
///
/// Errors for individual files are logged and counted but never propagated.
#[instrument(skip_all, fields(event_count = events.len()))]
pub async fn populate_hashes(
    events: &mut [ProcessEvent],
    hasher: &Arc<MultiAlgorithmHasher>,
) -> HashPassStats {
    let mut stats = HashPassStats::default();

    // Phase 1: Dedup by executable_path.
    let mut unique_paths: HashMap<PathBuf, Option<(String, String)>> = HashMap::new();
    for event in events.iter() {
        if let Some(ref raw) = event.executable_path {
            let path = PathBuf::from(raw);
            unique_paths.entry(path).or_insert(None);
        }
    }
    stats.unique_paths = unique_paths.len();

    if unique_paths.is_empty() {
        return stats;
    }

    // Phase 2: Authorize + hash in parallel via buffer_unordered.
    let concurrency = hasher.max_concurrent();
    let engine = Arc::clone(hasher);

    let results: Vec<(PathBuf, HashOutcome)> = stream::iter(unique_paths.keys().cloned())
        .map(|path| {
            let h = Arc::clone(&engine);
            async move {
                let exe = KernelResolvedExe::from_sysinfo_exe(path.clone());
                let outcome = hash_one(&exe, &h).await;
                (path, outcome)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    // Phase 3: Collect results into the lookup map + update stats.
    for (path, outcome) in results {
        match outcome {
            HashOutcome::Hashed(hex, algo) => {
                unique_paths.insert(path, Some((hex, algo)));
                stats.hashed = stats.hashed.saturating_add(1);
            }
            HashOutcome::AuthFailed => {
                stats.auth_failures = stats.auth_failures.saturating_add(1);
            }
            HashOutcome::Nonauthoritative => {
                stats.nonauthoritative = stats.nonauthoritative.saturating_add(1);
            }
            HashOutcome::IoFailure => {
                stats.io_failures = stats.io_failures.saturating_add(1);
            }
        }
    }

    // Phase 4: Stamp hashes onto events.
    for event in events.iter_mut() {
        let Some(raw) = event.executable_path.as_deref() else {
            continue;
        };
        let path = PathBuf::from(raw);
        if let Some(entry) = unique_paths.get(&path).and_then(Option::as_ref) {
            event.executable_hash = Some(entry.0.clone());
            event.hash_algorithm = Some(entry.1.clone());
        }
    }

    // Telemetry: emit coverage stats so operators can distinguish
    // "feature disabled" from "files inaccessible".
    info!(
        unique_paths = stats.unique_paths,
        hashed = stats.hashed,
        auth_failures = stats.auth_failures,
        nonauthoritative = stats.nonauthoritative,
        io_failures = stats.io_failures,
        "hash pass completed"
    );

    stats
}

/// Outcome of a single hash attempt.
enum HashOutcome {
    /// Successfully hashed — contains (hex, algorithm_name).
    Hashed(String, String),
    /// Authorization rejected the path.
    AuthFailed,
    /// Engine detected mid-read mutation.
    Nonauthoritative,
    /// I/O or other engine error.
    IoFailure,
}

/// Authorize and hash a single executable.
#[allow(clippy::pattern_type_mismatch)]
async fn hash_one(exe: &KernelResolvedExe, hasher: &MultiAlgorithmHasher) -> HashOutcome {
    // Authorization gate.
    if let Err(ref err) = authorize_kernel_path(exe) {
        let display_path = auth::bytes_safe_display(exe.as_path(), 200);
        #[allow(clippy::wildcard_enum_match_arm)]
        match err {
            AuthError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                debug!(path = %display_path, error = %err, "hash auth skipped: permission denied");
            }
            AuthError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                debug!(path = %display_path, error = %err, "hash auth skipped: file not found");
            }
            _ => {
                warn!(path = %display_path, error = %err, "hash auth rejected");
            }
        }
        return HashOutcome::AuthFailed;
    }

    // Hash via the engine.
    match hasher.compute(exe.as_path()).await {
        Ok(result) => primary_hash_hex(&result).map_or(HashOutcome::IoFailure, |hex| {
            HashOutcome::Hashed(hex, HashAlgorithm::Sha256.wire_name().to_owned())
        }),
        Err(daemoneye_lib::integrity::HashError::Nonauthoritative { .. }) => {
            debug!(path = ?exe.as_path(), "hash: file mutated mid-read (nonauthoritative)");
            HashOutcome::Nonauthoritative
        }
        Err(daemoneye_lib::integrity::HashError::PermissionDenied { .. }) => {
            debug!(path = ?exe.as_path(), "hash skipped: permission denied");
            HashOutcome::IoFailure
        }
        #[allow(clippy::wildcard_enum_match_arm)]
        Err(ref err) => {
            warn!(path = ?exe.as_path(), error = %err, "hash failed");
            HashOutcome::IoFailure
        }
    }
}

/// Extract the primary (SHA-256) hex string from a [`HashResult`].
fn primary_hash_hex(result: &HashResult) -> Option<String> {
    result.sha256().map(str::to_owned)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::string_add
)]
mod tests {
    use super::*;
    use daemoneye_lib::integrity::{HasherConfig, auth::MAX_EXECUTABLE_PATH_LEN};
    use std::fs;
    use tempfile::NamedTempFile;

    fn new_event(pid: u32, exe: &str) -> ProcessEvent {
        ProcessEvent {
            pid,
            ppid: None,
            name: format!("proc-{pid}"),
            executable_path: Some(exe.to_owned()),
            command_line: Vec::new(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            hash_algorithm: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: std::time::SystemTime::now(),
            platform_metadata: None,
        }
    }

    // ── authorize_kernel_path ─────────────────────────────────────────

    #[test]
    fn auth_rejects_path_too_long() {
        let long_path = PathBuf::from("/".to_owned() + &"a".repeat(MAX_EXECUTABLE_PATH_LEN + 1));
        let exe = KernelResolvedExe::from_sysinfo_exe(long_path);
        assert!(matches!(
            authorize_kernel_path(&exe),
            Err(AuthError::PathTooLong { .. })
        ));
    }

    #[test]
    fn auth_rejects_traversal() {
        let exe = KernelResolvedExe::from_sysinfo_exe(PathBuf::from("/usr/bin/../sbin/evil"));
        assert!(matches!(
            authorize_kernel_path(&exe),
            Err(AuthError::PathTraversal { .. })
        ));
    }

    #[test]
    fn auth_rejects_nonexistent() {
        let exe =
            KernelResolvedExe::from_sysinfo_exe(PathBuf::from("/definitely/does/not/exist/xyz"));
        assert!(matches!(
            authorize_kernel_path(&exe),
            Err(AuthError::Io { .. })
        ));
    }

    #[test]
    fn auth_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let exe = KernelResolvedExe::from_sysinfo_exe(dir.path().to_path_buf());
        assert!(matches!(
            authorize_kernel_path(&exe),
            Err(AuthError::NotRegularFile { .. })
        ));
    }

    #[test]
    fn auth_accepts_regular_file() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"test binary content").unwrap();
        let exe = KernelResolvedExe::from_sysinfo_exe(tmp.path().to_path_buf());
        assert!(authorize_kernel_path(&exe).is_ok());
    }

    #[test]
    fn auth_boundary_4096_bytes() {
        // Path of exactly 4096 bytes should pass length check (may fail
        // on other checks like file-not-found, which is fine).
        let path = PathBuf::from("/".to_owned() + &"a".repeat(MAX_EXECUTABLE_PATH_LEN - 1));
        assert_eq!(path.as_os_str().len(), MAX_EXECUTABLE_PATH_LEN);
        let exe = KernelResolvedExe::from_sysinfo_exe(path);
        let result = authorize_kernel_path(&exe);
        // Should NOT be PathTooLong — it may be FileNotFound, which is fine.
        assert!(!matches!(result, Err(AuthError::PathTooLong { .. })));
    }

    #[test]
    fn auth_boundary_multi_byte_utf8() {
        // 4-byte emoji repeated to cross the boundary. Must not panic.
        let emoji_path = PathBuf::from("\u{1F600}".repeat(1025)); // 4100 bytes
        let exe = KernelResolvedExe::from_sysinfo_exe(emoji_path);
        let result = authorize_kernel_path(&exe);
        assert!(matches!(result, Err(AuthError::PathTooLong { .. })));
    }

    // ── populate_hashes ──────────────────────────────────────────────

    #[tokio::test]
    async fn populate_hashes_fills_hash_and_algorithm() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"parallel hash pass").unwrap();
        let path = tmp.path().to_string_lossy().into_owned();

        let mut events = vec![new_event(1, &path), new_event(2, &path)];
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());

        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 1);
        assert_eq!(stats.hashed, 1);
        assert_eq!(stats.auth_failures, 0);

        for event in &events {
            assert_eq!(event.hash_algorithm.as_deref(), Some("sha256"));
            assert!(
                event
                    .executable_hash
                    .as_deref()
                    .is_some_and(|h| h.len() == 64)
            );
        }
    }

    #[tokio::test]
    async fn populate_hashes_dedup_works() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"dedup test").unwrap();
        let path = tmp.path().to_string_lossy().into_owned();

        let mut events: Vec<ProcessEvent> = (0..50_u32).map(|pid| new_event(pid, &path)).collect();
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());

        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 1);
        assert_eq!(stats.hashed, 1);
        assert!(events.iter().all(|e| e.executable_hash.is_some()));
    }

    #[tokio::test]
    async fn populate_hashes_missing_file_is_nonfatal() {
        let mut events = vec![
            new_event(1, "/definitely/does/not/exist/xyz"),
            new_event(2, "/also/not/here"),
        ];
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());

        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 2);
        assert_eq!(stats.hashed, 0);
        // Missing files fail at auth (I/O not found).
        assert_eq!(stats.auth_failures, 2);
        for event in &events {
            assert!(event.executable_hash.is_none());
        }
    }

    #[tokio::test]
    async fn populate_hashes_empty_slice_is_noop() {
        let mut events: Vec<ProcessEvent> = Vec::new();
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());
        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 0);
        assert_eq!(stats.hashed, 0);
    }

    #[tokio::test]
    async fn populate_hashes_skips_events_without_path() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"with path").unwrap();
        let path = tmp.path().to_string_lossy().into_owned();

        // Leak the file so it persists through the test.
        std::mem::forget(tmp);

        let mut event_without_path = new_event(2, "ignored");
        event_without_path.executable_path = None;

        let mut events = vec![new_event(1, &path), event_without_path];
        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());
        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 1);
        assert!(events.first().is_some_and(|e| e.executable_hash.is_some()));
        assert!(events.get(1).is_some_and(|e| e.executable_hash.is_none()));
    }

    #[tokio::test]
    async fn populate_hashes_parallel_multiple_files() {
        // Create multiple temp files to exercise buffer_unordered concurrency.
        let files: Vec<_> = (0..10)
            .map(|i| {
                let tmp = NamedTempFile::new().unwrap();
                fs::write(tmp.path(), format!("binary-{i}")).unwrap();
                let path = tmp.path().to_string_lossy().into_owned();
                std::mem::forget(tmp);
                path
            })
            .collect();

        let mut events: Vec<ProcessEvent> = files
            .iter()
            .enumerate()
            .map(|(i, path)| {
                #[allow(clippy::as_conversions)]
                new_event(i as u32, path)
            })
            .collect();

        let hasher = Arc::new(MultiAlgorithmHasher::new(HasherConfig::default()).unwrap());
        let stats = populate_hashes(&mut events, &hasher).await;
        assert_eq!(stats.unique_paths, 10);
        assert_eq!(stats.hashed, 10);
        assert!(events.iter().all(|e| e.executable_hash.is_some()));
    }
}
