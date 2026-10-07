//! The agent's storage half of a detection cycle (T6 · U12): ingest what was collected, load the
//! rules that were persisted, persist the alerts that were delivered.
//!
//! These are free functions over their dependencies so tests build them over a `tempfile` store.
//!
//! # `source_seq`
//!
//! A cycle's rows carry `source_seq = (cycle_ordinal << 32) | row_index`. The ingest watermark
//! treats the first row of the next ordinal as contiguous with the last row of this one, so a
//! cycle boundary is not a gap, while a skipped ordinal or a skipped row is. The ordinal is not
//! stored on its own: it is the high half of the watermark the event store commits with the rows,
//! so it can never drift from the data it protects.

use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::models::{Alert, ProcessRecord};
use daemoneye_lib::storage::ingest::{IngestError, IngestHandle, IngestRecord, SequenceGap};
use daemoneye_lib::storage::{EventStore, StorageError};
use std::path::Path;
use tracing::warn;

/// The collector id the agent files procmond's rows under.
pub const PROCMOND_COLLECTOR_ID: &str = "procmond";

/// Bits of `source_seq` below the cycle ordinal.
const ORDINAL_SHIFT: u32 = 32;

/// What one cycle's ingest did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    /// The largest `collection_time` (ms) submitted; `0` when nothing was.
    pub high_water_ms: u64,
    /// How many rows were submitted (before the watermark discarded any).
    pub submitted: usize,
    /// Sequence gaps the writer saw since the previous flush.
    pub gaps: Vec<SequenceGap>,
}

/// Why a cycle's rows could not be ingested.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CycleIngestError {
    /// The pipeline refused a row or failed the flush barrier.
    #[error("ingest failed: {0}")]
    Ingest(#[from] IngestError),
    /// More rows than a cycle's 32-bit row index can number. Wrapping would alias the next
    /// ordinal's sequence space and get fresh rows discarded as duplicates, so it is refused.
    #[error("cycle has more rows than a 32-bit row index can number")]
    TooManyRows,
    /// The cycle ordinal space is exhausted.
    #[error("cycle ordinal space exhausted")]
    OrdinalExhausted,
    /// The stored watermark could not be read.
    #[error("reading the ingest watermark failed: {0}")]
    Storage(#[from] StorageError),
}

fn source_seq(cycle_ordinal: u32, row_index: u32) -> u64 {
    (u64::from(cycle_ordinal) << ORDINAL_SHIFT) | u64::from(row_index)
}

/// Submit one cycle's rows and wait for them to commit.
///
/// Returns only after the group commit, so a caller that evaluates rules next never races the
/// writer. The key time of each row is its own `collection_time`; see `IngestRecord`.
///
/// # Errors
///
/// [`CycleIngestError`] if a row cannot be keyed or numbered, the pipeline is closed, or a commit
/// failed.
pub async fn ingest_cycle(
    handle: &IngestHandle,
    collector_id: &str,
    cycle_ordinal: u32,
    records: &[ProcessRecord],
) -> Result<IngestOutcome, CycleIngestError> {
    let mut high_water_ms = 0_u64;
    for (index, record) in records.iter().enumerate() {
        let row_index = u32::try_from(index).map_err(|_overflow| CycleIngestError::TooManyRows)?;
        let row = IngestRecord::new(
            collector_id,
            source_seq(cycle_ordinal, row_index),
            row_index,
            record.clone(),
        )?;
        high_water_ms = high_water_ms.max(row.ts_ms());
        handle.submit(row).await?;
    }
    let report = handle.flush().await?;
    Ok(IngestOutcome {
        high_water_ms,
        submitted: records.len(),
        gaps: report.gaps,
    })
}

/// The ordinal the next cycle should use: one above the last one committed for `collector_id`,
/// or `0` for a store that has none.
///
/// # Errors
///
/// [`CycleIngestError::OrdinalExhausted`] at the last ordinal, [`CycleIngestError::Storage`] if
/// the watermark cannot be read.
pub fn next_cycle_ordinal(store: &EventStore, collector_id: &str) -> Result<u32, CycleIngestError> {
    let marks = store.ingest_watermarks()?;
    let Some(mark) = marks.get(collector_id) else {
        return Ok(0);
    };
    let committed = u32::try_from(mark.checked_shr(ORDINAL_SHIFT).unwrap_or(0))
        .map_err(|_overflow| CycleIngestError::OrdinalExhausted)?;
    committed
        .checked_add(1)
        .ok_or(CycleIngestError::OrdinalExhausted)
}

/// Load every persisted rule into `engine`, logging each one the engine rejects with its id.
/// Returns how many loaded.
///
/// # Errors
///
/// [`StorageError`] if the persisted rules cannot be read.
pub fn load_persisted_rules(
    store: &EventStore,
    engine: &mut DetectionEngine,
) -> Result<usize, StorageError> {
    let mut loaded = 0_usize;
    for rule in store.get_all_rules()? {
        let rule_id = rule.id.raw().to_owned();
        match engine.load_rule(rule) {
            Ok(()) => loaded = loaded.saturating_add(1),
            Err(error) => {
                warn!(rule_id = %rule_id, error = %error, "persisted rule rejected at load")
            }
        }
    }
    Ok(loaded)
}

/// Persist every alert.
///
/// The completeness marker is then readable later without a second write path. A failure to store one alert is logged with its id and does not stop the rest. Returns how many
/// were stored.
pub fn persist_alerts(store: &EventStore, alerts: &[Alert]) -> usize {
    let mut stored = 0_usize;
    for alert in alerts {
        match store.store_alert(alert) {
            Ok(()) => stored = stored.saturating_add(1),
            Err(error) => warn!(alert_id = %alert.id, error = %error, "persisting an alert failed"),
        }
    }
    stored
}

/// Open the event store, turning a schema mismatch into an error that says how to recover.
///
/// # Errors
///
/// Any [`StorageError`] opening the store; a schema mismatch names the rebuild path.
pub fn open_event_store(path: &Path) -> Result<EventStore, anyhow::Error> {
    EventStore::new(path).map_err(|error| {
        if let &StorageError::SchemaVersionMismatch { found, expected } = &error {
            return anyhow::anyhow!(
                "event store at {} has schema version {found}, this binary expects {expected}; \
                 rebuild it with the signed export and rebuild path (storage::schema::migrate), \
                 which requires a signed bundle",
                path.display()
            );
        }
        // Unwrapped: the storage errors already name the path, and the CLI test pins their text.
        anyhow::Error::new(error)
    })
}
