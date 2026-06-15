//! Schema-version classification and the signed export → rebuild state machine
//! (T3 · U8).
//!
//! The event store stabilizes its on-disk format with a single `schema_version`
//! cell. On open the engine classifies that cell three ways
//! ([`SchemaClassification`]): **absent** (first-init), **equal** (proceed), or
//! **different** (rebuild). The rebuild path is deliberately *export-then-rebuild,
//! never migrate-in-place* — a binary that bumped `schema_version` may no longer
//! be able to decode the old value payloads, so the only safe move is to archive
//! the old store verbatim and reinitialize fresh (R14–R19).
//!
//! ## Why a whole-file archive, not per-partition raw rows
//!
//! redb 4.1.0's untyped-table surface (`open_untyped_table` →
//! `ReadOnlyUntypedTable`) exposes only [`redb::ReadableTableMetadata`] (length),
//! **not** raw key/value iteration — there is no public way to read an arbitrary
//! partition's bytes without knowing its type. Since the whole point of the
//! rebuild is that the new binary may *not* understand the old types, the export
//! captures the entire redb database file plus a partition manifest derived from
//! `list_tables()`. That preserves undecodable old data losslessly and reinforces
//! the plan's own export-then-rebuild decision.
//!
//! ## Crypto-free by construction
//!
//! This module defines the [`BundleSigner`] seam but takes **no** cryptographic
//! or IPC dependency. The agent injects an `ed25519-dalek`-backed signer and
//! orchestrates the call (R19); `daemoneye-lib` stays crypto-free.
//!
//! ## Resumability
//!
//! Every destructive step is gated behind a durable, verified, signed archive and
//! is driven by a **sidecar progress marker** that lives *outside* the store being
//! rebuilt (reinit destroys user tables, so a marker in a user table could not
//! survive tracking its own destruction). A crash mid-rebuild resumes
//! deterministically from the marker phase rather than re-running a destructive
//! step or boot-looping.

use crate::storage::error::StorageError;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, TableHandle};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::codec::TsSeqKey;

/// On-disk schema version understood by this binary. Bump only when the stored
/// value format changes incompatibly — the rebuild path handles the transition.
pub const SCHEMA_VERSION: u32 = 1;

/// Bundle envelope format version (the manifest framing), independent of
/// [`SCHEMA_VERSION`] so the archive format can evolve on its own axis.
pub const BUNDLE_FORMAT_VERSION: u32 = 1;

/// Maximum consecutive rebuild failures before the store enters the read-only
/// degraded terminal state (R17). A deterministic failure (e.g. a bad signer)
/// trips this rather than boot-looping forever.
pub const MAX_REBUILD_FAILURES: u32 = 3;

/// redb cell holding the single schema-version value (key is the literal
/// `"version"`). A plain typed table, not a bucket.
const SCHEMA_VERSION_TABLE: TableDefinition<'static, &str, u32> =
    TableDefinition::new("schema_version");

/// The key under which the schema version is stored in [`SCHEMA_VERSION_TABLE`].
const VERSION_KEY: &str = "version";

/// Bucket key codec used to read the oldest event timestamp when computing the
/// gap record. The key codec is schema-stable across versions (only value
/// payloads evolve), so the oldest `ts_ms` is readable even from an old store.
type BucketKeyTable<'a> = TableDefinition<'a, TsSeqKey, &'static [u8]>;

/// Three-way classification of the on-disk `schema_version` cell (R14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaClassification {
    /// No `schema_version` recorded — a brand-new store. Write the current
    /// version; no export, no gap record.
    FirstInit,
    /// Recorded version equals the running binary — proceed normally.
    Match,
    /// Recorded version differs — the export → rebuild path must run.
    Mismatch {
        /// Version recorded in the store.
        found: u32,
        /// Version the running binary understands.
        expected: u32,
    },
}

/// Pluggable signing seam for the export bundle.
///
/// The storage crate defines it but supplies no implementation; the agent injects
/// an `ed25519-dalek`-backed signer (R19). Sign opaque bytes; verify a detached
/// signature.
pub trait BundleSigner: Send + Sync {
    /// Produce a detached signature over `bytes`.
    fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, SignerError>;

    /// Verify a detached `signature` over `bytes`; `Ok(())` iff it validates.
    fn verify(&self, bytes: &[u8], signature: &[u8]) -> Result<(), SignerError>;
}

/// Opaque signing/verification failure surfaced by a [`BundleSigner`].
#[derive(Debug, thiserror::Error)]
#[error("bundle signer error: {0}")]
pub struct SignerError(pub String);

/// One partition (redb table) recorded in the bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionInfo {
    /// Table name as reported by `list_tables()`.
    pub name: String,
    /// Reserved forensic field, currently `0`. The completeness gate matches on
    /// partition *names*, not counts, so this is advisory only.
    pub entries: u64,
}

/// Stable, versioned bundle manifest header (R18). Round-trips via postcard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Envelope format version ([`BUNDLE_FORMAT_VERSION`]).
    pub format_version: u32,
    /// The *old* schema version being archived.
    pub schema_version: u32,
    /// Wall-clock archive time, injected (ms since epoch).
    pub written_at_ms: u64,
    /// Every partition discovered via `list_tables()` at archive time.
    pub partitions: Vec<PartitionInfo>,
}

/// The unrecoverable observation window created by a rebuild (R17). The agent
/// persists this and emits a high-severity audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapRecord {
    /// Inclusive start of the gap (ms since epoch).
    pub start_ms: u64,
    /// Exclusive end of the gap (ms since epoch).
    pub end_ms: u64,
}

