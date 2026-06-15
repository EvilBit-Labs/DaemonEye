//! Database abstractions and storage management.
//!
//! This module provides database operations using redb for optimal performance and security.
//! It includes table definitions, transaction handling, and data serialization.
//!
//! # Module layout (T3 · M2 event store)
//!
//! The event-store engine is organized by domain under `storage/`:
//! - `error` — [`StorageError`], shared across the engine and the legacy
//!   [`DatabaseManager`].
//! - `codec` — custom `redb::Value`/`redb::Key` impls (fixed-width `(ts_ms,
//!   seq)` keys, version-tagged postcard values).
//! - `bucket` — time-bucket partitioning of `processes.events`.
//! - `index` — secondary multimap indexes over pid, ppid, name, and
//!   `exe_hash`, each mapping a term to `(ts_ms, seq)` pointers.
//!
//! [`EventStore`] is the agent-owned, single-writer store that supersedes the
//! stubbed [`DatabaseManager`]; the latter is retained so procmond's existing
//! callers keep working until its storage is reconciled (T8).

mod bucket;
mod codec;
mod error;
mod index;
pub mod ingest;
pub mod mrc;
mod records;
pub mod schema;

pub use error::StorageError;
pub use schema::{
    BundleManifest, BundleSigner, GapRecord, MigrationReport, SCHEMA_VERSION, SchemaClassification,
    SignerError,
};

use crate::models::{Alert, DetectionRule, ProcessRecord, SystemInfo};
use bucket::{
    bucket_id, bucket_table_name, choose_granularity, parse_bucket_name, retention_cutoff,
};
use codec::{TsSeqKey, decode_value, encode_value};
use index::{
    exe_hash_prefix, exe_index_name, hash_index_def, name_hash128, name_index_name, pid_index_name,
    ppid_index_name, u32_index_def,
};
use ingest::IngestRecord;
use mrc::MrcMap;
use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTableMetadata, TableDefinition,
    TableHandle,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io, path::Path};

/// redb table type for a process-event bucket: `(ts_ms, seq)` → versioned bytes.
type EventTable<'a> = TableDefinition<'a, TsSeqKey, &'static [u8]>;

/// Build the redb table definition for a bucket table name.
const fn bucket_def(name: &str) -> EventTable<'_> {
    TableDefinition::new(name)
}

/// Collect the live process-event bucket ids from a read transaction, ascending.
fn collect_bucket_ids(txn: &ReadTransaction) -> Result<Vec<u64>, StorageError> {
    let mut ids: Vec<u64> = txn
        .list_tables()?
        .filter_map(|handle| parse_bucket_name(handle.name()))
        .collect();
    ids.sort_unstable();
    Ok(ids)
}

/// Agent-owned, single-writer event store (T3 · M2).
///
/// Supersedes the stubbed [`DatabaseManager`]. `processes.events` is partitioned
/// into per-time-window bucket tables (`processes.events@<id>`, U3) over the U2
/// codecs; secondary indexes (U4), the single-writer ingest pipeline (U5/U6),
/// the read handle + MRC (U7), and the signed schema-rebuild path (U8) build on
/// this foundation.
pub struct EventStore {
    db: Database,
    /// Bucket granularity in milliseconds (hourly by default; coarsens to daily
    /// for long retention windows so the live-bucket count stays bounded).
    granularity_ms: u64,
    /// Retention window in milliseconds; buckets older than this are dropped.
    retention_ms: u64,
    /// Recent window (ms) the in-memory MRC parent map is rebuilt from on start.
    mrc_window_ms: u64,
}

impl EventStore {
    /// Default retention window: 7 days, in milliseconds.
    const DEFAULT_RETENTION_MS: u64 = 604_800_000;

    /// Default MRC rebuild window: 30 minutes, in milliseconds (§11.7.7).
    const DEFAULT_MRC_WINDOW_MS: u64 = 1_800_000;

    /// Create (or open) the event store at `path`.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();
        ensure_db_dir(db_path)?;
        let db =
            Database::create(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;
        let retention_ms = Self::DEFAULT_RETENTION_MS;
        let store = Self {
            db,
            granularity_ms: choose_granularity(retention_ms),
            retention_ms,
            mrc_window_ms: Self::DEFAULT_MRC_WINDOW_MS,
        };
        store.classify_and_init()?;
        Ok(store)
    }

    /// Open an existing event store at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();
        ensure_db_dir(db_path)?;
        let db =
            Database::open(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;
        let retention_ms = Self::DEFAULT_RETENTION_MS;
        let store = Self {
            db,
            granularity_ms: choose_granularity(retention_ms),
            retention_ms,
            mrc_window_ms: Self::DEFAULT_MRC_WINDOW_MS,
        };
        store.classify_and_init()?;
        Ok(store)
    }

    /// Classify the store's `schema_version` (U8): a fresh store is stamped with
    /// the current version; a match proceeds; a mismatch is surfaced so the agent
    /// can run the export → rebuild path ([`schema::migrate`]) before reopening.
    fn classify_and_init(&self) -> Result<(), StorageError> {
        match schema::classify(&self.db)? {
            SchemaClassification::FirstInit => schema::write_current_version(&self.db),
            SchemaClassification::Match => Ok(()),
            SchemaClassification::Mismatch { found, expected } => {
                Err(StorageError::SchemaVersionMismatch { found, expected })
            }
        }
    }

    /// Persist a single process event keyed by `(ts_ms, seq)`, routed to the
    /// time bucket for `ts_ms`. ACID: one write transaction per call. The ingest
    /// pipeline (U5) batches many of these into a single group commit.
    pub fn put_event(
        &self,
        ts_ms: u64,
        seq: u32,
        record: &ProcessRecord,
    ) -> Result<(), StorageError> {
        let bytes = encode_value(record)?;
        let id = bucket_id(ts_ms, self.granularity_ms)?;
        let key = (ts_ms, seq);
        let txn = self.db.begin_write()?;

        let mut base = txn.open_table(bucket_def(&bucket_table_name(id)))?;
        base.insert(key, bytes.as_slice())?;
        drop(base);

        let mut pid_idx = txn.open_multimap_table(u32_index_def(&pid_index_name(id)))?;
        pid_idx.insert(record.pid.raw(), key)?;
        drop(pid_idx);

        if let Some(ppid) = record.ppid {
            let mut ppid_idx = txn.open_multimap_table(u32_index_def(&ppid_index_name(id)))?;
            ppid_idx.insert(ppid.raw(), key)?;
            drop(ppid_idx);
        }

        let mut name_idx = txn.open_multimap_table(hash_index_def(&name_index_name(id)))?;
        name_idx.insert(name_hash128(&record.name), key)?;
        drop(name_idx);

        if let Some(prefix) = record.executable_hash.as_deref().and_then(exe_hash_prefix) {
            let mut exe_idx = txn.open_multimap_table(hash_index_def(&exe_index_name(id)))?;
            exe_idx.insert(prefix, key)?;
            drop(exe_idx);
        }

        txn.commit()?;
        Ok(())
    }

