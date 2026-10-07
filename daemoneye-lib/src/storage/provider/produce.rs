//! The blocking producer behind one `BucketScanExec` partition (R9, R12).
//!
//! Runs under `spawn_blocking`. It owns one redb snapshot, walks its run of buckets, and sends a
//! `RecordBatch` whenever the pending rows reach the row bound or the next row would cross the
//! byte bound. No lock or transaction is held across an `.await`: there is no `.await` here.

use super::arrow::{ArrowEncodeError, encode};
use super::filters::PushedFilters;
use super::keyset::{Key, intersect_sorted, union_sorted};
use super::scan::ScanPlan;
use super::{ScanCounters, ScanLimits};
use crate::models::ProcessRecord;
use crate::storage::postings_cache::{Postings, PostingsCache};
use crate::storage::read::{BucketReader, KeyedRecord};
use crate::storage::{EventStore, StorageError};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::error::{DataFusionError, Result};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Bytes charged to every row for its fixed-width columns, offsets and validity bits.
///
/// Measured, not guessed: `provider_row_estimate_never_undercounts_the_arrow_batch` fails if a
/// typical batch's real Arrow size ever exceeds the estimate built from this.
pub(super) const ROW_FIXED_BYTES: usize = 128;

/// Rows pulled from redb per read, so a run of very large rows cannot be decoded all at once.
///
/// The read primitives take a row limit only, so this is also the most rows decoded between byte
/// checks: a chunk of maximally-sized rows overshoots `batch_max_bytes` transiently by up to
/// `READ_CHUNK_ROWS` rows before the oversized check runs on each.
const READ_CHUNK_ROWS: usize = 256;

/// The Arrow size a row will take, estimated from its variable-length fields.
///
/// Used both to close a batch before it crosses `batch_max_bytes` and to refuse a row that could
/// never fit. Over-counts slightly by design: it is cheaper to close a batch early than to blow
/// the bound.
pub(super) fn estimated_row_bytes(record: &ProcessRecord) -> usize {
    [
        Some(record.name.len()),
        record
            .executable_path
            .as_ref()
            .map(|p| p.to_string_lossy().len()),
        record.command_line.as_ref().map(String::len),
        record.executable_hash.as_ref().map(String::len),
    ]
    .into_iter()
    .flatten()
    .fold(ROW_FIXED_BYTES, usize::saturating_add)
}

/// Why the producer stopped early.
enum Stop {
    /// The consumer dropped the stream; not an error.
    Closed,
    Failed(DataFusionError),
}

impl From<StorageError> for Stop {
    fn from(err: StorageError) -> Self {
        Self::Failed(DataFusionError::External(Box::new(err)))
    }
}

impl From<ArrowEncodeError> for Stop {
    fn from(err: ArrowEncodeError) -> Self {
        Self::Failed(DataFusionError::External(Box::new(err)))
    }
}

/// Rows accumulated for the next batch.
#[derive(Default)]
struct Pending {
    rows: Vec<ProcessRecord>,
    bytes: usize,
}

/// One partition's work, cloned out of the plan so it can move onto the blocking thread.
pub(super) struct PartitionJob {
    store: Arc<EventStore>,
    cache: Arc<PostingsCache>,
    table_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    filters: PushedFilters,
    buckets: Vec<u64>,
    limits: ScanLimits,
    now_bucket: u64,
    counters: Arc<ScanCounters>,
}

impl PartitionJob {
    pub(super) fn new(plan: &ScanPlan, buckets: Vec<u64>) -> Self {
        Self {
            store: Arc::clone(&plan.store),
            cache: Arc::clone(&plan.cache),
            table_schema: Arc::clone(&plan.table_schema),
            projection: plan.projection.clone(),
            filters: plan.filters.clone(),
            buckets,
            limits: plan.limits,
            now_bucket: plan.now_bucket,
            counters: Arc::clone(&plan.counters),
        }
    }

    /// Read the run and send batches; a dropped receiver ends the run quietly.
    pub(super) fn run(self, tx: &Sender<Result<RecordBatch>>) -> Result<()> {
        match self.drive(tx) {
            Ok(()) | Err(Stop::Closed) => Ok(()),
            Err(Stop::Failed(err)) => Err(err),
        }
    }

