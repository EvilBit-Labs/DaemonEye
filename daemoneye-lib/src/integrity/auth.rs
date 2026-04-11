//! Shared authorization predicates for executable hashing.
//!
//! These helpers are consumed by both:
//! - `procmond/src/hash_pass.rs` (`authorize_kernel_path`) for the
//!   post-enumeration path where sysinfo supplies kernel-resolved exe paths.
//! - `collector-core/src/binary_hasher.rs` (`authorize_confined_path`) for
//!   the triggered path where cap-std `Dir` handles confine opens.
//!
//! All predicates operate on **byte lengths only** — they never index or
//! slice into the path string (CWE-135 safety).

use std::path::Path;
use thiserror::Error;

/// Linux `PATH_MAX`. Used as a sanity bound on executable paths.
///
/// This is a byte-length comparison only — no slicing, no indexing.
/// The prior value of 107 (Unix `sockaddr_un.sun_path`) was
/// architecturally incorrect: it conflates socket path limits with
/// filesystem path limits.
pub const MAX_EXECUTABLE_PATH_LEN: usize = 4096;

/// Maximum file size (bytes) for the authorization pre-open gate.
///
/// Distinct from the engine's `HasherConfig::max_file_size` — this rejects
/// absurdly large binaries before the engine ever opens them.
/// Defaults to 512 MiB (matches the engine default).
pub const MAX_EXECUTABLE_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// Errors from the authorization predicates.
///
/// These are distinct from [`super::HashError`] because authorization
/// happens *before* the engine opens the file. A path rejected here
/// never reaches the hash engine.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuthError {
    /// Path exceeds [`MAX_EXECUTABLE_PATH_LEN`] bytes.
    #[error("path too long: {len} bytes exceeds limit of {limit} bytes")]
    PathTooLong {
        /// Observed byte length.
        len: usize,
        /// Configured limit.
        limit: usize,
    },

    /// Path is not a regular file (symlink, directory, device, etc.).
    #[error("not a regular file: {}", path.display())]
    NotRegularFile {
        /// The path that failed the check.
        path: std::path::PathBuf,
    },

    /// File exceeds the authorization-layer size limit.
    #[error("file too large for hashing: {size} bytes exceeds {limit} bytes")]
    FileTooLarge {
        /// Observed file size.
        size: u64,
        /// Configured limit.
        limit: u64,
    },

    /// Path contains traversal components (`..`).
    #[error("path contains traversal component: {}", path.display())]
    PathTraversal {
        /// The path that failed the check.
        path: std::path::PathBuf,
    },

    /// Underlying I/O error during metadata checks.
    #[error("I/O error checking {}: {source}", path.display())]
    Io {
        /// The path being checked.
        path: std::path::PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// cap-std rejected the open because the path escaped the `Dir` root.
    ///
    /// This fires when a symlink, bind-mount, or `..` traversal attempts
    /// to reach outside the allowed root.
    #[error("path escaped allowed root: {message}")]
    CapStdEscape {
        /// The cap-std error message (preserved for regression testing).
        message: String,
    },

    /// Target is a symbolic link and the policy forbids following.
    #[error("target is a symlink: {}", path.display())]
    SymlinkRejected {
        /// The path that is a symlink.
        path: std::path::PathBuf,
    },

    /// Path is outside all configured allowed roots.
    #[error("path not under any allowed root: {}", path.display())]
    PathNotAllowed {
        /// The path that failed the check.
        path: std::path::PathBuf,
    },

    /// macOS: the allowed root's (`st_dev`, `st_ino`) fingerprint changed,
    /// indicating a bind-mount or volume swap.
    #[cfg(target_os = "macos")]
    #[error("root mount point changed: {root}")]
    RootMountChanged {
        /// The root whose fingerprint no longer matches.
        root: String,
    },
}

