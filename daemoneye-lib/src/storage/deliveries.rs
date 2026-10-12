//! `alert_deliveries` outbox operations (R9, R12, R13, R14, R16, R17, R25).
//!
//! Every write-back is a compare-and-set on `(state, attempts, redrives)` so a worker holding an
//! old read cannot overwrite a redrive or another worker's claim. Storage never reads the clock:
//! callers pass `now_ms`. Rows are scanned per sink by key prefix; an undecodable row is counted
//! and skipped, never fatal to its neighbours.

use crate::models::{Alert, AlertSeverity, DeliveryReason, DeliveryRecord, DeliveryState, Uuid};
use crate::storage::codec::{decode_value, encode_value};
use crate::storage::error::StorageError;
use crate::storage::records::{
    ALERT_DELIVERIES_TABLE, ALERTS_TABLE, DELIVERY_KEY_SEP, delivery_key, get_record,
};
use redb::{Database, ReadableDatabase, ReadableTable, Table};
use std::collections::BTreeMap;

/// Exclusive upper bound of one sink's key prefix: the character after [`DELIVERY_KEY_SEP`].
const DELIVERY_KEY_SEP_END: char = '\u{20}';

/// `(state, attempts, redrives)` as the caller last read them.
pub type DeliveryExpectation = (DeliveryState, u32, u32);

/// Result of [`EventStore::enqueue_alert`](crate::storage::EventStore::enqueue_alert).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnqueueReport {
    /// A row for this alert already exists on some sink; nothing was written.
    AlreadyEnqueued,
    /// The alert and one row per sink were written.
    Enqueued {
        /// Per sink: the new row's state and the alert it displaced from the open queue, if any.
        rows: Vec<(String, DeliveryState, Option<Uuid>)>,
    },
}

/// Non-terminal rows of one sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDeliveries {
    /// Decoded rows, oldest enqueue first.
    pub rows: Vec<(Uuid, DeliveryRecord)>,
    /// Rows that could not be decoded and were skipped.
    pub corrupt: usize,
}

/// A row claimed for an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The stored `(state, attempts, redrives)` after the claim; hand it back to `complete`.
    pub expected: DeliveryExpectation,
    /// The stored row after the claim.
    pub record: DeliveryRecord,
}

/// Result of a compare-and-set write-back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompleteOutcome {
    /// The expectation held and the row was replaced.
    Written,
    /// The row changed or vanished; nothing was written.
    Stale,
}

type Rows<'txn> = Table<'txn, &'static str, &'static [u8]>;
type ScannedRow = (String, Option<DeliveryRecord>);

fn alert_id_of(key: &str) -> Option<Uuid> {
    key.rsplit_once(DELIVERY_KEY_SEP)
        .and_then(|(_sink, id)| Uuid::parse_str(id).ok())
}

fn put(table: &mut Rows<'_>, key: &str, record: &DeliveryRecord) -> Result<(), StorageError> {
    let bytes = encode_value(record)?;
    table.insert(key, bytes.as_slice())?;
    Ok(())
}

fn get_row<T: ReadableTable<&'static str, &'static [u8]>>(
    table: &T,
    key: &str,
) -> Result<Option<DeliveryRecord>, StorageError> {
    match table.get(key)? {
        Some(guard) => Ok(Some(decode_value(guard.value())?)),
        None => Ok(None),
    }
}

fn decode_scanned(key: &str, bytes: &[u8]) -> Option<DeliveryRecord> {
    match decode_value(bytes) {
        Ok(record) => Some(record),
        Err(error) => {
            tracing::warn!(%key, %error, "undecodable alert_deliveries row");
            None
        }
    }
}

/// Every row of one sink (or of all sinks); `None` marks an undecodable row.
fn scan<T: ReadableTable<&'static str, &'static [u8]>>(
    table: &T,
    sink: Option<&str>,
) -> Result<Vec<ScannedRow>, StorageError> {
    let mut out = Vec::new();
    if let Some(name) = sink {
        let start = format!("{name}{DELIVERY_KEY_SEP}");
        let end = format!("{name}{DELIVERY_KEY_SEP_END}");
        for entry in table.range(start.as_str()..end.as_str())? {
            let (key, value) = entry?;
            out.push((
                key.value().to_owned(),
                decode_scanned(key.value(), value.value()),
            ));
        }
    } else {
        for entry in table.iter()? {
            let (key, value) = entry?;
            out.push((
                key.value().to_owned(),
                decode_scanned(key.value(), value.value()),
            ));
        }
    }
    Ok(out)
}