    /// Group-commit a batch of records in a single write transaction — the
    /// ingest pipeline's (U5) commit path. Records are grouped by time bucket so
    /// each bucket's tables are opened once. Commits use redb's default
    /// `Durability::Immediate` (one fsync per group), so the R7/R8 durability
    /// guarantees hold. `collector_id` / `source_seq` on each record are the
    /// pipeline's dedup concern and are not persisted here.
    pub fn put_batch(&self, records: &[IngestRecord]) -> Result<(), StorageError> {
        if records.is_empty() {
            return Ok(());
        }
        let mut by_bucket: BTreeMap<u64, Vec<&IngestRecord>> = BTreeMap::new();
        for rec in records {
            let id = bucket_id(rec.ts_ms, self.granularity_ms)?;
            by_bucket.entry(id).or_default().push(rec);
        }
        let txn = self.db.begin_write()?;
        for (id, recs) in &by_bucket {
            let mut base = txn.open_table(bucket_def(&bucket_table_name(*id)))?;
            let mut pid_idx = txn.open_multimap_table(u32_index_def(&pid_index_name(*id)))?;
            let mut ppid_idx = txn.open_multimap_table(u32_index_def(&ppid_index_name(*id)))?;
            let mut name_idx = txn.open_multimap_table(hash_index_def(&name_index_name(*id)))?;
            let mut exe_idx = txn.open_multimap_table(hash_index_def(&exe_index_name(*id)))?;
            for rec in recs {
                let bytes = encode_value(&rec.record)?;
                let key = (rec.ts_ms, rec.seq);
                base.insert(key, bytes.as_slice())?;
                pid_idx.insert(rec.record.pid.raw(), key)?;
                if let Some(ppid) = rec.record.ppid {
                    ppid_idx.insert(ppid.raw(), key)?;
                }
                name_idx.insert(name_hash128(&rec.record.name), key)?;
                if let Some(prefix) = rec
                    .record
                    .executable_hash
                    .as_deref()
                    .and_then(exe_hash_prefix)
                {
                    exe_idx.insert(prefix, key)?;
                }
            }
            drop(exe_idx);
            drop(name_idx);
            drop(ppid_idx);
            drop(pid_idx);
            drop(base);
        }
        txn.commit()?;
        Ok(())
    }

    /// All process records whose `pid` matches, across every live bucket, via
    /// the `idx:pid` secondary index.
    pub fn find_by_pid(&self, pid: u32) -> Result<Vec<ProcessRecord>, StorageError> {
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for id in collect_bucket_ids(&txn)? {
            let idx = match txn.open_multimap_table(u32_index_def(&pid_index_name(id))) {
                Ok(idx) => idx,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(other) => return Err(other.into()),
            };
            let base = txn.open_table(bucket_def(&bucket_table_name(id)))?;
            for posting in idx.get(pid)? {
                let (ts, seq) = posting?.value();
                if let Some(guard) = base.get((ts, seq))? {
                    out.push(decode_value(guard.value())?);
                }
            }
        }
        Ok(out)
    }