/// Result of a completed rebuild, returned to the agent orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Schema version archived.
    pub from_version: u32,
    /// Schema version the store now runs at ([`SCHEMA_VERSION`]).
    pub to_version: u32,
    /// Path to the signed archive bundle.
    pub bundle_path: PathBuf,
    /// Unrecoverable observation gap (R17).
    pub gap: GapRecord,
}

/// Sidecar progress phases — strictly monotonic.
///
/// A resumed run reads the phase to know which destructive steps already
/// completed. Failure is tracked separately by `ProgressMarker::failure_count`,
/// never by regressing the phase: regressing to an "aborted" phase after the
/// destructive reinit would let a resume re-run the archive-overwriting export
/// against the already-emptied store (data loss).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MigrationPhase {
    /// Building/signing the archive bundle; the old store is untouched.
    Exporting,
    /// Bundle is durable, signed, and verified against on-disk bytes. Past this
    /// point the live store may be destroyed, so the marker — not the live store —
    /// is the authoritative source of `from_version`/gap on resume.
    Verified,
    /// Old tables are being dropped and the store reinitialized (idempotent).
    Dropping,
    /// Rebuild finished; the store runs at the new version.
    Complete,
}

/// Whether `phase` is at or past [`MigrationPhase::Verified`] — i.e. a durable,
/// verified archive exists and the live store may already be gone.
const fn at_least_verified(phase: MigrationPhase) -> bool {
    matches!(
        phase,
        MigrationPhase::Verified | MigrationPhase::Dropping | MigrationPhase::Complete
    )
}

/// Sidecar marker persisted next to (not inside) the store. fsync'd on every
/// write so a crash resumes from the last durable phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressMarker {
    /// Last durable phase.
    pub phase: MigrationPhase,
    /// Old schema version being archived.
    pub from_version: u32,
    /// Target schema version.
    pub to_version: u32,
    /// Absolute path of the archive bundle.
    pub bundle_path: PathBuf,
    /// Computed gap start (oldest event ts, or `now_ms` for an empty store).
    /// Preserved here so a resume past [`MigrationPhase::Verified`] reports the
    /// original gap rather than recomputing a bogus one from the emptied store.
    pub gap_start_ms: u64,
    /// Gap end (the `now_ms` of the run that began the rebuild). Preserved for the
    /// same reason as `gap_start_ms`.
    pub gap_end_ms: u64,
    /// Consecutive failures so far; once it reaches [`MAX_REBUILD_FAILURES`] the
    /// store is degraded read-only.
    pub failure_count: u32,
}

/// Classify the store's `schema_version` cell against [`SCHEMA_VERSION`] (R14).
pub fn classify(db: &Database) -> Result<SchemaClassification, StorageError> {
    let rtxn = db.begin_read()?;
    let table = match rtxn.open_table(SCHEMA_VERSION_TABLE) {
        Ok(table) => table,
        // A store written before the version cell existed reads as first-init.
        Err(redb::TableError::TableDoesNotExist(_)) => {
            return Ok(SchemaClassification::FirstInit);
        }
        Err(err) => return Err(err.into()),
    };
    let Some(cell) = table.get(VERSION_KEY)? else {
        return Ok(SchemaClassification::FirstInit);
    };
    let found = cell.value();
    if found == SCHEMA_VERSION {
        Ok(SchemaClassification::Match)
    } else {
        Ok(SchemaClassification::Mismatch {
            found,
            expected: SCHEMA_VERSION,
        })
    }
}

/// Write the current [`SCHEMA_VERSION`] into the store (first-init or post-reinit).
pub fn write_current_version(db: &Database) -> Result<(), StorageError> {
    let wtxn = db.begin_write()?;
    let mut table = wtxn.open_table(SCHEMA_VERSION_TABLE)?;
    table.insert(VERSION_KEY, SCHEMA_VERSION)?;
    drop(table);
    wtxn.commit()?;
    Ok(())
}

/// Default archive bundle path beside the database file.
fn default_bundle_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".rebuild-bundle");
    db_path.with_file_name(name)
}

/// Sidecar marker path beside the database file.
fn marker_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".migration");
    db_path.with_file_name(name)
}

/// Detached-signature path beside the bundle file.
fn signature_path(bundle_path: &Path) -> PathBuf {
    let mut name = bundle_path.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    bundle_path.with_file_name(name)
}

/// Read the sidecar marker, if one exists.
fn read_marker(db_path: &Path) -> Result<Option<ProgressMarker>, StorageError> {
    let path = marker_path(db_path);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(postcard::from_bytes(&bytes)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StorageError::IoError { path, source }),
    }
}

/// Write the sidecar marker durably (write + fsync).
fn write_marker(db_path: &Path, marker: &ProgressMarker) -> Result<(), StorageError> {
    let path = marker_path(db_path);
    let bytes = postcard::to_allocvec(marker)?;
    let mut file = fs::File::create(&path).map_err(|source| StorageError::IoError {
        path: path.clone(),
        source,
    })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| StorageError::IoError { path, source })
}

/// Remove the sidecar marker (best-effort cleanup after a clean rebuild).
fn remove_marker(db_path: &Path) -> Result<(), StorageError> {
    let path = marker_path(db_path);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StorageError::IoError { path, source }),
    }
}