/// Check that `path`'s byte length does not exceed `MAX_EXECUTABLE_PATH_LEN`.
///
/// Uses `as_os_str().len()` which returns the byte length on Unix and the
/// WTF-8 byte length on Windows — never indexes or slices the string.
///
/// # Errors
///
/// Returns [`AuthError::PathTooLong`] if the path exceeds the limit.
pub fn check_path_length(path: &Path) -> Result<(), AuthError> {
    let len = path.as_os_str().len();
    if len > MAX_EXECUTABLE_PATH_LEN {
        return Err(AuthError::PathTooLong {
            len,
            limit: MAX_EXECUTABLE_PATH_LEN,
        });
    }
    Ok(())
}

/// Check that `path` contains no `..` traversal components.
///
/// # Errors
///
/// Returns [`AuthError::PathTraversal`] if any component is `..`.
pub fn check_no_traversal(path: &Path) -> Result<(), AuthError> {
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(AuthError::PathTraversal {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(())
}

/// Check that `metadata` describes a regular file (and not a symlink).
///
/// Also rejects symlinks explicitly via `file_type().is_symlink()` so the
/// no-symlink policy holds even if a caller accidentally passes
/// [`std::fs::metadata`] (which follows symlinks) instead of
/// [`std::fs::symlink_metadata`]. When the metadata comes from
/// `symlink_metadata`, a symlink has `file_type().is_symlink() == true`
/// and we return [`AuthError::SymlinkRejected`]. When it comes from
/// `metadata`, the target's type is reported and a non-file target still
/// trips the `!is_file()` branch below.
///
/// # Safety contract for callers
///
/// Callers that want to reject symlinks (the default no-follow policy in
/// this workspace) **must** obtain `metadata` via
/// [`std::fs::symlink_metadata`]. Passing regular `std::fs::metadata`
/// would silently follow the symlink, so the target is checked instead
/// of the link itself — this function cannot fully compensate for that
/// at the type level because it receives metadata, not a path.
///
/// # Errors
///
/// Returns [`AuthError::SymlinkRejected`] if the metadata is for a
/// symbolic link, or [`AuthError::NotRegularFile`] if it is not a
/// regular file for any other reason.
pub fn check_regular_file(path: &Path, metadata: &std::fs::Metadata) -> Result<(), AuthError> {
    if metadata.file_type().is_symlink() {
        return Err(AuthError::SymlinkRejected {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(AuthError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Check that the file size does not exceed `limit`.
///
/// # Errors
///
/// Returns [`AuthError::FileTooLarge`] if `metadata.len() > limit`.
pub fn check_size(metadata: &std::fs::Metadata, limit: u64) -> Result<(), AuthError> {
    let size = metadata.len();
    if size > limit {
        return Err(AuthError::FileTooLarge { size, limit });
    }
    Ok(())
}

/// Truncate a path to at most `max_bytes` for safe logging.
///
/// Uses a per-`char` UTF-8-aware accumulator so the truncation point always
/// falls on a valid code-point boundary — never producing a partial
/// multi-byte sequence (CWE-135). Non-UTF-8 paths are lossily converted
/// first. The returned string is guaranteed to be at most `max_bytes` bytes
/// long, including the trailing `"..."` marker. When `max_bytes <= 3` and
/// the path overflows the budget, the result is at most `max_bytes`
/// characters of the ellipsis (never slicing mid-char).
#[must_use]
pub fn bytes_safe_display(path: &Path, max_bytes: usize) -> String {
    let lossy = path.to_string_lossy();
    let byte_len = lossy.len();
    if byte_len <= max_bytes {
        return lossy.into_owned();
    }
    // Tiny budgets: return at most max_bytes bytes of ellipsis, never
    // slicing mid-char. Each '.' is one byte so this is byte-safe.
    if max_bytes <= 3 {
        return "...".chars().take(max_bytes).collect();
    }
    // Reserve 3 bytes for the trailing ellipsis, then collect code points
    // whose UTF-8 length fits in the remaining budget.
    let budget = max_bytes.saturating_sub(3);
    let mut truncated = String::with_capacity(max_bytes);
    let mut used: usize = 0;
    for ch in lossy.chars() {
        let ch_len = ch.len_utf8();
        if used.saturating_add(ch_len) > budget {
            break;
        }
        truncated.push(ch);
        used = used.saturating_add(ch_len);
    }
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_length_at_boundary() {
        // Exactly 4096 bytes — should pass.
        let path = PathBuf::from("a".repeat(MAX_EXECUTABLE_PATH_LEN));
        assert!(check_path_length(&path).is_ok());
    }

    #[test]
    fn path_length_one_over() {
        let path = PathBuf::from("a".repeat(MAX_EXECUTABLE_PATH_LEN + 1));
        assert!(matches!(
            check_path_length(&path),
            Err(AuthError::PathTooLong { .. })
        ));
    }

    #[test]
    fn path_length_with_emoji_no_panic() {
        // 4096 emoji characters = 16384 bytes. Must not panic (CWE-135).
        let emoji_path = PathBuf::from("\u{1F600}".repeat(MAX_EXECUTABLE_PATH_LEN));
        let result = check_path_length(&emoji_path);
        assert!(matches!(result, Err(AuthError::PathTooLong { .. })));
    }

    #[test]
    fn traversal_detected() {
        let path = PathBuf::from("/usr/bin/../sbin/evil");
        assert!(matches!(
            check_no_traversal(&path),
            Err(AuthError::PathTraversal { .. })
        ));
    }

    #[test]
    fn clean_path_passes_traversal() {
        let path = PathBuf::from("/usr/bin/ls");
        assert!(check_no_traversal(&path).is_ok());
    }

    #[test]
    fn bytes_safe_display_truncates_on_char_boundary() {
        // 12-byte emoji path with budget 7: reserve 3 bytes for "...",
        // leaving 4 bytes -> fits exactly one emoji (4 bytes) + "...".
        let path = PathBuf::from("\u{1F600}\u{1F600}\u{1F600}"); // 12 bytes
        let display = bytes_safe_display(&path, 7);
        assert_eq!(display, "\u{1F600}...");
        assert!(display.len() <= 7);
    }

    #[test]
    fn bytes_safe_display_respects_max_bytes_budget() {
        // Exhaustively check that output never exceeds max_bytes for a
        // long multi-byte path across a range of budgets.
        let path = PathBuf::from("\u{1F600}".repeat(50)); // 200 bytes
        for max_bytes in 0..=64 {
            let display = bytes_safe_display(&path, max_bytes);
            assert!(
                display.len() <= max_bytes,
                "result `{display}` exceeded budget {max_bytes}",
            );
        }
    }

    #[test]
    fn bytes_safe_display_short_path_unchanged() {
        let path = PathBuf::from("/bin/ls");
        let display = bytes_safe_display(&path, 100);
        assert_eq!(display, "/bin/ls");
    }

    #[test]
    fn bytes_safe_display_tiny_budget_no_panic() {
        // max_bytes <= 3 must return at most max_bytes characters of "..."
        // without panicking or slicing mid-char.
        let path = PathBuf::from("/very/long/path/for/logging");
        assert_eq!(bytes_safe_display(&path, 0), "");
        assert_eq!(bytes_safe_display(&path, 1), ".");
        assert_eq!(bytes_safe_display(&path, 2), "..");
        assert_eq!(bytes_safe_display(&path, 3), "...");
    }

    #[test]
    fn bytes_safe_display_budget_four_is_ellipsis_only() {
        // 4-byte budget reserves 3 for "...", leaving 1 byte. A 4-byte
        // emoji cannot fit, so output is just "...".
        let path = PathBuf::from("\u{1F600}\u{1F600}");
        let display = bytes_safe_display(&path, 4);
        assert_eq!(display, "...");
        assert!(display.len() <= 4);
    }

    #[test]
    fn check_size_passes_under_limit() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"small").unwrap();
        let meta = std::fs::metadata(tmp.path()).unwrap();
        assert!(check_size(&meta, MAX_EXECUTABLE_FILE_SIZE).is_ok());
    }

    #[test]
    fn check_regular_file_rejects_dir() {
        let dir = tempfile::tempdir().unwrap();
        let meta = std::fs::metadata(dir.path()).unwrap();
        assert!(matches!(
            check_regular_file(dir.path(), &meta),
            Err(AuthError::NotRegularFile { .. })
        ));
    }
}
