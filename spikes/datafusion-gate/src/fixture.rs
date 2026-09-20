//! U3 — deterministic fixture generator.
//!
//! Writes a real T3 event store holding at least 100,000 process events across
//! at least 24 hourly bucket partitions (R1, R2, R4). A fixed seed reproduces
//! the same store, so a rerun reproduces the recorded numbers.
//!
//! The tree is shaped so the `ShadowHunt` join has a known answer: `PLANTED`
//! shell children whose `ppid` points at a long-running service parent, plus
//! noise processes that must not match. Every pid appears exactly once, so the
//! self-join is 1:1 and the planted count is exact.
//!
//! Each record's `collection_time` is set equal to the `ts_ms` that assigns its
//! bucket. That is load-bearing: `ts_ms` lives only in the redb key, which is
//! not reachable from outside `daemoneye-lib`, so `collection_time` is the only
//! event time the provider (U5) can see.

use chrono::{DateTime, Utc};
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::storage::ingest::IngestRecord;
use daemoneye_lib::storage::{EventStore, StorageError};
use std::path::Path;

/// One hour in milliseconds; the event store's default bucket granularity.
pub const HOUR_MS: u64 = 3_600_000;

/// Service parent name the `ShadowHunt` rule looks for.
pub const SERVICE_NAME: &str = "httpd";

/// Shell child name the `ShadowHunt` rule looks for.
pub const SHELL_NAME: &str = "bash";

/// Rows written per `put_batch` call, to bound peak memory during generation.
const BATCH_ROWS: usize = 5_000;

/// Collector identity recorded on every generated row.
const COLLECTOR_ID: &str = "spike-fixture";

/// What the generator was asked to produce.
#[derive(Debug, Clone, Copy)]
pub struct FixtureSpec {
    /// Total event rows to write.
    pub rows: u64,
    /// Time span the rows are spread across, in hours.
    pub span_hours: u64,
    /// Number of planted service-to-shell matches.
    pub planted: u64,
    /// Epoch millisecond the span starts at.
    pub start_ms: u64,
}

/// Index distance between planted pairs, so matches reach nearly every
/// bucket rather than clustering at the start of the span. The tested bar is
/// at least 20 of 26 buckets, not all of them.
///
/// At least 2 so a pair's service and shell occupy distinct indices.
const fn plant_stride(spec: FixtureSpec) -> u64 {
    let divisor = if spec.planted == 0 { 1 } else { spec.planted };
    match spec.rows.checked_div(divisor) {
        Some(stride) if stride >= 2 => stride,
        _ => 2,
    }
}

impl FixtureSpec {
    /// One past the last timestamp the spec covers.
    ///
    /// Both arms bound their query with this, so it lives here rather than
    /// being recomputed in each binary where the two copies could drift.
    #[must_use]
    pub const fn end_ms(&self) -> u64 {
        self.start_ms
            .saturating_add(self.span_hours.saturating_mul(HOUR_MS))
    }
}

impl Default for FixtureSpec {
    fn default() -> Self {
        Self {
            rows: 120_000,
            span_hours: 26,
            planted: 500,
            // A fixed epoch start keeps bucket ids stable across runs.
            start_ms: 1_767_225_600_000,
        }
    }
}

/// What the generator actually wrote.
#[derive(Debug, Clone, Copy)]
pub struct FixtureStats {
    /// Rows written.
    pub rows: u64,
    /// Distinct bucket partitions the store reports.
    pub buckets: usize,
    /// Planted matches actually written, counted during generation rather than
    /// copied from the request. `plant_stride` clamps for an infeasible spec,
    /// so the two can differ and only the counted value is trustworthy.
    pub planted: u64,
    /// First event timestamp, inclusive.
    pub start_ms: u64,
    /// One past the last event timestamp.
    pub end_ms: u64,
}