fn open_rows(rows: &[ScannedRow]) -> usize {
    rows.iter()
        .filter(|row| {
            row.1
                .as_ref()
                .is_some_and(|record| !record.state.is_terminal())
        })
        .count()
}

fn dead_lettered(rows: Vec<ScannedRow>) -> Vec<(String, DeliveryRecord)> {
    rows.into_iter()
        .filter_map(|(key, record)| record.map(|row| (key, row)))
        .filter(|entry| entry.1.state == DeliveryState::DeadLettered)
        .collect()
}

const fn dead_letter(record: &mut DeliveryRecord, reason: DeliveryReason, now_ms: u64) {
    record.state = DeliveryState::DeadLettered;
    record.reason = Some(reason);
    record.terminal_at_ms = Some(now_ms);
}

fn new_record(alert: &Alert, now_ms: u64) -> DeliveryRecord {
    DeliveryRecord {
        state: DeliveryState::Pending,
        reason: None,
        severity: alert.severity,
        attempts: 0,
        redrives: 0,
        last_error: None,
        enqueued_at_ms: now_ms,
        next_attempt_ms: now_ms,
        terminal_at_ms: None,
        suppressed_by: None,
        deduplication_key: alert.deduplication_key.clone(),
    }
}

/// Dead-letter the oldest lowest-severity `Pending` row strictly below `incoming`, if any.
fn displace(
    rows: &mut Rows<'_>,
    existing: &[ScannedRow],
    incoming: AlertSeverity,
    now_ms: u64,
) -> Result<Option<Uuid>, StorageError> {
    if incoming < AlertSeverity::High {
        return Ok(None);
    }
    let victim = existing
        .iter()
        .filter_map(|entry| entry.1.as_ref().map(|row| (&entry.0, row)))
        .filter(|&(_, row)| row.state == DeliveryState::Pending && row.severity < incoming)
        .min_by_key(|&(key, row)| (row.severity, row.enqueued_at_ms, key.as_str()));
    let Some((key, row)) = victim else {
        return Ok(None);
    };
    let mut displaced = row.clone();
    dead_letter(&mut displaced, DeliveryReason::QueueFull, now_ms);
    put(rows, key, &displaced)?;
    Ok(alert_id_of(key))
}

fn place(
    rows: &mut Rows<'_>,
    alert: &Alert,
    sink: &str,
    suppressed_by: Option<Uuid>,
    max_open_rows: usize,
    now_ms: u64,
) -> Result<(String, DeliveryState, Option<Uuid>), StorageError> {
    let mut record = new_record(alert, now_ms);
    let mut displaced = None;
    if let Some(survivor) = suppressed_by {
        record.state = DeliveryState::SuppressedDuplicate;
        record.suppressed_by = Some(survivor);
        record.terminal_at_ms = Some(now_ms);
    } else {
        let existing = scan(rows, Some(sink))?;
        if open_rows(&existing) >= max_open_rows {
            displaced = displace(rows, &existing, alert.severity, now_ms)?;
            if displaced.is_none() {
                dead_letter(&mut record, DeliveryReason::QueueFull, now_ms);
            }
        }
    }
    put(rows, &delivery_key(sink, alert.id), &record)?;
    Ok((sink.to_owned(), record.state, displaced))
}

