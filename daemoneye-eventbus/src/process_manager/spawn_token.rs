//! Per-spawn collector authentication tokens (R9).
//!
//! The agent mints a fresh 32-byte random token every time it spawns a collector, writes it to a
//! file only the account that created it can read, and passes the collector the **path**. The
//! value never appears in an argument vector, an environment variable, or a log line, because both
//! of those are visible in a process listing. The path is not secret; the file's permissions are
//! what protect the value.
//!
//! A token is valid for exactly one process lifetime. Issuing again for the same identity — which
//! is what a respawn does — replaces the stored value, so a token captured from a previous spawn
//! authenticates nothing once the agent restarts that collector. [`SpawnTokenStore::revoke`] does
//! the same when the agent reaps one.
//!
//! Verification lives in `daemoneye-lib` (`detection::catalog::verify_spawn_token`), not here:
//! this crate and that one are siblings, and `daemoneye-agent` is the composition point that
//! fetches the expected value from this store and hands it to the verifier.
//!
//! # Platform note (R9's owner-only property)
//!
//! R9 names a permission property — readable only by the account that created it — and states the
//! Unix expression of it. Each platform reaches that property the way that platform actually
//! offers it.
//!
//! On **Unix** the file is created with `create_new` and mode `0o400` in a single `open(2)`, so
//! there is no create-then-`chmod` window and no pre-existing file or symlink is followed. The
//! directory is created `0o700` under the socket directory.
//!
//! On **Windows** the property comes from *placement* rather than from setting a DACL. The token
//! directory is rooted at `%LOCALAPPDATA%` instead of the socket directory, and the file inherits
//! that location's ACL, which grants the owning user, `SYSTEM` and `Administrators`. That is the
//! same shape as the Unix arm, where `0o400` inside `0o700` grants the owner and `root` — in both
//! cases the account and the machine's superuser, and nobody else. There is no socket directory to
//! sit in on Windows regardless: the transport there is named pipes.
//!
//! Setting an explicit DACL was rejected rather than deferred. Every Win32 API that could
//! (`SetNamedSecurityInfoW`, `SetEntriesInAclW`) is an `unsafe fn` in the `windows` crate against
//! a workspace that sets `unsafe_code = "forbid"`, and the crates that wrap them safely are either
//! dormant since 2021 or pre-1.0 from an unvetted publisher — neither is a dependency this
//! codebase should take on a credential path.
//!
//! **The asymmetry that remains is verification, not protection.** The Unix arm *checks* an
//! existing directory and refuses to issue when other accounts can reach it, because a reachable
//! directory may already have been tampered with. Reading a DACL back needs the same API being
//! avoided, so the Windows arm sets the property but cannot confirm it. `check_private_dir` is
//! therefore a real gate on Unix and a no-op on Windows; the test names say which is which.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use thiserror::Error;

/// Command-line flag the agent uses to hand a collector its token file path.
pub const SPAWN_TOKEN_ARG: &str = "--spawn-token-file";

/// Raw token length in bytes before hex encoding (R9).
const SPAWN_TOKEN_BYTES: usize = 32;

/// Token length in ASCII characters after hex encoding.
const SPAWN_TOKEN_HEX_LEN: usize = SPAWN_TOKEN_BYTES * 2;

/// Filename suffix for a collector's token file.
const TOKEN_FILE_SUFFIX: &str = ".spawn-token";

/// Subdirectory the store creates under the socket directory to hold token files.
///
/// The store creates and owns this rather than writing straight into the socket directory: a
/// directory it created is a directory whose mode it knows, which is what makes the check in
/// [`check_private_dir`] meaningful instead of a formality over someone else's bits.
const TOKEN_DIR_NAME: &str = "spawn-tokens";