/// Inspect the old store: read its schema version, enumerate partitions for the
/// manifest, and compute the gap start (oldest event `ts_ms`, schema-stable key
/// codec). The handle is dropped before any file-level archive step.
fn inspect_store(db_path: &Path) -> Result<(u32, Vec<PartitionInfo>, u64), StorageError> {
    let db = Database::open(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
        path: db_path.to_path_buf(),
        source,
    })?;
    let from_version = match classify(&db)? {
        SchemaClassification::Mismatch { found, .. } => found,
        // inspect_store is only called on the mismatch path; treat first-init/match
        // defensively as the current version (no harm — the manifest records it).
        SchemaClassification::FirstInit | SchemaClassification::Match => SCHEMA_VERSION,
    };
    let rtxn = db.begin_read()?;
    let mut partitions = Vec::new();
    let mut bucket_names = Vec::new();
    for handle in rtxn.list_tables()? {
        let name = handle.name().to_owned();
        partitions.push(PartitionInfo {
            name: name.clone(),
            entries: 0,
        });
        if super::bucket::parse_bucket_name(&name).is_some() {
            bucket_names.push(name);
        }
    }
    let gap_start = oldest_event_ms(&rtxn, &bucket_names)?;
    Ok((from_version, partitions, gap_start.unwrap_or(u64::MAX)))
}

/// The minimum event `ts_ms` across the named bucket tables, or `None` when the
/// store holds no events. Reads only keys (16-byte codec), never value payloads,
/// so it is safe against an unknown value schema.
fn oldest_event_ms(
    rtxn: &redb::ReadTransaction,
    bucket_names: &[String],
) -> Result<Option<u64>, StorageError> {
    let mut oldest: Option<u64> = None;
    for name in bucket_names {
        let def: BucketKeyTable<'_> = TableDefinition::new(name);
        let table = rtxn.open_table(def)?;
        if let Some(entry) = table.iter()?.next() {
            let ((ts_ms, _seq), _value) = {
                let (k, v) = entry?;
                (k.value(), v)
            };
            oldest = Some(oldest.map_or(ts_ms, |cur| cur.min(ts_ms)));
        }
    }
    Ok(oldest)
}

/// Build the signed archive bundle: `[manifest_len LE u64][manifest][db bytes]`
/// plus a detached signature sidecar. The bundle file is created `O_CREAT|O_EXCL`
/// at mode 0600 *before* any bytes are written (Unix; Windows hardening deferred).
fn export_bundle(
    db_path: &Path,
    bundle_path: &Path,
    manifest: &BundleManifest,
    signer: &dyn BundleSigner,
) -> Result<(), StorageError> {
    // A resumed export may find a partial bundle; start clean so EXCL succeeds.
    remove_if_exists(bundle_path)?;
    remove_if_exists(&signature_path(bundle_path))?;

    let manifest_bytes = postcard::to_allocvec(manifest)?;
    let db_bytes = fs::read(db_path).map_err(|source| StorageError::IoError {
        path: db_path.to_path_buf(),
        source,
    })?;

    let mut bundle = Vec::with_capacity(
        8_usize
            .saturating_add(manifest_bytes.len())
            .saturating_add(db_bytes.len()),
    );
    let manifest_len =
        u64::try_from(manifest_bytes.len()).map_err(|_err| StorageError::Overflow {
            context: "bundle manifest length".to_owned(),
        })?;
    bundle.extend_from_slice(&manifest_len.to_le_bytes());
    bundle.extend_from_slice(&manifest_bytes);
    bundle.extend_from_slice(&db_bytes);

    let mut file = create_exclusive_0600(bundle_path)?;
    file.write_all(&bundle)
        .and_then(|()| file.sync_all())
        .map_err(|source| StorageError::IoError {
            path: bundle_path.to_path_buf(),
            source,
        })?;
    drop(file);

    let signature = signer.sign(&bundle).map_err(|err| StorageError::Bucket {
        bucket: "schema-rebuild".to_owned(),
        message: format!("export signing failed: {err}"),
    })?;
    let sig_path = signature_path(bundle_path);
    let mut sig_file = fs::File::create(&sig_path).map_err(|source| StorageError::IoError {
        path: sig_path.clone(),
        source,
    })?;
    sig_file
        .write_all(&signature)
        .and_then(|()| sig_file.sync_all())
        .map_err(|source| StorageError::IoError {
            path: sig_path,
            source,
        })?;
    Ok(())
}

/// The drop gate (R15): re-read the bundle and signature from disk (never the
/// in-memory buffer, so read-back corruption is caught), verify the signature,
/// and prove partition completeness by re-deriving the archived store's table set
/// and asserting it equals the manifest's. Additionally assert the bundle's
/// embedded manifest equals `expected`. Any failure aborts before the drop.
fn verify_bundle(
    bundle_path: &Path,
    signer: &dyn BundleSigner,
    expected: &BundleManifest,
) -> Result<(), StorageError> {
    let embedded = verify_bundle_self(bundle_path, signer)?;
    if &embedded != expected {
        return Err(StorageError::Bucket {
            bucket: "schema-rebuild".to_owned(),
            message: "bundle manifest does not match the expected manifest".to_owned(),
        });
    }
    Ok(())
}

/// Self-consistent bundle verification: read the bundle + signature from disk,
/// verify the signature, and prove the embedded manifest's partition set matches
/// what the archived db actually holds. Returns the embedded manifest. Used both
/// at export time (via [`verify_bundle`]) and — critically — again before a
/// *resumed* drop, so a bundle that was deleted or corrupted between runs aborts
/// the destruction instead of emptying the store with no archive (R15).
fn verify_bundle_self(
    bundle_path: &Path,
    signer: &dyn BundleSigner,
) -> Result<BundleManifest, StorageError> {
    let bundle = fs::read(bundle_path).map_err(|source| StorageError::IoError {
        path: bundle_path.to_path_buf(),
        source,
    })?;
    let sig_path = signature_path(bundle_path);
    let signature = fs::read(&sig_path).map_err(|source| StorageError::IoError {
        path: sig_path,
        source,
    })?;
    signer
        .verify(&bundle, &signature)
        .map_err(|err| StorageError::Bucket {
            bucket: "schema-rebuild".to_owned(),
            message: format!("bundle signature verification failed: {err}"),
        })?;

    let (embedded_manifest, db_bytes) = split_bundle(&bundle)?;
    let archived: Vec<String> = enumerate_archived_partitions(db_bytes, bundle_path)?;
    let mut expected: Vec<String> = embedded_manifest
        .partitions
        .iter()
        .map(|partition| partition.name.clone())
        .collect();
    let mut archived_sorted = archived;
    expected.sort();
    archived_sorted.sort();
    if expected != archived_sorted {
        return Err(StorageError::Bucket {
            bucket: "schema-rebuild".to_owned(),
            message: format!(
                "incomplete archive: manifest lists {} partitions, bundle holds {}",
                expected.len(),
                archived_sorted.len()
            ),
        });
    }
    Ok(embedded_manifest)
}