pub(super) fn enqueue_alert(
    db: &Database,
    alert: &Alert,
    sinks: &[&str],
    suppressed_by: Option<Uuid>,
    max_open_rows_per_sink: usize,
    now_ms: u64,
) -> Result<EnqueueReport, StorageError> {
    let wtxn = db.begin_write()?;
    let placed = {
        let mut alerts = wtxn.open_table(ALERTS_TABLE)?;
        let mut rows = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
        for sink in sinks {
            if rows.get(delivery_key(sink, alert.id).as_str())?.is_some() {
                return Ok(EnqueueReport::AlreadyEnqueued);
            }
        }
        let bytes = encode_value(alert)?;
        alerts.insert(alert.id.to_string().as_str(), bytes.as_slice())?;
        let mut placed = Vec::with_capacity(sinks.len());
        for sink in sinks {
            placed.push(place(
                &mut rows,
                alert,
                sink,
                suppressed_by,
                max_open_rows_per_sink,
                now_ms,
            )?);
        }
        placed
    };
    wtxn.commit()?;
    Ok(EnqueueReport::Enqueued { rows: placed })
}

pub(super) fn get_delivery(
    db: &Database,
    sink: &str,
    alert_id: Uuid,
) -> Result<Option<DeliveryRecord>, StorageError> {
    get_record(db, ALERT_DELIVERIES_TABLE, &delivery_key(sink, alert_id))
}