/// Why a token could not be issued or read.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SpawnTokenError {
    /// The collector identity would not produce a safe filename inside the token directory.
    #[error("collector id `{0}` is not usable as a token filename")]
    InvalidCollectorId(String),
    /// The token directory is reachable by accounts other than the one that owns it.
    #[error("token directory {path} is group- or world-accessible (mode {mode:#o})")]
    InsecureDirectory {
        /// The offending directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// The token directory path is a symlink, so its mode says nothing about where it leads.
    ///
    /// Checked rather than followed: a symlink to an attacker-owned `0o700` directory passes
    /// every mode check while the credentials land somewhere the attacker reads.
    #[error("token directory path {0} is a symlink, not a directory this store created")]
    SymlinkedDirectory(PathBuf),
    /// The token directory exists but belongs to another account.
    ///
    /// Mode bits alone do not establish the owner-only property R9 names: `0o700` owned by
    /// somebody else grants that somebody, not this process.
    #[error("token directory {path} is owned by uid {owner}, not this process's uid {expected}")]
    ForeignDirectory {
        /// The offending directory.
        path: PathBuf,
        /// The uid that owns it.
        owner: u32,
        /// This process's effective uid.
        expected: u32,
    },
    /// No private per-user directory could be resolved to root the token directory at.
    ///
    /// Only reachable off Unix, where the owner-only property comes from placing the directory
    /// under the account's own local application-data directory. Falling back to a shared location
    /// would silently drop the protection, so this fails closed instead.
    #[error("no private per-user directory is available to hold spawn tokens")]
    NoPrivateRoot,
    /// The token file did not hold a well-formed token.
    #[error("token file {0} does not contain a 64-character hex token")]
    Malformed(PathBuf),
    /// Filesystem failure.
    #[error("spawn token I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// A token that has just been issued for one spawn.
///
/// Carries the path, never the value: everything a caller needs to launch the collector, and
/// nothing it needs to authenticate as one.
#[derive(Debug, Clone)]
pub struct IssuedToken {
    path: PathBuf,
}

impl IssuedToken {
    /// Path to the file holding the token.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The two arguments to append to the collector's command line.
    #[must_use]
    pub fn command_args(&self) -> Vec<String> {
        vec![SPAWN_TOKEN_ARG.to_owned(), self.path.display().to_string()]
    }
}

/// The tokens currently valid, one per live collector spawn.
pub struct SpawnTokenStore {
    directory: PathBuf,
    /// Synchronous lock, never held across an `.await`.
    tokens: Mutex<HashMap<String, String>>,
}

/// Hand-written so the store's `Debug` cannot reproduce a live token.
///
/// `Mutex`'s derived `Debug` prints its contents whenever the lock is free, so a derive here made
/// `format!("{store:?}")` print every token the agent had issued — and `CollectorAdmission`, which
/// holds an `Arc<SpawnTokenStore>`, inherited that. The count is the useful part; the values are
/// the credential.
impl std::fmt::Debug for SpawnTokenStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let issued = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        formatter
            .debug_struct("SpawnTokenStore")
            .field("directory", &self.directory)
            .field("issued", &issued)
            .finish_non_exhaustive()
    }
}