/// Split a bundle buffer back into its manifest and embedded db bytes.
fn split_bundle(bundle: &[u8]) -> Result<(BundleManifest, &[u8]), StorageError> {
    let len_bytes = bundle.get(..8).ok_or_else(|| StorageError::Bucket {
        bucket: "schema-rebuild".to_owned(),
        message: "bundle truncated: missing manifest length".to_owned(),
    })?;
    let mut len_arr = [0_u8; 8];
    len_arr.copy_from_slice(len_bytes);
    let manifest_len =
        usize::try_from(u64::from_le_bytes(len_arr)).map_err(|_err| StorageError::Overflow {
            context: "bundle manifest length".to_owned(),
        })?;
    let manifest_end = 8_usize
        .checked_add(manifest_len)
        .ok_or_else(|| StorageError::Overflow {
            context: "bundle manifest end".to_owned(),
        })?;
    let manifest_bytes = bundle
        .get(8..manifest_end)
        .ok_or_else(|| StorageError::Bucket {
            bucket: "schema-rebuild".to_owned(),
            message: "bundle truncated: manifest body".to_owned(),
        })?;
    let db_bytes = bundle
        .get(manifest_end..)
        .ok_or_else(|| StorageError::Bucket {
            bucket: "schema-rebuild".to_owned(),
            message: "bundle truncated: db body".to_owned(),
        })?;
    let manifest: BundleManifest = postcard::from_bytes(manifest_bytes)?;
    Ok((manifest, db_bytes))
}

/// Re-open the embedded db bytes as a temporary redb database and list its
/// tables — the ground truth for the completeness gate. The scratch file lives
/// beside the bundle (the access-restricted agent dir, not the world-readable
/// global temp dir) with a process-unique suffix so concurrent verifies never
/// collide.
fn enumerate_archived_partitions(
    db_bytes: &[u8],
    bundle_path: &Path,
) -> Result<Vec<String>, StorageError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static VERIFY_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = VERIFY_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = bundle_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".verify-{}-{seq}", std::process::id()));
    let temp = bundle_path.with_file_name(name);
    fs::write(&temp, db_bytes).map_err(|source| StorageError::IoError {
        path: temp.clone(),
        source,
    })?;
    let result = (|| {
        let db = Database::open(&temp).map_err(|source| StorageError::DatabaseCreationFailed {
            path: temp.clone(),
            source,
        })?;
        let rtxn = db.begin_read()?;
        let names: Vec<String> = rtxn
            .list_tables()?
            .map(|handle| handle.name().to_owned())
            .collect();
        Ok::<_, StorageError>(names)
    })();
    let _ignored = fs::remove_file(&temp);
    result
}

/// Reinitialize the store at the new version: delete the old database file and
/// create a fresh one carrying [`SCHEMA_VERSION`]. Idempotent — safe to re-run if
/// a crash interrupted a prior reinit (the archived bundle holds the old data).
fn reinit_store(db_path: &Path) -> Result<(), StorageError> {
    remove_if_exists(db_path)?;
    let db = Database::create(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
        path: db_path.to_path_buf(),
        source,
    })?;
    write_current_version(&db)?;
    Ok(())
}

/// Remove a file if it exists, mapping I/O failures to a typed error.
fn remove_if_exists(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StorageError::IoError {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Create a file `O_CREAT|O_EXCL` at owner-only (0600 on Unix) permissions.
#[cfg(unix)]
fn create_exclusive_0600(path: &Path) -> Result<fs::File, StorageError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| StorageError::IoError {
            path: path.to_path_buf(),
            source,
        })
}

/// Create a file `O_CREAT|O_EXCL`. Windows owner-only ACL hardening is deferred
/// (the agent state dir is already access-restricted); the EXCL invariant holds.
#[cfg(not(unix))]
fn create_exclusive_0600(path: &Path) -> Result<fs::File, StorageError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| StorageError::IoError {
            path: path.to_path_buf(),
            source,
        })
}