    fn drive(&self, tx: &Sender<Result<RecordBatch>>) -> Result<(), Stop> {
        let reader = self.store.open_read()?;
        let mut pending = Pending::default();
        for &bucket in &self.buckets {
            self.read_bucket(&reader, bucket, &mut pending, tx)?;
        }
        self.flush(&mut pending, tx)
    }

    fn chunk_rows(&self) -> usize {
        self.limits.batch_size.min(READ_CHUNK_ROWS)
    }

    fn read_bucket(
        &self,
        reader: &BucketReader,
        bucket: u64,
        pending: &mut Pending,
        tx: &Sender<Result<RecordBatch>>,
    ) -> Result<(), Stop> {
        let (start, end) = (self.filters.start_ms(), self.filters.end_ms());
        let chunk_rows = self.chunk_rows();
        if self.filters.term_sets().is_empty() {
            let mut after = None;
            loop {
                let chunk = reader.range_chunk(bucket, start, end, after, chunk_rows)?;
                let Some(&(last, _)) = chunk.last() else {
                    return Ok(());
                };
                after = Some(last);
                let is_full = chunk.len() == chunk_rows;
                self.admit_all(chunk, pending, tx)?;
                if !is_full {
                    return Ok(());
                }
            }
        }
        let keys = self.candidate_keys(reader, bucket)?;
        for window in keys.chunks(chunk_rows) {
            self.admit_all(reader.fetch(bucket, window)?, pending, tx)?;
        }
        Ok(())
    }

    /// Posting-list intersection for `bucket`, clamped to the time window (R12).
    fn candidate_keys(&self, reader: &BucketReader, bucket: u64) -> Result<Vec<Key>, Stop> {
        let mut acc: Option<Vec<Key>> = None;
        for set in self.filters.term_sets() {
            let lists = set
                .iter()
                .map(|&term| {
                    let key = (term.kind(), bucket, term.as_u128());
                    self.cache
                        .get_or_load(key, self.now_bucket, || reader.postings(bucket, term))
                })
                .collect::<Result<Vec<Postings>, StorageError>>()?;
            let slices: Vec<&[Key]> = lists.iter().map(AsRef::as_ref).collect();
            let merged = match *slices.as_slice() {
                [only] => only.to_vec(),
                ref many => union_sorted(many),
            };
            let next = match acc {
                None => merged,
                Some(prev) => intersect_sorted(&prev, &merged),
            };
            let is_empty = next.is_empty();
            acc = Some(next);
            if is_empty {
                break;
            }
        }
        let (start, end) = (self.filters.start_ms(), self.filters.end_ms());
        let mut keys = acc.unwrap_or_default();
        keys.retain(|&(ts_ms, _)| ts_ms >= start && ts_ms < end);
        Ok(keys)
    }

    fn admit_all(
        &self,
        chunk: Vec<KeyedRecord>,
        pending: &mut Pending,
        tx: &Sender<Result<RecordBatch>>,
    ) -> Result<(), Stop> {
        if tx.is_closed() {
            return Err(Stop::Closed);
        }
        for (_key, record) in chunk {
            self.counters.add_rows_read(1);
            let size = estimated_row_bytes(&record);
            if size > self.limits.batch_max_bytes {
                self.counters.add_oversized(1);
                continue;
            }
            if !pending.rows.is_empty()
                && pending.bytes.saturating_add(size) > self.limits.batch_max_bytes
            {
                self.flush(pending, tx)?;
            }
            pending.bytes = pending.bytes.saturating_add(size);
            pending.rows.push(record);
            if pending.rows.len() >= self.limits.batch_size {
                self.flush(pending, tx)?;
            }
        }
        Ok(())
    }

    fn flush(&self, pending: &mut Pending, tx: &Sender<Result<RecordBatch>>) -> Result<(), Stop> {
        let Pending { rows, .. } = std::mem::take(pending);
        if rows.is_empty() {
            return Ok(());
        }
        let batch = encode(&rows, &self.table_schema, self.projection.as_deref())?;
        self.counters.add_batch();
        tx.blocking_send(Ok(batch)).map_err(|_closed| Stop::Closed)
    }
}