impl SpawnTokenStore {
    /// Open a store under `socket_directory`, in an owner-only `spawn-tokens` subdirectory.
    ///
    /// An existing token directory is checked rather than corrected: one that other accounts can
    /// already reach may already have been tampered with, so this refuses instead of tightening the
    /// bits and carrying on. That is the directory-ownership half of the spawn-token question —
    /// the store owns its directory, it does not adopt a shared one.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnTokenError::InsecureDirectory`] when an existing token directory is group- or
    /// world-accessible, or [`SpawnTokenError::Io`] when it cannot be created or inspected.
    pub fn new(socket_directory: impl AsRef<Path>) -> Result<Self, SpawnTokenError> {
        let directory = token_directory(socket_directory.as_ref())?;
        create_private_dir(&directory)?;
        check_private_dir(&directory)?;
        Ok(Self {
            directory,
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// The directory this store writes token files into.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Mint a fresh token for `collector_id`, replacing any token issued for a previous spawn.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnTokenError::InvalidCollectorId`] for an identity that would not stay inside
    /// the token directory, or [`SpawnTokenError::Io`] on a filesystem failure.
    pub fn issue(&self, collector_id: &str) -> Result<IssuedToken, SpawnTokenError> {
        let path = self.token_path(collector_id)?;
        let token = random_token();

        // A previous spawn's file is removed first so the create below can stay exclusive.
        remove_token_file(&path)?;
        write_owner_only(&path, &token)?;

        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _previous = tokens.insert(collector_id.to_owned(), token);
        drop(tokens);

        Ok(IssuedToken { path })
    }

    /// The token currently valid for `collector_id`, or `None` if none was ever issued.
    ///
    /// This is the value `daemoneye-lib`'s constant-time verifier compares against.
    #[must_use]
    pub fn expected_token(&self, collector_id: &str) -> Option<String> {
        let tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokens.get(collector_id).cloned()
    }

    /// Invalidate a collector's token and delete its file, as the agent reaps it.
    pub fn revoke(&self, collector_id: &str) {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _removed = tokens.remove(collector_id);
        drop(tokens);

        if let Ok(path) = self.token_path(collector_id)
            && let Err(error) = remove_token_file(&path)
        {
            tracing::warn!(error = %error, "Failed to remove spawn token file");
        }
    }

    /// Path for `collector_id`'s token file, refusing an identity that would escape the directory.
    fn token_path(&self, collector_id: &str) -> Result<PathBuf, SpawnTokenError> {
        let usable = !collector_id.is_empty()
            && collector_id.len() <= 64
            && collector_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            });
        if !usable {
            return Err(SpawnTokenError::InvalidCollectorId(collector_id.to_owned()));
        }
        Ok(self
            .directory
            .join(format!("{collector_id}{TOKEN_FILE_SUFFIX}")))
    }
}

/// Read a token a collector was handed by path.
///
/// Collectors call this; it validates shape at the boundary so a truncated or padded file is a
/// named error rather than a registration that mysteriously fails to authenticate.
///
/// # Errors
///
/// Returns [`SpawnTokenError::Malformed`] when the file does not hold 64 lowercase hex characters,
/// or [`SpawnTokenError::Io`] when it cannot be read.
pub fn read_token_file(path: &Path) -> Result<String, SpawnTokenError> {
    let raw = fs::read_to_string(path)?;
    let token = raw.trim().to_owned();
    let well_formed = token.len() == SPAWN_TOKEN_HEX_LEN
        && token
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase());
    if well_formed {
        return Ok(token);
    }
    Err(SpawnTokenError::Malformed(path.to_owned()))
}

/// 32 random bytes as lowercase hex.
///
/// `rand::random` draws from the thread RNG, a ChaCha-family CSPRNG seeded from the operating
/// system's entropy source and periodically reseeded from it. The hex encoding is written by hand
/// rather than pulled in from a crate: two nibble lookups are smaller than a dependency.
fn random_token() -> String {
    /// Lowercase hex alphabet.
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let bytes: [u8; SPAWN_TOKEN_BYTES] = rand::random();
    let mut hex = String::with_capacity(SPAWN_TOKEN_HEX_LEN);
    for byte in bytes {
        let high = HEX.get(usize::from(byte >> 4_u32)).copied().unwrap_or(b'0');
        let low = HEX
            .get(usize::from(byte & 0x0f_u8))
            .copied()
            .unwrap_or(b'0');
        hex.push(char::from(high));
        hex.push(char::from(low));
    }
    hex
}

/// Delete a token file if it exists.
///
/// On Unix the file's own mode is left alone: unlinking is governed by the directory's
/// permissions, and `set_readonly(false)` would grant write to group and other as well, briefly
/// widening a `0o400` token file before it is removed. Windows refuses to delete a read-only file,
/// so there the attribute has to come off first.
fn remove_token_file(path: &Path) -> Result<(), SpawnTokenError> {
    #[cfg(not(unix))]
    {
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            if permissions.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                fs::set_permissions(path, permissions)?;
            }
        }
    }

    match fs::remove_file(path) {
        Err(ref error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(()) => Ok(()),
    }
}