/// Run (or resume) the export → rebuild state machine for a store whose
/// `schema_version` mismatched (R14–R19).
///
/// Sequence: take/read the sidecar marker (degraded gate first), inspect the old
/// store, then `Exporting → Verified → Dropping → Complete`. The destructive
/// reinit runs only after a durable, signature-verified, partition-complete
/// archive exists. On failure the old store is retained and the failure count
/// climbs; at [`MAX_REBUILD_FAILURES`] the call returns
/// [`StorageError::DegradedReadOnly`].
///
/// WAL replay (R16) is the agent's concern (the WAL belongs to procmond); this
/// returns the maximal [`GapRecord`] for the agent to narrow after replay.
pub fn migrate(
    db_path: &Path,
    signer: &dyn BundleSigner,
    now_ms: u64,
) -> Result<MigrationReport, StorageError> {
    let existing = read_marker(db_path)?;
    if let Some(marker) = existing.as_ref()
        && marker.failure_count >= MAX_REBUILD_FAILURES
    {
        return Err(StorageError::DegradedReadOnly {
            reason: format!(
                "schema rebuild failed {} times; manual intervention required",
                marker.failure_count
            ),
        });
    }

    let prior_failures = existing.as_ref().map_or(0, |marker| marker.failure_count);
    let resume_phase = existing
        .as_ref()
        .map_or(MigrationPhase::Exporting, |marker| marker.phase);

    // Seed the working marker. Past `Verified` the live store may already be gone,
    // so the *marker* — not a fresh `inspect_store` — is authoritative for
    // `from_version`/gap; before `Verified` the live store is intact and
    // `run_rebuild` (re)derives those from it. Placeholders here are overwritten
    // by the export branch before they are ever read.
    let mut marker = existing.unwrap_or_else(|| ProgressMarker {
        phase: MigrationPhase::Exporting,
        from_version: SCHEMA_VERSION,
        to_version: SCHEMA_VERSION,
        bundle_path: default_bundle_path(db_path),
        gap_start_ms: now_ms,
        gap_end_ms: now_ms,
        failure_count: prior_failures,
    });

    // The whole attempt — including `inspect_store` — is inside the failure-count
    // envelope, so a store that cannot even be inspected still converges to the
    // degraded terminal state instead of boot-looping forever.
    match run_rebuild(db_path, signer, now_ms, resume_phase, &mut marker) {
        Ok(()) => {
            // Best-effort cleanup: a stale `Complete` marker is self-healing (the
            // next open reports from it and touches nothing), so a removal failure
            // must not turn a genuinely-successful rebuild into an error.
            let _ignored = remove_marker(db_path);
            Ok(report_from_marker(&marker))
        }
        Err(err) => {
            // Preserve whatever phase `run_rebuild` last reached (never regress it)
            // and bump the failure count; the phase tells a resume what is safe to
            // re-run, the count drives the degraded gate. The marker write is
            // best-effort so a sidecar I/O failure cannot mask the real rebuild
            // error (and the destructive phase transitions are already persisted
            // inside `run_rebuild`, so the store is never left half-destroyed here).
            marker.failure_count = prior_failures.saturating_add(1);
            let _ignored = write_marker(db_path, &marker);
            if marker.failure_count >= MAX_REBUILD_FAILURES {
                return Err(StorageError::DegradedReadOnly {
                    reason: format!("schema rebuild failed: {err}"),
                });
            }
            Err(err)
        }
    }
}

/// Reconstruct the [`MigrationReport`] from the (authoritative) marker.
fn report_from_marker(marker: &ProgressMarker) -> MigrationReport {
    MigrationReport {
        from_version: marker.from_version,
        to_version: marker.to_version,
        bundle_path: marker.bundle_path.clone(),
        gap: GapRecord {
            start_ms: marker.gap_start_ms,
            end_ms: marker.gap_end_ms,
        },
    }
}