    /// All process records whose `name` matches, across every live bucket. The
    /// `idx:name` index is keyed by a 128-bit hash, so each candidate posting is
    /// verified against the primary row's exact name before inclusion — this is
    /// the collision-verify path that guards against hash collisions.
    pub fn find_by_name(&self, name: &str) -> Result<Vec<ProcessRecord>, StorageError> {
        let hash = name_hash128(name);
        // The index lowercases names, so the collision verify is case-insensitive
        // too — a candidate matches when its lowercased name equals the query's.
        let lowered_query = name.to_lowercase();
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for id in collect_bucket_ids(&txn)? {
            let idx = match txn.open_multimap_table(hash_index_def(&name_index_name(id))) {
                Ok(idx) => idx,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(other) => return Err(other.into()),
            };
            let base = txn.open_table(bucket_def(&bucket_table_name(id)))?;
            for posting in idx.get(hash)? {
                let (ts, seq) = posting?.value();
                if let Some(guard) = base.get((ts, seq))? {
                    let record: ProcessRecord = decode_value(guard.value())?;
                    if record.name.to_lowercase() == lowered_query {
                        out.push(record);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Fetch the process event stored at `(ts_ms, seq)`, if any.
    pub fn get_event(&self, ts_ms: u64, seq: u32) -> Result<Option<ProcessRecord>, StorageError> {
        let name = bucket_table_name(bucket_id(ts_ms, self.granularity_ms)?);
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(bucket_def(&name)) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(other) => return Err(other.into()),
        };
        match table.get((ts_ms, seq))? {
            Some(guard) => Ok(Some(decode_value(guard.value())?)),
            None => Ok(None),
        }
    }

    /// Range scan: every process record with `ts_ms` in `[start_ms, end_ms)`,
    /// ascending. The agent's internal read handle (R9); the CLI consumes it over
    /// IPC. Resolves the time window to the bucket subset (U3), then a key range
    /// per bucket via redb's MVCC read snapshot.
    pub fn scan_range(
        &self,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<ProcessRecord>, StorageError> {
        if end_ms <= start_ms {
            return Ok(Vec::new());
        }
        let start_bucket = bucket_id(start_ms, self.granularity_ms)?;
        let end_bucket = bucket_id(end_ms.saturating_sub(1), self.granularity_ms)?;
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for id in collect_bucket_ids(&txn)? {
            if id < start_bucket || id > end_bucket {
                continue;
            }
            let table = match txn.open_table(bucket_def(&bucket_table_name(id))) {
                Ok(table) => table,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(other) => return Err(other.into()),
            };
            for entry in table.range((start_ms, 0_u32)..(end_ms, 0_u32))? {
                let (_key, value) = entry?;
                out.push(decode_value(value.value())?);
            }
        }
        Ok(out)
    }

    /// Sorted `(ts_ms, seq)` postings for a pid across all buckets — the
    /// cache-shaped index-scan primitive T6's planner/LRU wraps (pointers only,
    /// no primary-row fetch, no intersection).
    pub fn pid_postings(&self, pid: u32) -> Result<Vec<(u64, u32)>, StorageError> {
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for id in collect_bucket_ids(&txn)? {
            let idx = match txn.open_multimap_table(u32_index_def(&pid_index_name(id))) {
                Ok(idx) => idx,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(other) => return Err(other.into()),
            };
            for posting in idx.get(pid)? {
                out.push(posting?.value());
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Build the in-memory MRC parent map from the recent window
    /// `[now_ms - mrc_window, now_ms]`. Rebuilt on start; a cache, never a
    /// persistence dependency.
    pub fn build_mrc(&self, now_ms: u64) -> Result<MrcMap, StorageError> {
        let start = now_ms.saturating_sub(self.mrc_window_ms);
        let records = self.scan_range(start, now_ms.saturating_add(1))?;
        Ok(mrc::build_mrc(&records))
    }

    // ---- Plain-table persistence (rules, alerts, scans) — U9 ---------------
    //
    // These small, directly-keyed tables live alongside the time-bucketed events
    // under the same `schema_version` governance. Implementations live in
    // [`records`]; the methods here are the public facade the agent and CLI use.

    /// Persist (upsert) a detection rule. Reloading these on start is the fix for
    /// the origin "zero rules after restart" bug.
    pub fn store_rule(&self, rule: &DetectionRule) -> Result<(), StorageError> {
        records::store_rule(&self.db, rule)
    }

    /// Fetch a detection rule by id.
    pub fn get_rule(&self, id: &str) -> Result<Option<DetectionRule>, StorageError> {
        records::get_rule(&self.db, id)
    }

    /// Fetch every persisted detection rule.
    pub fn get_all_rules(&self) -> Result<Vec<DetectionRule>, StorageError> {
        records::all_rules(&self.db)
    }

    /// Persist (upsert) an alert keyed by its uuid.
    pub fn store_alert(&self, alert: &Alert) -> Result<(), StorageError> {
        records::store_alert(&self.db, alert)
    }

    /// Fetch an alert by its uuid string.
    pub fn get_alert(&self, id: &str) -> Result<Option<Alert>, StorageError> {
        records::get_alert(&self.db, id)
    }

    /// Fetch every persisted alert.
    pub fn get_all_alerts(&self) -> Result<Vec<Alert>, StorageError> {
        records::all_alerts(&self.db)
    }

    /// Persist (upsert) scan metadata (carrying host identity, U9).
    pub fn store_scan(&self, scan: &ScanMetadata) -> Result<(), StorageError> {
        records::store_scan(&self.db, scan)
    }

    /// Fetch scan metadata by scan id.
    pub fn get_scan(&self, id: &str) -> Result<Option<ScanMetadata>, StorageError> {
        records::get_scan(&self.db, id)
    }

    /// Fetch every persisted scan metadata record.
    pub fn get_all_scans(&self) -> Result<Vec<ScanMetadata>, StorageError> {
        records::all_scans(&self.db)
    }

    /// List the live bucket ids in ascending order (discovered via `list_tables`).
    pub fn list_buckets(&self) -> Result<Vec<u64>, StorageError> {
        let txn = self.db.begin_read()?;
        let mut ids: Vec<u64> = txn
            .list_tables()?
            .filter_map(|handle| parse_bucket_name(handle.name()))
            .collect();
        ids.sort_unstable();
        Ok(ids)
    }

    /// Count the events across all buckets (test/diagnostic helper).
    pub fn event_count(&self) -> Result<u64, StorageError> {
        let txn = self.db.begin_read()?;
        let names: Vec<String> = txn
            .list_tables()?
            .filter(|handle| parse_bucket_name(handle.name()).is_some())
            .map(|handle| handle.name().to_owned())
            .collect();
        let mut total = 0_u64;
        for name in &names {
            let table = txn.open_table(bucket_def(name))?;
            total = total.saturating_add(table.len()?);
        }
        Ok(total)
    }

    /// Drop every bucket entirely older than the retention window relative to
    /// `now_ms`. Returns the number of buckets dropped (an O(1) `delete_table`
    /// each). Callers observe the count to assert retention actually ran.
    pub fn drop_expired(&self, now_ms: u64) -> Result<usize, StorageError> {
        let cutoff = retention_cutoff(now_ms, self.retention_ms, self.granularity_ms)?;
        let expired_ids: Vec<u64> = {
            let rtxn = self.db.begin_read()?;
            rtxn.list_tables()?
                .filter_map(|handle| parse_bucket_name(handle.name()))
                .filter(|&id| id < cutoff)
                .collect()
        };
        if expired_ids.is_empty() {
            return Ok(0);
        }
        let txn = self.db.begin_write()?;
        let mut dropped = 0_usize;
        for id in &expired_ids {
            if txn.delete_table(bucket_def(&bucket_table_name(*id)))? {
                dropped = dropped.saturating_add(1);
            }
            // Drop the bucket's secondary indexes alongside the base table.
            txn.delete_multimap_table(u32_index_def(&pid_index_name(*id)))?;
            txn.delete_multimap_table(u32_index_def(&ppid_index_name(*id)))?;
            txn.delete_multimap_table(hash_index_def(&name_index_name(*id)))?;
            txn.delete_multimap_table(hash_index_def(&exe_index_name(*id)))?;
        }
        txn.commit()?;
        Ok(dropped)
    }
}

/// Validates that the directory containing the database file exists and is accessible.
/// This provides platform-agnostic error messages before attempting to open the database.
fn ensure_db_dir(db_path: &Path) -> Result<(), StorageError> {
    if let Some(dir) = db_path.parent() {
        match fs::metadata(dir) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(StorageError::NotADirectory {
                        path: dir.to_path_buf(),
                    });
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StorageError::MissingDirectory {
                dir: dir.to_path_buf(),
                source: Some(e),
            }),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                Err(StorageError::DirectoryPermissionDenied {
                    path: dir.to_path_buf(),
                    source: e,
                })
            }
            Err(e) => Err(StorageError::IoError {
                path: dir.to_path_buf(),
                source: e,
            }),
        }
    } else {
        // Paths like "test.db" (no parent directory) are acceptable
        Ok(())
    }
}

/// Table definitions for the database schema.
pub struct Tables;

impl Tables {
    /// Process records table
    /// TODO: Use `ProcessRecord` type in Task 8 when redb Value trait is implemented
    pub const PROCESSES: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("processes");

    /// Detection rules table
    /// TODO: Use `DetectionRule` type in Task 8 when redb Value trait is implemented
    pub const DETECTION_RULES: TableDefinition<'static, &str, Vec<u8>> =
        TableDefinition::new("detection_rules");

    /// Alerts table
    /// TODO: Use Alert type in Task 8 when redb Value trait is implemented
    pub const ALERTS: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("alerts");

    /// System info table
    /// TODO: Use `SystemInfo` type in Task 8 when redb Value trait is implemented
    pub const SYSTEM_INFO: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("system_info");

    /// Scan metadata table
    /// TODO: Use `ScanMetadata` type in Task 8 when redb Value trait is implemented
    pub const SCAN_METADATA: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("scan_metadata");
}

/// Scan metadata for tracking collection operations.
///
/// Carries the host's **static** identity (hostname, OS, architecture) folded in
/// from `SystemInfo` (U9), so every scan is self-describing for cross-host
/// stitching. The volatile `SystemInfo` stats (cpu cores, total memory, uptime)
/// are deliberately *not* folded — they are point-in-time samples, not identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanMetadata {
    /// Scan ID
    pub scan_id: String,
    /// Scan timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Number of processes collected
    pub process_count: usize,
    /// Scan duration in milliseconds
    pub duration_ms: u64,
    /// Scan status
    pub status: ScanStatus,
    /// Error message if scan failed
    pub error_message: Option<String>,
    /// Host name the scan ran on (folded from `SystemInfo`).
    pub hostname: String,
    /// Operating system name (folded from `SystemInfo`).
    pub os_name: String,
    /// Operating system version (folded from `SystemInfo`).
    pub os_version: String,
    /// CPU architecture (folded from `SystemInfo`).
    pub architecture: String,
}

impl ScanMetadata {
    /// Build scan metadata for a completed scan, folding the host's static
    /// identity from `system`.
    pub fn new(
        scan_id: impl Into<String>,
        timestamp: chrono::DateTime<chrono::Utc>,
        process_count: usize,
        duration_ms: u64,
        status: ScanStatus,
        system: &SystemInfo,
    ) -> Self {
        Self {
            scan_id: scan_id.into(),
            timestamp,
            process_count,
            duration_ms,
            status,
            error_message: None,
            hostname: system.hostname.clone(),
            os_name: system.os_name.clone(),
            os_version: system.os_version.clone(),
            architecture: system.architecture.clone(),
        }
    }

    /// Attach a failure message (builder-style), marking the scan errored.
    #[must_use]
    pub fn with_error(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self.status = ScanStatus::Failed;
        self
    }
}

/// Scan status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanStatus {
    InProgress,
    Completed,
    Failed,
}

/// Database manager for `DaemonEye` storage operations.
pub struct DatabaseManager {
    db: Database,
}

impl DatabaseManager {
    /// Create a new database manager.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();

        // Platform-agnostic preflight directory checks
        ensure_db_dir(db_path)?;

        // Create the database, mapping errors to friendly messages
        let db =
            Database::create(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;

        let manager = Self { db };
        manager.initialize_schema()?;
        Ok(manager)
    }

    /// Open an existing database.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();

        // Platform-agnostic preflight directory checks
        ensure_db_dir(db_path)?;

        // Open the database, mapping errors to friendly messages
        let db =
            Database::open(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;

        Ok(Self { db })
    }

    /// Initialize the database schema.
    fn initialize_schema(&self) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Store a process record.
    pub fn store_process(&self, _id: u64, _process: &ProcessRecord) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Store multiple process records in a batch.
    pub fn store_processes_batch(
        &self,
        _processes: &[(u64, ProcessRecord)],
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get a process record by ID.
    pub fn get_process(&self, _id: u64) -> Result<Option<ProcessRecord>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all process records.
    pub fn get_all_processes(&self) -> Result<Vec<ProcessRecord>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Store a detection rule.
    pub fn store_rule(&self, _rule: &DetectionRule) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get a detection rule by ID.
    pub fn get_rule(&self, _id: &str) -> Result<Option<DetectionRule>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all detection rules.
    pub fn get_all_rules(&self) -> Result<Vec<DetectionRule>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Store an alert.
    pub fn store_alert(&self, _id: u64, _alert: &Alert) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get an alert by ID.
    pub fn get_alert(&self, _id: u64) -> Result<Option<Alert>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::ALERTS)?;
        // Ok(table
        //     .get(id)
        //     .map_err(StorageError::from)?
        //     .map(|guard| guard.value().clone()))
        Ok(None)
    }

    /// Get all alerts.
    pub fn get_all_alerts(&self) -> Result<Vec<Alert>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::ALERTS)?;
        // let mut alerts = Vec::new();

        // for result in table.iter().map_err(StorageError::from)? {
        //     let (_, alert) = result.map_err(StorageError::from)?;
        //     alerts.push(alert.value().clone());
        // }

        // Ok(alerts)
        Ok(Vec::new())
    }