/// Where the token directory lives: beside the socket, which is owner-only by its own mode.
///
/// Infallible on Unix, but it returns `Result` so both `cfg` arms present one signature to the
/// single call site; off Unix resolving a private per-user root genuinely can fail.
#[allow(clippy::unnecessary_wraps)]
#[cfg(unix)]
fn token_directory(socket_directory: &Path) -> Result<PathBuf, SpawnTokenError> {
    Ok(socket_directory.join(TOKEN_DIR_NAME))
}

/// Where the token directory lives off Unix: under the account's own local application-data
/// directory, whose ACL already grants that account, `SYSTEM` and `Administrators` and nobody
/// else. `socket_directory` is deliberately unused — rooting the tokens there would inherit
/// whatever ACL a shared path carries, which is the protection this arm exists to provide. See the
/// module's platform note.
#[cfg(not(unix))]
fn token_directory(_socket_directory: &Path) -> Result<PathBuf, SpawnTokenError> {
    let base = dirs::data_local_dir().ok_or(SpawnTokenError::NoPrivateRoot)?;
    Ok(base.join("DaemonEye").join(TOKEN_DIR_NAME))
}

/// Create `directory` owner-only if it is absent, with the mode set at creation.
#[cfg(unix)]
fn create_private_dir(directory: &Path) -> Result<(), SpawnTokenError> {
    use std::os::unix::fs::DirBuilderExt as _;

    if directory.exists() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    Ok(())
}

/// Create `directory` if it is absent. Off Unix its ACL is inherited from the private per-user
/// root [`token_directory`] chose; see the module's platform note.
#[cfg(not(unix))]
fn create_private_dir(directory: &Path) -> Result<(), SpawnTokenError> {
    if directory.exists() {
        return Ok(());
    }
    fs::create_dir_all(directory)?;
    Ok(())
}

/// Refuse a token directory other accounts can reach.
///
/// Three properties, in the order an attacker would try them:
///
/// 1. **Not a symlink.** Read with `symlink_metadata`, so the entry itself is inspected rather
///    than whatever it points at. `fs::metadata` follows the link, and a symlink to an
///    attacker-owned `0o700` directory then passes both remaining checks.
/// 2. **Owned by this process.** `0o700` says only that one account may reach it, never which
///    account. The effective uid comes from `nix::unistd::geteuid`, a safe wrapper: the workspace
///    forbids the `unsafe` a direct syscall would take.
/// 3. **Owner-only mode.** The original check, unchanged.
#[cfg(unix)]
fn check_private_dir(directory: &Path) -> Result<(), SpawnTokenError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let metadata = fs::symlink_metadata(directory)?;
    if metadata.file_type().is_symlink() {
        return Err(SpawnTokenError::SymlinkedDirectory(directory.to_owned()));
    }

    let expected = nix::unistd::geteuid().as_raw();
    let owner = metadata.uid();
    if owner != expected {
        return Err(SpawnTokenError::ForeignDirectory {
            path: directory.to_owned(),
            owner,
            expected,
        });
    }

    let mode = metadata.permissions().mode() & 0o777;
    // Owner-only means the low six bits — group and other — are all clear.
    if mode.trailing_zeros() >= 6 {
        return Ok(());
    }
    Err(SpawnTokenError::InsecureDirectory {
        path: directory.to_owned(),
        mode,
    })
}

/// A no-op off Unix: the property is established by placement, and reading an ACL back to confirm
/// it needs the very API the module note explains is unavailable. This exists so the call site
/// reads the same on both platforms, not because it verifies anything here.
#[cfg(not(unix))]
fn check_private_dir(directory: &Path) -> Result<(), SpawnTokenError> {
    let _metadata = fs::metadata(directory)?;
    Ok(())
}