/// Read every row of `sink` (or of all sinks); a store without the table has none.
fn read_rows(db: &Database, sink: Option<&str>) -> Result<Vec<ScannedRow>, StorageError> {
    let rtxn = db.begin_read()?;
    match rtxn.open_table(ALERT_DELIVERIES_TABLE) {
        Ok(table) => scan(&table, sink),
        Err(redb::TableError::TableDoesNotExist(_)) => Ok(Vec::new()),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn open_deliveries(db: &Database, sink: &str) -> Result<OpenDeliveries, StorageError> {
    let mut rows = Vec::new();
    let mut corrupt = 0_usize;
    for (key, record) in read_rows(db, Some(sink))? {
        match (alert_id_of(&key), record) {
            (Some(id), Some(row)) if !row.state.is_terminal() => rows.push((id, row)),
            (Some(_), Some(_)) => {}
            _ => corrupt = corrupt.saturating_add(1),
        }
    }
    rows.sort_by_key(|entry| (entry.1.enqueued_at_ms, entry.0));
    Ok(OpenDeliveries { rows, corrupt })
}

pub(super) fn dead_letters(
    db: &Database,
    sink: &str,
) -> Result<Vec<(Uuid, DeliveryRecord)>, StorageError> {
    let mut out: Vec<(Uuid, DeliveryRecord)> = dead_lettered(read_rows(db, Some(sink))?)
        .into_iter()
        .filter_map(|(key, row)| alert_id_of(&key).map(|id| (id, row)))
        .collect();
    out.sort_by_key(|entry| (entry.1.terminal_at_ms, entry.0));
    Ok(out)
}

pub(super) fn claim(
    db: &Database,
    sink: &str,
    alert_id: Uuid,
    now_ms: u64,
) -> Result<Claimed, StorageError> {
    let key = delivery_key(sink, alert_id);
    let stale = || StorageError::DeliveryStale { key: key.clone() };
    let wtxn = db.begin_write()?;
    let claimed = {
        let mut table = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
        let mut record = get_row(&table, &key)?.ok_or_else(stale)?;
        if !matches!(
            record.state,
            DeliveryState::Pending | DeliveryState::Retrying
        ) {
            return Err(stale());
        }
        record.state = DeliveryState::Retrying;
        record.attempts = record.attempts.saturating_add(1);
        record.next_attempt_ms = now_ms;
        put(&mut table, &key, &record)?;
        Claimed {
            expected: (record.state, record.attempts, record.redrives),
            record,
        }
    };
    wtxn.commit()?;
    Ok(claimed)
}

pub(super) fn complete(
    db: &Database,
    sink: &str,
    alert_id: Uuid,
    expected: DeliveryExpectation,
    new: &DeliveryRecord,
) -> Result<CompleteOutcome, StorageError> {
    let key = delivery_key(sink, alert_id);
    let wtxn = db.begin_write()?;
    let mut table = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
    let holds = get_row(&table, &key)?
        .is_some_and(|row| (row.state, row.attempts, row.redrives) == expected);
    if !holds {
        return Ok(CompleteOutcome::Stale);
    }
    put(&mut table, &key, new)?;
    drop(table);
    wtxn.commit()?;
    Ok(CompleteOutcome::Written)
}

pub(super) fn redrive_batch(
    db: &Database,
    sink: &str,
    limit: usize,
    max_redrives: u32,
    now_ms: u64,
) -> Result<usize, StorageError> {
    let wtxn = db.begin_write()?;
    let mut moved = 0_usize;
    {
        let mut table = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
        let mut candidates = dead_lettered(scan(&table, Some(sink))?);
        candidates.sort_by(|a, b| (a.1.terminal_at_ms, &a.0).cmp(&(b.1.terminal_at_ms, &b.0)));
        for (key, mut row) in candidates {
            if moved >= limit {
                break;
            }
            if row.redrives >= max_redrives {
                if row.reason != Some(DeliveryReason::RedrivesExhausted) {
                    row.reason = Some(DeliveryReason::RedrivesExhausted);
                    put(&mut table, &key, &row)?;
                }
                continue;
            }
            row.state = DeliveryState::Pending;
            row.reason = None;
            row.attempts = 0;
            row.redrives = row.redrives.saturating_add(1);
            row.next_attempt_ms = now_ms;
            row.terminal_at_ms = None;
            put(&mut table, &key, &row)?;
            moved = moved.saturating_add(1);
        }
    }
    wtxn.commit()?;
    Ok(moved)
}

pub(super) fn evict_dead_letters(
    db: &Database,
    sink: &str,
    keep: usize,
) -> Result<usize, StorageError> {
    let wtxn = db.begin_write()?;
    let mut removed = 0_usize;
    {
        let mut table = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
        let mut dead = dead_lettered(scan(&table, Some(sink))?);
        dead.sort_by(|a, b| {
            (a.1.severity, a.1.terminal_at_ms, &a.0).cmp(&(b.1.severity, b.1.terminal_at_ms, &b.0))
        });
        let excess = dead.len().saturating_sub(keep);
        for (key, _) in dead.into_iter().take(excess) {
            table.remove(key.as_str())?;
            removed = removed.saturating_add(1);
        }
    }
    wtxn.commit()?;
    Ok(removed)
}

pub(super) fn prune_deliveries(
    db: &Database,
    now_ms: u64,
    retention_ms: u64,
) -> Result<usize, StorageError> {
    let cutoff = now_ms.saturating_sub(retention_ms);
    let wtxn = db.begin_write()?;
    let mut removed = 0_usize;
    {
        let mut table = wtxn.open_table(ALERT_DELIVERIES_TABLE)?;
        for (key, record) in scan(&table, None)? {
            let expired = record.is_some_and(|row| {
                row.state.is_terminal() && row.terminal_at_ms.is_some_and(|at| at < cutoff)
            });
            if expired {
                table.remove(key.as_str())?;
                removed = removed.saturating_add(1);
            }
        }
    }
    wtxn.commit()?;
    Ok(removed)
}

pub(super) fn seed_dedup_window(
    db: &Database,
    now_ms: u64,
    window_ms: u64,
) -> Result<Vec<(String, u64, Uuid)>, StorageError> {
    let cutoff = now_ms.saturating_sub(window_ms);
    // alert id -> (dedup key, enqueued_at_ms, has a row that was not suppressed)
    let mut alerts: BTreeMap<Uuid, (String, u64, bool)> = BTreeMap::new();
    for (key, record) in read_rows(db, None)? {
        let (Some(id), Some(row)) = (alert_id_of(&key), record) else {
            continue;
        };
        if row.enqueued_at_ms < cutoff {
            continue;
        }
        let live = row.state != DeliveryState::SuppressedDuplicate;
        let entry = alerts
            .entry(id)
            .or_insert_with(|| (row.deduplication_key.clone(), row.enqueued_at_ms, false));
        entry.2 |= live;
    }
    let mut newest: BTreeMap<String, (u64, Uuid)> = BTreeMap::new();
    for (id, (dedup_key, enqueued_at_ms, live)) in alerts {
        if live {
            let slot = newest.entry(dedup_key).or_insert((enqueued_at_ms, id));
            if enqueued_at_ms > slot.0 {
                *slot = (enqueued_at_ms, id);
            }
        }
    }
    Ok(newest
        .into_iter()
        .map(|(key, (at_ms, id))| (key, at_ms, id))
        .collect())
}
