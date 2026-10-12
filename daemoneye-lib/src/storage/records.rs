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

use crate::models::{Alert, DetectionRule, Uuid};
use crate::storage::ScanMetadata;
use crate::storage::codec::{decode_value, encode_value};
use crate::storage::error::StorageError;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

/// `detection_rules` — rule id → versioned `DetectionRule`. Persisting and
/// reloading this table is the fix for the origin "zero rules after restart" bug.
const RULES_TABLE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("detection_rules");

/// `alerts` — alert uuid (string) → versioned `Alert`.
pub(super) const ALERTS_TABLE: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("alerts");

/// `alert_deliveries` — `"{sink}\u{1f}{alert_id}"` → versioned `DeliveryRecord`. Agent-owned.
pub(super) const ALERT_DELIVERIES_TABLE: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("alert_deliveries");

/// Separator between sink and alert id in a delivery key.
pub(super) const DELIVERY_KEY_SEP: char = '\u{1f}';

/// The `alert_deliveries` key for one sink and alert.
pub(super) fn delivery_key(sink: &str, alert_id: Uuid) -> String {
    format!("{sink}{DELIVERY_KEY_SEP}{alert_id}")
}

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
pub(super) fn get_record<T: serde::de::DeserializeOwned>(
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

    fn golden_record() -> crate::models::DeliveryRecord {
        use crate::models::{AlertSeverity, DeliveryReason, DeliveryState};
        crate::models::DeliveryRecord {
            state: DeliveryState::DeadLettered,
            reason: Some(DeliveryReason::Permanent),
            severity: AlertSeverity::High,
            attempts: 3,
            redrives: 1,
            last_error: Some("http_503".to_owned()),
            enqueued_at_ms: 1000,
            next_attempt_ms: 2000,
            terminal_at_ms: Some(3000),
            suppressed_by: Some(Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10)),
            deduplication_key: "k".to_owned(),
        }
    }

    /// Pins the V1 wire format: a later field needs a codec V2 decoder, and these bytes keep decoding.
    const GOLDEN_V1: [u8; 44] = [
        0x01, 0x03, 0x01, 0x01, 0x02, 0x03, 0x01, 0x01, 0x08, 0x68, 0x74, 0x74, 0x70, 0x5f, 0x35,
        0x30, 0x33, 0xe8, 0x07, 0xd0, 0x0f, 0x01, 0xb8, 0x17, 0x01, 0x10, 0x01, 0x02, 0x03, 0x04,
        0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x01, 0x6b,
    ];

    #[test]
    fn delivery_record_v1_encoding_is_pinned() {
        let encoded = encode_value(&golden_record()).unwrap();
        assert_eq!(encoded, GOLDEN_V1);

        let decoded: crate::models::DeliveryRecord = decode_value(&GOLDEN_V1).unwrap();
        assert_eq!(decoded, golden_record());
    }
}