/// Create the token file and write the value in one exclusive `open`, with no `chmod` window.
#[cfg(unix)]
fn write_owner_only(path: &Path, token: &str) -> Result<(), SpawnTokenError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Create the token file exclusively and mark it read-only. See the module note on the DACL gap.
#[cfg(not(unix))]
fn write_owner_only(path: &Path, token: &str) -> Result<(), SpawnTokenError> {
    use std::io::Write as _;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Find the token file path in an argument vector and read the token it holds.
///
/// Collectors call this at startup with their own `argv`. It returns `None` when the agent did not
/// hand this process a token — an unmanaged or manually launched collector — and logs, rather than
/// fails, when the path is present but unreadable: the registration will then be refused by the
/// agent's gate, which is where that decision belongs.
#[must_use]
pub fn spawn_token_from_args<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iterator = args.into_iter();
    while let Some(raw) = iterator.next() {
        let argument = raw.as_ref();
        // Both `--spawn-token-file <path>` and `--spawn-token-file=<path>` are accepted; the
        // second form is split with `split_once` rather than sliced, per the `string_slice` ban.
        let candidate = if argument == SPAWN_TOKEN_ARG {
            iterator.next().map(|next| next.as_ref().to_owned())
        } else {
            argument
                .split_once('=')
                .filter(|entry| entry.0 == SPAWN_TOKEN_ARG)
                .map(|entry| entry.1.to_owned())
        };
        let Some(token_path) = candidate else {
            continue;
        };
        return match read_token_file(Path::new(&token_path)) {
            Ok(token) => Some(token),
            Err(error) => {
                tracing::warn!(error = %error, "Failed to read the spawn token file");
                None
            }
        };
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn both_argument_forms_resolve_to_the_same_token() {
        // Arrange
        let dir = tempfile::tempdir().unwrap();
        let store = SpawnTokenStore::new(dir.path()).unwrap();
        let issued = store.issue("procmond").unwrap();
        let path = issued.path().display().to_string();
        let expected = store.expected_token("procmond").unwrap();

        // Act
        let separate = spawn_token_from_args(["procmond", SPAWN_TOKEN_ARG, &path]);
        let joined = spawn_token_from_args(["procmond", &format!("{SPAWN_TOKEN_ARG}={path}")]);

        // Assert
        assert_eq!(separate.as_deref(), Some(expected.as_str()));
        assert_eq!(joined.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn an_argument_vector_without_the_flag_yields_no_token() {
        assert!(spawn_token_from_args(["procmond", "--verbose"]).is_none());
    }

    #[test]
    fn an_unreadable_path_yields_no_token_rather_than_a_panic() {
        assert!(spawn_token_from_args([SPAWN_TOKEN_ARG, "/nonexistent/token"]).is_none());
    }

    /// A symlinked token directory is refused rather than followed.
    ///
    /// The target is `0o700` and owned by this account, so it passes both the mode and the
    /// ownership check; only inspecting the entry itself catches it. Off Unix `check_private_dir`
    /// is a no-op by design (see the module's platform note), so the test is Unix-only.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_token_directory_is_refused() {
        // Arrange
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(
            &target,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(TOKEN_DIR_NAME)).unwrap();

        // Act
        let opened = SpawnTokenStore::new(dir.path());

        // Assert
        assert!(
            matches!(opened, Err(SpawnTokenError::SymlinkedDirectory(_))),
            "a symlinked token directory must be refused, not followed"
        );
    }

    #[test]
    fn the_stores_debug_rendering_holds_no_token() {
        // Arrange
        let dir = tempfile::tempdir().unwrap();
        let store = SpawnTokenStore::new(dir.path()).unwrap();
        let _issued = store.issue("procmond").unwrap();
        let token = store.expected_token("procmond").unwrap();

        // Act
        let rendered = format!("{store:?}");

        // Assert: the message carries no secret material (CodeQL rust/cleartext-logging).
        assert!(
            !rendered.contains(&token),
            "the store's Debug rendering reproduced a live token"
        );
        assert!(rendered.contains("issued: 1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_this_account_owns_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SpawnTokenStore::new(dir.path()).is_ok());
    }
}