    /// Store system information.
    pub fn store_system_info(
        &self,
        _id: u64,
        _system_info: &SystemInfo,
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // {
        //     let mut table = write_txn.open_table(Tables::SYSTEM_INFO)?;
        //     table.insert(id, system_info).map_err(StorageError::from)?;
        // }
        // write_txn.commit()?;
        Ok(())
    }

    /// Get the latest system information.
    pub fn get_latest_system_info(&self) -> Result<Option<SystemInfo>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::SYSTEM_INFO)?;

        // // Get the latest entry (highest ID)
        // let mut latest: Option<SystemInfo> = None;
        // for result in table.iter().map_err(StorageError::from)? {
        //     let (_, system_info) = result.map_err(StorageError::from)?;
        //     latest = Some(system_info.value().clone());
        // }

        // Ok(latest)
        Ok(None)
    }

    /// Store scan metadata.
    pub fn store_scan_metadata(
        &self,
        _id: u64,
        _metadata: &ScanMetadata,
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // {
        //     let mut table = write_txn.open_table(Tables::SCAN_METADATA)?;
        //     table.insert(id, metadata).map_err(StorageError::from)?;
        // }
        // write_txn.commit()?;
        Ok(())
    }

    /// Get scan metadata by ID.
    pub fn get_scan_metadata(&self, _id: u64) -> Result<Option<ScanMetadata>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all scan metadata.
    pub fn get_all_scan_metadata(&self) -> Result<Vec<ScanMetadata>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Clean up old data based on retention policy.
    pub fn cleanup_old_data(&self, retention_days: u32) -> Result<usize, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _cutoff_time = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::days(i64::from(retention_days)))
            .unwrap_or_else(chrono::Utc::now);
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(0)
    }

    /// Get database statistics.
    pub fn get_stats(&self) -> Result<DatabaseStats, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(DatabaseStats::default())
    }
}

