//! Typed persistence for the event store's plain (non-bucketed) tables (T3 · U9).
//!
//! Detection rules, alerts, and scan metadata are small, directly-keyed tables —
//! unlike the time-bucketed process events ([`super::bucket`]). Their values reuse
//! the same version-tagged postcard envelope as the event codec
//! ([`super::codec::encode_value`]) so every table lives under one
//! `schema_version` story (U8): a future format change bumps the schema version
//! and the whole store is exported and rebuilt, never silently misread.
//!
//! Each table is keyed by the record's own stable string id (rule id, alert uuid,
//! scan id), so writes are idempotent upserts and reads are direct point lookups.
//! The functions take a `&Database` and own a single transaction each; the
//! [`super::EventStore`] facade methods delegate here.

use crate::models::{Alert, DetectionRule};
use crate::storage::ScanMetadata;
use crate::storage::codec::{decode_value, encode_value};
use crate::storage::error::StorageError;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

/// `detection_rules` — rule id → versioned `DetectionRule`. Persisting and
/// reloading this table is the fix for the origin "zero rules after restart" bug.
const RULES_TABLE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("detection_rules");

/// `alerts` — alert uuid (string) → versioned `Alert`.
const ALERTS_TABLE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("alerts");

/// `scans` — scan id → versioned `ScanMetadata` (now carrying host identity, U9).
const SCANS_TABLE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("scans");

/// Upsert a record into a plain string-keyed table within one write transaction.
fn put_record<T: serde::Serialize>(
    db: &Database,
    table: TableDefinition<'static, &str, &[u8]>,
    key: &str,
    value: &T,
) -> Result<(), StorageError> {
    let bytes = encode_value(value)?;
    let wtxn = db.begin_write()?;
    let mut handle = wtxn.open_table(table)?;
    handle.insert(key, bytes.as_slice())?;
    drop(handle);
    wtxn.commit()?;
    Ok(())
}

/// Point-lookup a record by key, decoding the version-tagged value. A
/// not-yet-created table reads as "absent", not an error.
fn get_record<T: serde::de::DeserializeOwned>(
    db: &Database,
    table: TableDefinition<'static, &str, &[u8]>,
    key: &str,
) -> Result<Option<T>, StorageError> {
    let rtxn = db.begin_read()?;
    let handle = match rtxn.open_table(table) {
        Ok(handle) => handle,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    match handle.get(key)? {
        Some(guard) => Ok(Some(decode_value(guard.value())?)),
        None => Ok(None),
    }
}

/// Collect every record in a plain table, decoding each value. A not-yet-created
/// table yields an empty vector.
fn all_records<T: serde::de::DeserializeOwned>(
    db: &Database,
    table: TableDefinition<'static, &str, &[u8]>,
) -> Result<Vec<T>, StorageError> {
    let rtxn = db.begin_read()?;
    let handle = match rtxn.open_table(table) {
        Ok(handle) => handle,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut out = Vec::new();
    for entry in handle.iter()? {
        let (_key, value) = entry?;
        out.push(decode_value(value.value())?);
    }
    Ok(out)
}

/// Persist (upsert) a detection rule keyed by its id.
pub(super) fn store_rule(db: &Database, rule: &DetectionRule) -> Result<(), StorageError> {
    put_record(db, RULES_TABLE, rule.id.raw(), rule)
}

/// Fetch a detection rule by id.
pub(super) fn get_rule(db: &Database, id: &str) -> Result<Option<DetectionRule>, StorageError> {
    get_record(db, RULES_TABLE, id)
}

/// Fetch every persisted detection rule.
pub(super) fn all_rules(db: &Database) -> Result<Vec<DetectionRule>, StorageError> {
    all_records(db, RULES_TABLE)
}

/// Persist (upsert) an alert keyed by its uuid.
pub(super) fn store_alert(db: &Database, alert: &Alert) -> Result<(), StorageError> {
    put_record(db, ALERTS_TABLE, &alert.id.to_string(), alert)
}

/// Fetch an alert by its uuid string.
pub(super) fn get_alert(db: &Database, id: &str) -> Result<Option<Alert>, StorageError> {
    get_record(db, ALERTS_TABLE, id)
}

/// Fetch every persisted alert.
pub(super) fn all_alerts(db: &Database) -> Result<Vec<Alert>, StorageError> {
    all_records(db, ALERTS_TABLE)
}

/// Persist (upsert) scan metadata keyed by its scan id.
pub(super) fn store_scan(db: &Database, scan: &ScanMetadata) -> Result<(), StorageError> {
    put_record(db, SCANS_TABLE, &scan.scan_id, scan)
}

/// Fetch scan metadata by scan id.
pub(super) fn get_scan(db: &Database, id: &str) -> Result<Option<ScanMetadata>, StorageError> {
    get_record(db, SCANS_TABLE, id)
}

/// Fetch every persisted scan metadata record.
pub(super) fn all_scans(db: &Database) -> Result<Vec<ScanMetadata>, StorageError> {
    all_records(db, SCANS_TABLE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use redb::Database;

    /// A single undecodable row must fail the read loudly — never silently drop
    /// the row (which would resurface the "zero rules" symptom) and never panic.
    #[test]
    fn reads_surface_a_corrupt_row_as_error_not_silent_drop() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("events.redb")).unwrap();

        // Write a value carrying an unknown codec version byte directly.
        let wtxn = db.begin_write().unwrap();
        let mut table = wtxn.open_table(RULES_TABLE).unwrap();
        table.insert("k", [0xFF_u8, 9, 9, 9].as_slice()).unwrap();
        drop(table);
        wtxn.commit().unwrap();

        assert!(
            all_rules(&db).is_err(),
            "get_all must fail on an undecodable row, not skip it"
        );
        assert!(
            get_rule(&db, "k").is_err(),
            "point lookup must surface the decode error"
        );
    }
}