/// Generate the fixture store at `path`.
///
/// Refuses a path that already holds events, so a stale fixture cannot silently
/// inflate a measurement.
///
/// # Errors
/// Returns [`StorageError`] when the store cannot be created or written, and
/// [`StorageError::Overflow`] when the spec's arithmetic would overflow.
pub fn generate(path: &Path, spec: FixtureSpec) -> Result<FixtureStats, StorageError> {
    let store = EventStore::new(path)?;
    let existing = store.event_count()?;
    if existing != 0 {
        return Err(StorageError::Overflow {
            context: format!("fixture path already holds {existing} events; refusing to append"),
        });
    }

    let span_ms = spec
        .span_hours
        .checked_mul(HOUR_MS)
        .ok_or_else(|| overflow("span_hours * HOUR_MS"))?;
    let step_ms = span_ms
        .checked_div(spec.rows.max(1))
        .ok_or_else(|| overflow("span_ms / rows"))?
        .max(1);

    let mut batch: Vec<IngestRecord> = Vec::with_capacity(BATCH_ROWS);
    let mut written: u64 = 0;
    let mut planted_written: u64 = 0;
    let mut last_ts = spec.start_ms;

    while written < spec.rows {
        let ts_ms = spec
            .start_ms
            .checked_add(
                written
                    .checked_mul(step_ms)
                    .ok_or_else(|| overflow("index * step"))?,
            )
            .ok_or_else(|| overflow("start + offset"))?;
        last_ts = ts_ms;

        let record = row_for(written, spec, ts_ms)?;
        // Count the shell half of each planted pair as it is actually written.
        if record.name == SHELL_NAME && record.ppid.is_some_and(|p| p.raw() != NOISE_PARENT_PID) {
            planted_written = planted_written.saturating_add(1);
        }
        batch.push(IngestRecord {
            collector_id: COLLECTOR_ID.to_owned(),
            source_seq: written,
            ts_ms,
            seq: 0,
            record,
        });

        written = written
            .checked_add(1)
            .ok_or_else(|| overflow("written + 1"))?;

        if batch.len() >= BATCH_ROWS || written == spec.rows {
            store.put_batch(&batch)?;
            batch.clear();
        }
    }

    let buckets = store.list_buckets()?.len();
    Ok(FixtureStats {
        rows: written,
        buckets,
        planted: planted_written,
        start_ms: spec.start_ms,
        end_ms: last_ts
            .checked_add(1)
            .ok_or_else(|| overflow("last_ts + 1"))?,
    })
}

/// Pid reserved for noise parents. Never emitted as a row, so a noise child can
/// never resolve to a service and inflate the match count.
const NOISE_PARENT_PID: u32 = 1;

/// Build the record for row `index`.
///
/// Pids are unique and the planted matches are exact. A service/shell pair is
/// planted every `rows / planted` indices, so matches are distributed across
/// nearly every bucket in the span rather than clustered at its start. Every other row
/// is noise whose parent is [`NOISE_PARENT_PID`], which is never emitted.
fn row_for(index: u64, spec: FixtureSpec, ts_ms: u64) -> Result<ProcessRecord, StorageError> {
    // SAFETY: pid space is u32; the fixture never generates more than u32::MAX
    // rows, and the offset keeps pids away from the low reserved range.
    #[expect(
        clippy::as_conversions,
        reason = "pid is u32 by domain; row counts are bounded well below u32::MAX"
    )]
    let pid = (index.wrapping_add(1000) & u64::from(u32::MAX)) as u32;

    let stride = plant_stride(spec);
    let slot = index.checked_rem(stride).unwrap_or(0);
    let is_planted_pair = index.checked_div(stride).is_some_and(|k| k < spec.planted);

    let mut record = if is_planted_pair && slot == 0 {
        // Service parent.
        let mut r = ProcessRecord::new(pid, SERVICE_NAME.to_owned());
        r.executable_path = Some(format!("/usr/sbin/{SERVICE_NAME}").into());
        r
    } else if is_planted_pair && slot == 1 {
        // Shell child of the service emitted immediately before it.
        let mut r = ProcessRecord::new(pid, SHELL_NAME.to_owned());
        r.ppid = Some(daemoneye_lib::models::process::ProcessId::new(
            pid.saturating_sub(1),
        ));
        r.executable_path = Some(format!("/bin/{SHELL_NAME}").into());
        r
    } else {
        // Noise. Some of it is named `bash` so the join cannot pass by matching
        // the child name alone; its parent is a pid that is never emitted, so a
        // noise row can never resolve to a service whatever the spec.
        let name = if index.is_multiple_of(7) {
            SHELL_NAME
        } else {
            "worker"
        };
        let mut r = ProcessRecord::new(pid, name.to_owned());
        r.ppid = Some(daemoneye_lib::models::process::ProcessId::new(
            NOISE_PARENT_PID,
        ));
        r.executable_path = Some(format!("/usr/bin/{name}").into());
        r
    };

    record.collection_time = ts_to_utc(ts_ms)?;
    Ok(record)
}

/// Convert an epoch millisecond to the `DateTime<Utc>` stored on the record.
///
/// Errors rather than substituting the wall clock: `collection_time` is the
/// only event time the provider can see, so a silent fallback to "now" would
/// scatter a row into the wrong bucket and break the determinism the decision
/// artifact rests on.
fn ts_to_utc(ts_ms: u64) -> Result<DateTime<Utc>, StorageError> {
    DateTime::from_timestamp_millis(ts_ms.cast_signed()).ok_or_else(|| StorageError::Overflow {
        context: format!("ts_ms {ts_ms} is outside the representable timestamp range"),
    })
}

/// Build an overflow error with context.
fn overflow(context: &str) -> StorageError {
    StorageError::Overflow {
        context: context.to_owned(),
    }
}