/// Drive the phases, skipping any the marker says already completed. Each
/// destructive step is individually guarded so a resume re-runs only what is
/// safe and never the archive-overwriting export once a verified bundle exists.
fn run_rebuild(
    db_path: &Path,
    signer: &dyn BundleSigner,
    now_ms: u64,
    resume_phase: MigrationPhase,
    marker: &mut ProgressMarker,
) -> Result<(), StorageError> {
    // Export + verify, unless a prior run already produced a verified bundle.
    // This branch is the *only* place the live store is read, and it only runs
    // while the live store is guaranteed intact (we never reinit before Verified).
    if !at_least_verified(resume_phase) {
        let (from_version, partitions, gap_start_raw) = inspect_store(db_path)?;
        marker.from_version = from_version;
        marker.gap_start_ms = if gap_start_raw == u64::MAX {
            now_ms // empty store: zero-width gap
        } else {
            gap_start_raw
        };
        marker.gap_end_ms = now_ms;
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: now_ms,
            partitions,
        };
        marker.phase = MigrationPhase::Exporting;
        write_marker(db_path, marker)?;
        export_bundle(db_path, &marker.bundle_path, &manifest, signer)?;
        verify_bundle(&marker.bundle_path, signer, &manifest)?;
        marker.phase = MigrationPhase::Verified;
        write_marker(db_path, marker)?;
    }

    // Destructive reinit, gated behind a verified archive and skipped entirely if
    // a prior run already completed it (so a `Complete` resume touches nothing and
    // just reports). reinit itself is idempotent.
    if marker.phase != MigrationPhase::Complete {
        if at_least_verified(resume_phase) {
            // Resumed past export without re-deriving the manifest — the live store
            // may be gone. Re-validate the archive still exists, verifies, and is
            // partition-complete *before* the destructive drop, so a bundle
            // deleted/corrupted between runs aborts the rebuild instead of emptying
            // the store with no recovery copy.
            verify_bundle_self(&marker.bundle_path, signer)?;
        }
        marker.phase = MigrationPhase::Dropping;
        write_marker(db_path, marker)?;
        reinit_store(db_path)?;
        marker.phase = MigrationPhase::Complete;
        write_marker(db_path, marker)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::models::ProcessRecord;
    use crate::storage::EventStore;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- Test doubles ------------------------------------------------------

    /// A trivial deterministic signer: signature = a fixed tag XOR'd length, so
    /// `verify` is a real check (a corrupted bundle fails) without crypto deps.
    struct FakeSigner {
        sign_calls: Arc<AtomicUsize>,
    }

    impl FakeSigner {
        fn new() -> Self {
            Self {
                sign_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn checksum(bytes: &[u8]) -> Vec<u8> {
            // FNV-1a — order-sensitive, enough to detect a one-byte flip.
            const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
            const FNV_PRIME: u64 = 1_099_511_628_211;
            let mut acc: u64 = FNV_OFFSET;
            for byte in bytes {
                acc ^= u64::from(*byte);
                acc = acc.wrapping_mul(FNV_PRIME);
            }
            acc.to_le_bytes().to_vec()
        }
    }

    impl BundleSigner for FakeSigner {
        fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, SignerError> {
            self.sign_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Self::checksum(bytes))
        }

        fn verify(&self, bytes: &[u8], signature: &[u8]) -> Result<(), SignerError> {
            if Self::checksum(bytes) == signature {
                Ok(())
            } else {
                Err(SignerError("checksum mismatch".to_owned()))
            }
        }
    }

    /// A signer whose `sign` always fails — drives the abort/retain/degraded path.
    struct FailingSigner;

    impl BundleSigner for FailingSigner {
        fn sign(&self, _bytes: &[u8]) -> Result<Vec<u8>, SignerError> {
            Err(SignerError("induced signing failure".to_owned()))
        }

        fn verify(&self, _bytes: &[u8], _signature: &[u8]) -> Result<(), SignerError> {
            Err(SignerError("induced verify failure".to_owned()))
        }
    }

    // ---- Helpers -----------------------------------------------------------

    fn temp_db_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.redb");
        (dir, path)
    }

    /// Open a store, seed one event, then stamp an OLD schema version so the next
    /// open classifies as a mismatch.
    fn seed_old_store(path: &Path, event_ts_ms: u64) {
        let store = EventStore::new(path).unwrap();
        let record = ProcessRecord::new(1234, "victim".to_owned());
        store.put_event(event_ts_ms, 0, &record).unwrap();
        drop(store);
        // Overwrite the version cell with an older value directly.
        let db = Database::open(path).unwrap();
        let wtxn = db.begin_write().unwrap();
        let mut table = wtxn.open_table(SCHEMA_VERSION_TABLE).unwrap();
        table.insert(VERSION_KEY, SCHEMA_VERSION - 1).unwrap();
        drop(table);
        wtxn.commit().unwrap();
        drop(db);
    }

    fn open_db(path: &Path) -> Database {
        Database::open(path).unwrap()
    }

    // ---- Classification (sub-step a) --------------------------------------

    #[test]
    fn classify_fresh_store_is_first_init() {
        let (_dir, path) = temp_db_path();
        let db = Database::create(&path).unwrap();
        assert_eq!(classify(&db).unwrap(), SchemaClassification::FirstInit);
    }

    #[test]
    fn classify_after_write_current_version_is_match() {
        let (_dir, path) = temp_db_path();
        let db = Database::create(&path).unwrap();
        write_current_version(&db).unwrap();
        assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
    }

    #[test]
    fn classify_old_version_is_mismatch() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 1_000);
        let db = open_db(&path);
        assert_eq!(
            classify(&db).unwrap(),
            SchemaClassification::Mismatch {
                found: SCHEMA_VERSION - 1,
                expected: SCHEMA_VERSION,
            }
        );
    }

    #[test]
    fn event_store_open_first_init_stamps_current_version() {
        // A fresh EventStore must classify as Match on reopen (first-init wrote it).
        let (_dir, path) = temp_db_path();
        let store = EventStore::new(&path).unwrap();
        drop(store);
        let db = open_db(&path);
        assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
    }

    // ---- Match path: no export (AE1) --------------------------------------

    #[test]
    fn matching_version_writes_no_export_artifact() {
        let (_dir, path) = temp_db_path();
        let store = EventStore::new(&path).unwrap();
        drop(store);
        // Reopen at the same version — migrate must not be invoked, so no bundle.
        assert!(!default_bundle_path(&path).exists());
        assert!(!marker_path(&path).exists());
    }

    // ---- Mismatch + empty WAL: drop + reinit + gap (AE2) -------------------

    #[test]
    fn mismatch_rebuild_signs_drops_and_reinits_with_gap() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();
        let now_ms = 9_000;

        let report = migrate(&path, &signer, now_ms).unwrap();

        // Signed bundle written; signer was invoked.
        assert!(report.bundle_path.exists());
        assert!(signature_path(&report.bundle_path).exists());
        assert!(signer.sign_calls.load(Ordering::SeqCst) >= 1);

        // Versions and gap.
        assert_eq!(report.from_version, SCHEMA_VERSION - 1);
        assert_eq!(report.to_version, SCHEMA_VERSION);
        assert_eq!(report.gap.start_ms, 5_000);
        assert_eq!(report.gap.end_ms, now_ms);

        // Store reinitialized fresh at the new version, old data gone from live store.
        let db = open_db(&path);
        assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
        drop(db);
        let store = EventStore::open(&path).unwrap();
        assert_eq!(store.event_count().unwrap(), 0);

        // Marker cleaned up on success.
        assert!(!marker_path(&path).exists());
    }

    #[test]
    fn mismatch_on_empty_store_gives_zero_width_gap() {
        let (_dir, path) = temp_db_path();
        // Seed a store with NO events, then age its version.
        let store = EventStore::new(&path).unwrap();
        drop(store);
        let db = open_db(&path);
        let wtxn = db.begin_write().unwrap();
        let mut table = wtxn.open_table(SCHEMA_VERSION_TABLE).unwrap();
        table.insert(VERSION_KEY, SCHEMA_VERSION - 1).unwrap();
        drop(table);
        wtxn.commit().unwrap();
        drop(db);

        let report = migrate(&path, &FakeSigner::new(), 7_777).unwrap();
        assert_eq!(report.gap.start_ms, 7_777);
        assert_eq!(report.gap.end_ms, 7_777);
    }

    // ---- Drop-gating: failure retains old tables (R15) --------------------

    #[test]
    fn export_failure_aborts_and_retains_old_store() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);

        let err = migrate(&path, &FailingSigner, 9_000).unwrap_err();
        // The failure surfaces (not degraded yet on the first failure).
        assert!(matches!(err, StorageError::Bucket { .. }));

        // Old store retained: the event is still present and version still old.
        let db = open_db(&path);
        assert_eq!(
            classify(&db).unwrap(),
            SchemaClassification::Mismatch {
                found: SCHEMA_VERSION - 1,
                expected: SCHEMA_VERSION,
            }
        );
        drop(db);

        // A marker records the failure for the degraded gate. The phase is NOT
        // regressed to an "aborted" sentinel — it stays at the last step reached
        // (export, before any verified bundle), so a resume re-derives from the
        // still-intact live store and never re-runs a destructive step.
        let marker = read_marker(&path).unwrap().unwrap();
        assert_eq!(marker.phase, MigrationPhase::Exporting);
        assert_eq!(marker.failure_count, 1);
    }

    #[test]
    fn repeated_deterministic_failure_enters_degraded_mode() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);

        // Fail up to the threshold; the final call returns the degraded terminal.
        let mut last = None;
        for _ in 0..MAX_REBUILD_FAILURES {
            last = Some(migrate(&path, &FailingSigner, 9_000));
        }
        let err = last.unwrap().unwrap_err();
        assert!(matches!(err, StorageError::DegradedReadOnly { .. }));

        // Old store still intact — degraded means read-only, not destroyed.
        let db = open_db(&path);
        assert!(matches!(
            classify(&db).unwrap(),
            SchemaClassification::Mismatch { .. }
        ));
    }

    // ---- Completeness gate (missed-partition guard) -----------------------

    #[test]
    fn verify_bundle_rejects_manifest_with_phantom_partition() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();

        // Build a real bundle, then verify against a manifest claiming an extra
        // partition the archive does not contain.
        let (from_version, mut partitions, _gap) = inspect_store(&path).unwrap();
        let real_manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: 1,
            partitions: partitions.clone(),
        };
        let bundle_path = default_bundle_path(&path);
        export_bundle(&path, &bundle_path, &real_manifest, &signer).unwrap();

        partitions.push(PartitionInfo {
            name: "phantom.partition".to_owned(),
            entries: 0,
        });
        let inflated = BundleManifest {
            partitions,
            ..real_manifest
        };
        let err = verify_bundle(&bundle_path, &signer, &inflated).unwrap_err();
        assert!(matches!(err, StorageError::Bucket { .. }));
    }

    // ---- Verify read-back: bit flip fails the gate ------------------------

    #[test]
    fn bitflip_in_bundle_fails_verification() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();
        let (from_version, partitions, _gap) = inspect_store(&path).unwrap();
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: 1,
            partitions,
        };
        let bundle_path = default_bundle_path(&path);
        export_bundle(&path, &bundle_path, &manifest, &signer).unwrap();

        // Corrupt the last byte of the on-disk bundle (in the embedded db region).
        let mut bytes = fs::read(&bundle_path).unwrap();
        let last = bytes.len().saturating_sub(1);
        if let Some(byte) = bytes.get_mut(last) {
            *byte ^= 0xFF;
        }
        fs::write(&bundle_path, &bytes).unwrap();

        let err = verify_bundle(&bundle_path, &signer, &manifest).unwrap_err();
        assert!(matches!(err, StorageError::Bucket { .. }));
    }

    // ---- Resumability: resume from the sidecar marker ---------------------

    #[test]
    fn resume_after_verified_skips_reexport_and_completes() {
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();

        // Build + verify a real bundle, then hand-write a Verified marker as if a
        // crash struck just before the drop.
        let (from_version, partitions, gap_start) = inspect_store(&path).unwrap();
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: 1,
            partitions,
        };
        let bundle_path = default_bundle_path(&path);
        export_bundle(&path, &bundle_path, &manifest, &signer).unwrap();
        // The marker carries the ORIGINAL gap window (start 5000, end 8000) from
        // the run that began the rebuild.
        write_marker(
            &path,
            &ProgressMarker {
                phase: MigrationPhase::Verified,
                from_version,
                to_version: SCHEMA_VERSION,
                bundle_path,
                gap_start_ms: gap_start,
                gap_end_ms: 8_000,
                failure_count: 0,
            },
        )
        .unwrap();
        let signs_before = signer.sign_calls.load(Ordering::SeqCst);

        // Resume with a LATER now (9000): must NOT re-export (no new sign call) and
        // must complete the drop. The report must reflect the marker's preserved
        // from_version and gap — recomputing from the emptied store would give
        // from_version == SCHEMA_VERSION and a zero-width gap.
        let report = migrate(&path, &signer, 9_000).unwrap();
        assert_eq!(signer.sign_calls.load(Ordering::SeqCst), signs_before);
        assert_eq!(report.from_version, SCHEMA_VERSION - 1);
        assert_eq!(report.gap.start_ms, 5_000);
        assert_eq!(report.gap.end_ms, 8_000);

        let db = open_db(&path);
        assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
        assert!(!marker_path(&path).exists());
    }

    #[test]
    fn resume_after_dropping_completes_idempotently_from_marker() {
        // A crash struck mid-reinit (marker = Dropping): the live store may be
        // half-gone, but the verified bundle is the durable copy. Resume must
        // finish the reinit and report the marker's preserved gap — never
        // re-export, never recompute the gap from the emptied store.
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();
        let (from_version, partitions, _gap) = inspect_store(&path).unwrap();
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: 1,
            partitions,
        };
        let bundle_path = default_bundle_path(&path);
        export_bundle(&path, &bundle_path, &manifest, &signer).unwrap();
        write_marker(
            &path,
            &ProgressMarker {
                phase: MigrationPhase::Dropping,
                from_version,
                to_version: SCHEMA_VERSION,
                bundle_path,
                gap_start_ms: 5_000,
                gap_end_ms: 8_000,
                failure_count: 0,
            },
        )
        .unwrap();
        let signs_before = signer.sign_calls.load(Ordering::SeqCst);

        let report = migrate(&path, &signer, 9_000).unwrap();
        assert_eq!(signer.sign_calls.load(Ordering::SeqCst), signs_before);
        assert_eq!(report.from_version, SCHEMA_VERSION - 1);
        assert_eq!(report.gap.start_ms, 5_000);
        let db = open_db(&path);
        assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
    }

    #[test]
    fn resume_after_complete_reports_without_touching_store() {
        // A crash struck after the rebuild finished but before the marker was
        // cleaned up (marker = Complete). Resume must do nothing destructive —
        // no re-export, no second reinit — and just report from the marker.
        let (_dir, path) = temp_db_path();
        // The store is already reinitialized at the current version.
        let store = EventStore::new(&path).unwrap();
        store
            .put_event(20_000, 0, &ProcessRecord::new(1, "fresh".to_owned()))
            .unwrap();
        drop(store);

        let signer = FakeSigner::new();
        let bundle_path = default_bundle_path(&path);
        // A Complete marker with a preserved gap; no bundle file needed since the
        // Complete path must not touch it.
        write_marker(
            &path,
            &ProgressMarker {
                phase: MigrationPhase::Complete,
                from_version: SCHEMA_VERSION - 1,
                to_version: SCHEMA_VERSION,
                bundle_path,
                gap_start_ms: 5_000,
                gap_end_ms: 8_000,
                failure_count: 0,
            },
        )
        .unwrap();

        let report = migrate(&path, &signer, 9_000).unwrap();
        assert_eq!(signer.sign_calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.from_version, SCHEMA_VERSION - 1);
        assert_eq!(report.gap.start_ms, 5_000);
        assert_eq!(report.gap.end_ms, 8_000);

        // The already-rebuilt store is untouched: the fresh event is still there.
        let reopened = EventStore::open(&path).unwrap();
        assert_eq!(reopened.event_count().unwrap(), 1);
        assert!(!marker_path(&path).exists());
    }

    #[test]
    fn resume_past_verified_aborts_drop_when_bundle_is_missing() {
        // The catastrophe guard: resuming past export with a MISSING archive must
        // abort before the destructive drop — never empty the store when the only
        // recovery copy is gone.
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();
        let bundle_path = default_bundle_path(&path);
        // Verified marker, but the bundle was never written / was deleted.
        write_marker(
            &path,
            &ProgressMarker {
                phase: MigrationPhase::Verified,
                from_version: SCHEMA_VERSION - 1,
                to_version: SCHEMA_VERSION,
                bundle_path,
                gap_start_ms: 5_000,
                gap_end_ms: 8_000,
                failure_count: 0,
            },
        )
        .unwrap();

        let err = migrate(&path, &signer, 9_000).unwrap_err();
        assert!(matches!(err, StorageError::IoError { .. }));

        // The live store is RETAINED: the original event survives, version unchanged.
        let db = open_db(&path);
        assert!(matches!(
            classify(&db).unwrap(),
            SchemaClassification::Mismatch { .. }
        ));
    }

    #[test]
    fn resume_past_verified_aborts_drop_when_bundle_is_corrupted() {
        // Same catastrophe guard, but the bundle EXISTS and is corrupted: the
        // resume re-verification must reject it and abort before the drop, never
        // empty the store behind a bundle that no longer verifies.
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let signer = FakeSigner::new();
        let (from_version, partitions, _gap) = inspect_store(&path).unwrap();
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: from_version,
            written_at_ms: 1,
            partitions,
        };
        let bundle_path = default_bundle_path(&path);
        export_bundle(&path, &bundle_path, &manifest, &signer).unwrap();
        // Corrupt the on-disk bundle after it was signed.
        let mut bytes = fs::read(&bundle_path).unwrap();
        let last = bytes.len().saturating_sub(1);
        if let Some(byte) = bytes.get_mut(last) {
            *byte ^= 0xFF;
        }
        fs::write(&bundle_path, &bytes).unwrap();
        write_marker(
            &path,
            &ProgressMarker {
                phase: MigrationPhase::Verified,
                from_version,
                to_version: SCHEMA_VERSION,
                bundle_path,
                gap_start_ms: 5_000,
                gap_end_ms: 8_000,
                failure_count: 0,
            },
        )
        .unwrap();

        let err = migrate(&path, &signer, 9_000).unwrap_err();
        assert!(matches!(err, StorageError::Bucket { .. }));

        // Live store retained — the corrupted archive never gated a destroy.
        let db = open_db(&path);
        assert!(matches!(
            classify(&db).unwrap(),
            SchemaClassification::Mismatch { .. }
        ));
    }

    // ---- Bundle format round-trips (R18) ----------------------------------

    #[test]
    fn manifest_round_trips_through_postcard() {
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            schema_version: 3,
            written_at_ms: 42,
            partitions: vec![PartitionInfo {
                name: "processes.events@7".to_owned(),
                entries: 9,
            }],
        };
        let bytes = postcard::to_allocvec(&manifest).unwrap();
        let decoded: BundleManifest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, manifest);
    }

    // ---- Bundle permissions (Unix 0600) -----------------------------------

    #[cfg(unix)]
    #[test]
    fn bundle_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = temp_db_path();
        seed_old_store(&path, 5_000);
        let report = migrate(&path, &FakeSigner::new(), 9_000).unwrap();
        let mode = fs::metadata(&report.bundle_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