/// Database statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseStats {
    pub processes: usize,
    pub rules: usize,
    pub alerts: usize,
    pub system_info: usize,
    pub scans: usize,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::let_underscore_must_use)]
mod tests {
    use super::*;
    use crate::models::AlertSeverity;
    use tempfile::tempdir;

    #[test]
    fn test_database_creation() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let _manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");
        assert!(db_path.exists());
    }

    #[test]
    fn event_store_round_trips_a_process_record_through_real_redb() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("events.redb");
        let store = EventStore::new(&db_path).expect("create event store");

        let record = ProcessRecord::new(4321, "subscribed-collector".to_owned());
        store
            .put_event(1_700_000_000_000, 7, &record)
            .expect("put event");

        let fetched = store
            .get_event(1_700_000_000_000, 7)
            .expect("get event")
            .expect("event present");
        assert_eq!(fetched.pid, record.pid);
        assert_eq!(fetched.name, record.name);
        assert_eq!(store.event_count().expect("count"), 1);

        // A different key returns None, not the stored row.
        assert!(
            store
                .get_event(1_700_000_000_000, 8)
                .expect("get miss")
                .is_none()
        );
    }

    #[test]
    fn event_store_get_on_fresh_store_returns_none_not_error() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("fresh.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        assert!(store.get_event(1, 1).expect("get on empty").is_none());
        assert_eq!(store.event_count().expect("count empty"), 0);
    }

    #[test]
    fn event_store_routes_events_into_per_hour_buckets() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("buckets.redb");
        let store = EventStore::new(&db_path).expect("create event store");

        // Three events: two in the same hour, one in the next hour.
        let hour = 3_600_000_u64;
        let base = hour.saturating_mul(100);
        store
            .put_event(base, 1, &ProcessRecord::new(1, "a".to_owned()))
            .expect("put a");
        store
            .put_event(
                base.saturating_add(1000),
                2,
                &ProcessRecord::new(2, "b".to_owned()),
            )
            .expect("put b");
        store
            .put_event(
                base.saturating_add(hour),
                3,
                &ProcessRecord::new(3, "c".to_owned()),
            )
            .expect("put c");

        // Two distinct buckets discovered, ascending; all three events counted.
        let buckets = store.list_buckets().expect("list buckets");
        assert_eq!(buckets.len(), 2, "two distinct hour buckets");
        let first = buckets.first().copied().expect("first bucket");
        let last = buckets.last().copied().expect("last bucket");
        assert!(first < last, "buckets ascending and distinct");
        assert_eq!(store.event_count().expect("count"), 3);

        // Each event is retrievable from its routed bucket.
        assert!(store.get_event(base, 1).expect("get a").is_some());
        assert!(
            store
                .get_event(base.saturating_add(hour), 3)
                .expect("get c")
                .is_some()
        );
    }

    #[test]
    fn event_store_retention_drops_expired_buckets_only() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("retention.redb");
        let store = EventStore::new(&db_path).expect("create event store");

        let hour = 3_600_000_u64;
        // An old event (hour 1) and a recent event (hour 1000).
        store
            .put_event(hour, 1, &ProcessRecord::new(1, "old".to_owned()))
            .expect("put old");
        let recent_ts = hour.saturating_mul(1000);
        store
            .put_event(recent_ts, 2, &ProcessRecord::new(2, "recent".to_owned()))
            .expect("put recent");
        assert_eq!(store.list_buckets().expect("list").len(), 2);

        // "now" = hour 1001 with the default 7-day (168-hour) retention: the
        // hour-1 bucket is far expired; the hour-1000 bucket is inside the window.
        let now = hour.saturating_mul(1001);
        let dropped = store.drop_expired(now).expect("drop expired");
        assert_eq!(dropped, 1, "exactly the expired bucket dropped (mechanism)");

        // The recent event survives; the old one is gone.
        assert!(store.get_event(recent_ts, 2).expect("get recent").is_some());
        assert!(store.get_event(hour, 1).expect("get old").is_none());
        assert_eq!(store.list_buckets().expect("list after").len(), 1);
        assert_eq!(store.event_count().expect("count after"), 1);
    }

    #[test]
    fn event_store_retention_noop_when_nothing_expired() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("retention-noop.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;
        let ts = hour.saturating_mul(1000);
        store
            .put_event(ts, 1, &ProcessRecord::new(1, "recent".to_owned()))
            .expect("put");
        // now just after the event; nothing is past the retention window.
        let dropped = store.drop_expired(ts.saturating_add(hour)).expect("drop");
        assert_eq!(dropped, 0);
        assert_eq!(store.event_count().expect("count"), 1);
    }

    #[test]
    fn event_store_finds_records_by_pid_via_index() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("idx-pid.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;
        let base = hour.saturating_mul(500);

        // pid 1234 appears twice (different ts/seq); pid 9999 once.
        store
            .put_event(base, 1, &ProcessRecord::new(1234, "bash".to_owned()))
            .expect("put 1");
        store
            .put_event(
                base.saturating_add(hour),
                2,
                &ProcessRecord::new(1234, "bash".to_owned()),
            )
            .expect("put 2");
        store
            .put_event(base, 3, &ProcessRecord::new(9999, "zsh".to_owned()))
            .expect("put 3");

        let hits = store.find_by_pid(1234).expect("find pid");
        assert_eq!(hits.len(), 2, "both pid-1234 rows returned via idx:pid");
        assert!(hits.iter().all(|r| r.pid.raw() == 1234));
        assert_eq!(store.find_by_pid(9999).expect("find pid").len(), 1);
        assert!(store.find_by_pid(4242).expect("find miss").is_empty());
    }

    #[test]
    fn event_store_finds_records_by_name_with_collision_verify() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("idx-name.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;
        let base = hour.saturating_mul(500);

        // Two "nginx" rows and one "redis" row.
        store
            .put_event(base, 1, &ProcessRecord::new(1, "nginx".to_owned()))
            .expect("put 1");
        store
            .put_event(base, 2, &ProcessRecord::new(2, "nginx".to_owned()))
            .expect("put 2");
        store
            .put_event(base, 3, &ProcessRecord::new(3, "redis".to_owned()))
            .expect("put 3");

        // find_by_name returns ONLY exact-name matches (the primary-row verify
        // guards against hash collisions), and is case-insensitive on the term.
        let nginx = store.find_by_name("NGINX").expect("find nginx");
        assert_eq!(nginx.len(), 2, "both nginx rows returned");
        assert!(nginx.iter().all(|r| r.name == "nginx"));
        assert_eq!(store.find_by_name("redis").expect("find redis").len(), 1);
        assert!(
            store
                .find_by_name("absent")
                .expect("find absent")
                .is_empty()
        );
    }

    #[test]
    fn event_store_retention_drops_indexes_with_their_bucket() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("idx-retention.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;

        // Old event (hour 1) indexed by pid 7; recent event (hour 1000) pid 8.
        store
            .put_event(hour, 1, &ProcessRecord::new(7, "old".to_owned()))
            .expect("put old");
        store
            .put_event(
                hour.saturating_mul(1000),
                2,
                &ProcessRecord::new(8, "new".to_owned()),
            )
            .expect("put new");
        assert_eq!(store.find_by_pid(7).expect("find 7 before").len(), 1);

        let dropped = store.drop_expired(hour.saturating_mul(1001)).expect("drop");
        assert_eq!(dropped, 1);
        // The expired bucket's index is gone; the recent one survives.
        assert!(store.find_by_pid(7).expect("find 7 after").is_empty());
        assert_eq!(store.find_by_pid(8).expect("find 8 after").len(), 1);
    }

    #[test]
    fn event_store_scan_range_returns_window_ascending_across_buckets() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("scan.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;

        // hour 0: ts 1000, 2000; hour 1: ts hour+500; hour 5: ts 5*hour.
        store
            .put_event(1_000, 1, &ProcessRecord::new(1, "a".to_owned()))
            .expect("a");
        store
            .put_event(2_000, 2, &ProcessRecord::new(2, "b".to_owned()))
            .expect("b");
        store
            .put_event(
                hour.saturating_add(500),
                3,
                &ProcessRecord::new(3, "c".to_owned()),
            )
            .expect("c");
        store
            .put_event(
                hour.saturating_mul(5),
                4,
                &ProcessRecord::new(4, "d".to_owned()),
            )
            .expect("d");

        // Window [1500, hour+1000) spans hour 0 and hour 1: catches b and c, not a or d.
        let hits = store
            .scan_range(1_500, hour.saturating_add(1_000))
            .expect("scan");
        let names: Vec<&str> = hits.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["b", "c"],
            "in-window rows, ascending by (ts, seq)"
        );

        // Empty / inverted windows.
        assert!(store.scan_range(10, 10).expect("empty").is_empty());
        assert!(store.scan_range(5_000, 1_000).expect("inverted").is_empty());
    }

    #[test]
    fn event_store_pid_postings_returns_sorted_pointers() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("postings.redb");
        let store = EventStore::new(&db_path).expect("create event store");
        let hour = 3_600_000_u64;

        // pid 7 appears at (hour, 9) and (0, 1) — postings must come back sorted.
        store
            .put_event(hour, 9, &ProcessRecord::new(7, "p".to_owned()))
            .expect("p1");
        store
            .put_event(100, 1, &ProcessRecord::new(7, "p".to_owned()))
            .expect("p2");
        store
            .put_event(100, 2, &ProcessRecord::new(8, "q".to_owned()))
            .expect("q");

        let postings = store.pid_postings(7).expect("postings");
        assert_eq!(
            postings,
            vec![(100, 1), (hour, 9)],
            "ascending (ts, seq) pointers"
        );
        assert!(store.pid_postings(999).expect("miss").is_empty());
    }

    #[test]
    fn event_store_build_mrc_resolves_parent_lineage_in_window() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("mrc.redb");
        let store = EventStore::new(&db_path).expect("create event store");

        let now = 100_000_000_u64; // within one hourly bucket
        let mut parent = ProcessRecord::new(1, "init".to_owned());
        parent.ppid = None;
        let mut child = ProcessRecord::new(100, "bash".to_owned());
        child.ppid = Some(crate::models::process::ProcessId::new(1));
        store
            .put_event(now.saturating_sub(2_000), 1, &parent)
            .expect("parent");
        store
            .put_event(now.saturating_sub(1_000), 2, &child)
            .expect("child");

        let mrc = store.build_mrc(now).expect("build mrc");
        let bash = mrc.get(&100).expect("child in mrc");
        assert_eq!(bash.ppid, Some(1));
        assert_eq!(bash.parent_name.as_deref(), Some("init"));
    }

    // ---- Plain-table persistence (U9) -------------------------------------

    #[test]
    fn event_store_rules_persist_across_reopen() {
        // The headline "zero rules after restart" fix: a stored rule must survive
        // dropping and reopening the store.
        let temp_dir = tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("events.redb");

        let rule = DetectionRule::new(
            crate::models::RuleId::new("apache-bash-spawn"),
            "Apache spawns bash",
            "Detects a bash child of an apache process",
            "SELECT 1",
            "lateral-movement",
            AlertSeverity::High,
        );
        let writer = EventStore::new(&db_path).expect("create store");
        writer.store_rule(&rule).expect("store rule");
        // Visible immediately within the same handle.
        assert_eq!(writer.get_all_rules().expect("all rules").len(), 1);
        drop(writer);

        // Reopen a fresh handle: the rule is still there (the actual regression).
        let store = EventStore::open(&db_path).expect("reopen store");
        let loaded = store.get_all_rules().expect("all rules after reopen");
        assert_eq!(loaded.len(), 1, "rule did not survive restart");
        assert_eq!(
            loaded.first().expect("one rule").id.raw(),
            "apache-bash-spawn"
        );

        let by_id = store
            .get_rule("apache-bash-spawn")
            .expect("get rule")
            .expect("rule present");
        assert_eq!(by_id, rule);
        assert!(
            store.get_rule("missing").expect("get miss").is_none(),
            "absent rule must read as None"
        );
    }

    #[test]
    fn event_store_store_rule_upserts_by_id() {
        let temp_dir = tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("events.redb");
        let store = EventStore::new(&db_path).expect("create store");

        let v1 = DetectionRule::new(
            crate::models::RuleId::new("r1"),
            "first",
            "desc",
            "SELECT 1",
            "cat",
            AlertSeverity::Low,
        );
        let v2 = DetectionRule::new(
            crate::models::RuleId::new("r1"),
            "second",
            "desc",
            "SELECT 1",
            "cat",
            AlertSeverity::High,
        );
        store.store_rule(&v1).expect("store v1");
        store.store_rule(&v2).expect("store v2");

        // Same id → one row, latest wins.
        let all = store.get_all_rules().expect("all");
        assert_eq!(all.len(), 1);
        assert_eq!(all.first().expect("one rule").name, "second");
    }

    #[test]
    fn event_store_alerts_round_trip() {
        let temp_dir = tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("events.redb");
        let writer = EventStore::new(&db_path).expect("create store");

        let proc = ProcessRecord::new(99, "suspect".to_owned());
        let alert = Alert::new(
            AlertSeverity::Critical,
            "lateral movement",
            "apache -> bash",
            "apache-bash-spawn",
            proc,
        );
        writer.store_alert(&alert).expect("store alert");
        drop(writer);

        // Reopen a fresh handle to prove durability symmetric with rules/scans.
        let store = EventStore::open(&db_path).expect("reopen store");
        let fetched = store
            .get_alert(&alert.id.to_string())
            .expect("get alert")
            .expect("alert present");
        assert_eq!(fetched, alert);
        assert_eq!(store.get_all_alerts().expect("all alerts").len(), 1);
    }

    #[test]
    fn event_store_scans_persist_with_host_identity() {
        let temp_dir = tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("events.redb");

        let mut system = SystemInfo::new();
        system.hostname = "edge-01".to_owned();
        system.os_name = "linux".to_owned();
        system.os_version = "6.1".to_owned();
        system.architecture = "aarch64".to_owned();
        // Volatile stats are present on SystemInfo but must NOT be folded in.
        system.cpu_cores = 8;
        system.total_memory = 16_000_000_000;

        let scan = ScanMetadata::new(
            "scan-001",
            chrono::Utc::now(),
            42,
            1234,
            ScanStatus::Completed,
            &system,
        );
        let writer = EventStore::new(&db_path).expect("create store");
        writer.store_scan(&scan).expect("store scan");
        drop(writer);

        let store = EventStore::open(&db_path).expect("reopen store");
        let loaded = store
            .get_scan("scan-001")
            .expect("get scan")
            .expect("scan present");
        // Host identity folded in.
        assert_eq!(loaded.hostname, "edge-01");
        assert_eq!(loaded.os_name, "linux");
        assert_eq!(loaded.os_version, "6.1");
        assert_eq!(loaded.architecture, "aarch64");
        // The scan carries identity, not the volatile stats (no such fields exist).
        assert_eq!(loaded.process_count, 42);
        assert_eq!(store.get_all_scans().expect("all scans").len(), 1);
    }

    #[test]
    fn event_store_plain_tables_empty_on_fresh_store() {
        let temp_dir = tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("events.redb");
        let store = EventStore::new(&db_path).expect("create store");
        // Reads against never-created tables are empty, not errors.
        assert!(store.get_all_rules().expect("rules").is_empty());
        assert!(store.get_all_alerts().expect("alerts").is_empty());
        assert!(store.get_all_scans().expect("scans").is_empty());
        assert!(store.get_rule("x").expect("rule").is_none());
    }

    #[test]
    fn test_database_creation_invalid_path() {
        // Test with a path that doesn't exist
        let invalid_path = Path::new("/nonexistent/path/test.db");
        let result = DatabaseManager::new(invalid_path);
        // This might succeed or fail depending on the system, but shouldn't panic
        let _ = result;
    }

    #[test]
    fn test_scan_metadata_creation() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-1".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        assert_eq!(metadata.scan_id, "test-scan-1");
        assert_eq!(metadata.process_count, 100);
        assert_eq!(metadata.duration_ms, 5000);
        assert_eq!(metadata.status, ScanStatus::Completed);
        assert!(metadata.error_message.is_none());
    }

    #[test]
    fn test_scan_metadata_with_error() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-2".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 0,
            duration_ms: 1000,
            status: ScanStatus::Failed,
            error_message: Some("Test error".to_owned()),
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        assert_eq!(metadata.status, ScanStatus::Failed);
        assert_eq!(metadata.error_message, Some("Test error".to_owned()));
    }

    #[test]
    fn test_scan_metadata_serialization() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-3".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 50,
            duration_ms: 2500,
            status: ScanStatus::InProgress,
            error_message: None,
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        let json = serde_json::to_string(&metadata).expect("Failed to serialize metadata");
        let deserialized: ScanMetadata =
            serde_json::from_str(&json).expect("Failed to deserialize metadata");

        assert_eq!(metadata.scan_id, deserialized.scan_id);
        assert_eq!(metadata.process_count, deserialized.process_count);
        assert_eq!(metadata.duration_ms, deserialized.duration_ms);
        assert_eq!(metadata.status, deserialized.status);
    }

    #[test]
    fn test_scan_status_variants() {
        let statuses = vec![
            ScanStatus::InProgress,
            ScanStatus::Completed,
            ScanStatus::Failed,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).expect("Failed to serialize status");
            let deserialized: ScanStatus =
                serde_json::from_str(&json).expect("Failed to deserialize status");
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_database_stats_default() {
        let stats = DatabaseStats::default();
        assert_eq!(stats.processes, 0);
        assert_eq!(stats.rules, 0);
        assert_eq!(stats.alerts, 0);
        assert_eq!(stats.system_info, 0);
        assert_eq!(stats.scans, 0);
    }

    #[test]
    fn test_database_stats_creation() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
        };

        assert_eq!(stats.processes, 100);
        assert_eq!(stats.rules, 10);
        assert_eq!(stats.alerts, 5);
        assert_eq!(stats.system_info, 1);
        assert_eq!(stats.scans, 50);
    }

    #[test]
    fn test_database_stats_serialization() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
        };

        let json = serde_json::to_string(&stats).expect("Failed to serialize stats");
        let deserialized: DatabaseStats =
            serde_json::from_str(&json).expect("Failed to deserialize stats");
        assert_eq!(stats, deserialized);
    }

    #[test]
    fn test_storage_error_display() {
        let errors = vec![
            StorageError::TableNotFound {
                table: "test".to_owned(),
            },
            StorageError::RecordNotFound {
                id: "test-id".to_owned(),
            },
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_process_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());

        // Test that store_process doesn't panic (currently stubbed)
        manager
            .store_process(1, &process)
            .expect("Failed to store process");

        // Test that get_process returns None (currently stubbed)
        let retrieved = manager.get_process(1).expect("Failed to get process");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_rule_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        // Test that store_rule doesn't panic (currently stubbed)
        manager.store_rule(&rule).expect("Failed to store rule");

        // Test that get_rule returns None (currently stubbed)
        let retrieved = manager.get_rule("rule-1").expect("Failed to get rule");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_alert_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        // Test that store_alert doesn't panic (currently stubbed)
        manager
            .store_alert(1, &alert)
            .expect("Failed to store alert");

        // Test that get_alert returns None (currently stubbed)
        let retrieved = manager.get_alert(1).expect("Failed to get alert");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_get_all_alerts() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_alerts returns empty vector (currently stubbed)
        let alerts = manager.get_all_alerts().expect("Failed to get all alerts");
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_system_info_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let system_info = SystemInfo {
            hostname: "test-host".to_owned(),
            os_name: "TestOS".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
            cpu_cores: 4,
            total_memory: 8192,
            uptime: 3600,
            capabilities: vec!["test_capability".to_owned()],
        };

        // Test that store_system_info doesn't panic (currently stubbed)
        manager
            .store_system_info(1, &system_info)
            .expect("Failed to store system info");

        // Test that get_latest_system_info returns None (currently stubbed)
        let retrieved = manager
            .get_latest_system_info()
            .expect("Failed to get latest system info");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_database_stats() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());

        // Test that store_process doesn't panic (currently stubbed)
        manager
            .store_process(1, &process)
            .expect("Failed to store process");

        // Test that get_stats returns default values (currently stubbed)
        let stats = manager.get_stats().expect("Failed to get stats");
        assert_eq!(stats.processes, 0); // Currently stubbed to return 0
    }

    #[test]
    fn test_tables_constants() {
        // Test that table definitions are accessible
        let _ = Tables::PROCESSES;
        let _ = Tables::DETECTION_RULES;
        let _ = Tables::ALERTS;
        let _ = Tables::SYSTEM_INFO;
        let _ = Tables::SCAN_METADATA;
    }

    #[test]
    fn test_database_manager_open() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a database first
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");
        drop(manager); // Close the database so we can open it again

        // Now open the existing database
        let open_manager = DatabaseManager::open(&db_path).expect("Failed to open database");
        assert!(db_path.exists());

        // Test that we can call methods on the opened database
        let stats = open_manager.get_stats().expect("Failed to get stats");
        assert_eq!(stats.processes, 0);
    }

    #[test]
    fn test_batch_process_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let processes = vec![
            (1, ProcessRecord::new(1234, "process1".to_owned())),
            (2, ProcessRecord::new(5678, "process2".to_owned())),
            (3, ProcessRecord::new(9012, "process3".to_owned())),
        ];

        // Test that store_processes_batch doesn't panic (currently stubbed)
        manager
            .store_processes_batch(&processes)
            .expect("Failed to store processes batch");
    }

    #[test]
    fn test_get_all_processes() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_processes returns empty vector (currently stubbed)
        let processes = manager
            .get_all_processes()
            .expect("Failed to get all processes");
        assert!(processes.is_empty());
    }

    #[test]
    fn test_get_all_rules() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_rules returns empty vector (currently stubbed)
        let rules = manager.get_all_rules().expect("Failed to get all rules");
        assert!(rules.is_empty());
    }

    #[test]
    fn test_scan_metadata_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let metadata = ScanMetadata {
            scan_id: "test-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        // Test that store_scan_metadata doesn't panic (currently stubbed)
        manager
            .store_scan_metadata(1, &metadata)
            .expect("Failed to store scan metadata");

        // Test that get_scan_metadata returns None (currently stubbed)
        let retrieved = manager
            .get_scan_metadata(1)
            .expect("Failed to get scan metadata");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_all_scan_metadata() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_scan_metadata returns empty vector (currently stubbed)
        let metadata = manager
            .get_all_scan_metadata()
            .expect("Failed to get scan metadata");
        assert!(metadata.is_empty());
    }

    #[test]
    fn test_cleanup_old_data() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that cleanup_old_data returns 0 (currently stubbed)
        let cleaned = manager
            .cleanup_old_data(30)
            .expect("Failed to cleanup old data");
        assert_eq!(cleaned, 0);
    }

    #[test]
    fn test_storage_error_friendly_variants() {
        // Test the new friendly error variants
        let missing_dir_error = StorageError::MissingDirectory {
            dir: "/tmp/missing".into(),
            source: Some(std::io::Error::other("test io error")),
        };
        let missing_dir_msg = format!("{missing_dir_error}");
        assert!(missing_dir_msg.contains("Database directory"));
        assert!(missing_dir_msg.contains("does not exist"));
        assert!(missing_dir_msg.contains("/tmp/missing"));

        let not_dir_error = StorageError::NotADirectory {
            path: "/tmp/file".into(),
        };
        let not_dir_msg = format!("{not_dir_error}");
        assert!(not_dir_msg.contains("is not a directory"));
        assert!(not_dir_msg.contains("/tmp/file"));
    }

    #[test]
    fn test_storage_error_from_serialization_error() {
        // Test that StorageError can be created from serde_json::Error
        let json_error = serde_json::from_str::<serde_json::Value>("invalid json")
            .expect_err("Expected JSON error");
        let storage_error = StorageError::from(json_error);
        assert!(format!("{storage_error}").contains("Serialization error"));
    }

    #[test]
    fn test_scan_metadata_with_all_fields() {
        let metadata = ScanMetadata {
            scan_id: "comprehensive-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 1000,
            duration_ms: 10000,
            status: ScanStatus::InProgress,
            error_message: Some("Test error message".to_owned()),
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        assert_eq!(metadata.scan_id, "comprehensive-scan");
        assert_eq!(metadata.process_count, 1000);
        assert_eq!(metadata.duration_ms, 10000);
        assert_eq!(metadata.status, ScanStatus::InProgress);
        assert_eq!(
            metadata.error_message,
            Some("Test error message".to_owned())
        );
    }

    #[test]
    fn test_database_stats_with_all_fields() {
        let stats = DatabaseStats {
            processes: 1000,
            rules: 50,
            alerts: 25,
            system_info: 5,
            scans: 100,
        };

        assert_eq!(stats.processes, 1000);
        assert_eq!(stats.rules, 50);
        assert_eq!(stats.alerts, 25);
        assert_eq!(stats.system_info, 5);
        assert_eq!(stats.scans, 100);
    }

    #[test]
    fn test_database_stats_clone() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
        };

        let cloned_stats = stats.clone();
        assert_eq!(stats, cloned_stats);
    }

    #[test]
    fn test_scan_metadata_clone() {
        let metadata = ScanMetadata {
            scan_id: "test-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
            hostname: "test-host".to_owned(),
            os_name: "linux".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
        };

        let cloned_metadata = metadata.clone();
        assert_eq!(metadata, cloned_metadata);
    }

    #[test]
    fn test_scan_status_clone() {
        let status = ScanStatus::InProgress;
        let cloned_status = status.clone();
        assert_eq!(status, cloned_status);
    }
}

// TODO: Implement redb Value trait implementations in Task 8
// For now, just focus on getting the basic structure compiling for Task 1
