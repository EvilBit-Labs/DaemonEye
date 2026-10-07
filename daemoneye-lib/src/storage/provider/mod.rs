//! `DataFusion` `TableProvider` over the event store (ADR-0006, ADR-0008, KTD2).
//!
//! The provider reads a bucket at a time and emits `RecordBatch`es bounded in
//! both rows and bytes, so peak resident set tracks one batch rather than the
//! retention window. It reports `TableProviderFilterPushDown::Inexact` for
//! every predicate it consumes and never `Exact`, so a `FilterExec` always
//! re-checks the rows an index lookup admitted (R10).
//!
//! # Layout
//!
//! - `filters`: which predicates the provider consumes, and the time window and index terms taken
//!   from them. One parser serves both `supports_filters_pushdown` and `scan`.
//! - `scan`: `BucketScanExec`, the `ExecutionPlan`.
//! - `produce`: the blocking producer for one partition (chunked reads, byte and row bounds).
//! - `keyset`: sorted-key intersection and union over posting lists.
//!
//! # Assumption: the key time is the `collection_time`
//!
//! Buckets are keyed by the ingest `ts_ms`, and the scan prunes buckets and clamps key ranges by
//! the `collection_time` predicates it is given. That is sound only while a row's key `ts_ms`
//! equals its `collection_time` in milliseconds. `IngestRecord` enforces it: its key time is derived
//! from the record and cannot be supplied. `EventStore::put_event` still takes a free `ts_ms`; it
//! is a test and fixture path, not one production code calls.

pub mod arrow;
mod filters;
mod keyset;
mod produce;
mod scan;

pub use scan::BucketScanExec;

use self::arrow::{ArrowEncodeError, schema_for};
use self::filters::{PushedFilters, classify};
use self::scan::ScanPlan;
use crate::config::DetectionConfig;
use crate::detection_bounds::{
    EXECUTOR_BATCH_MAX_BYTES, EXECUTOR_BATCH_SIZE, EXECUTOR_TARGET_PARTITIONS,
};
use crate::proto::TableDescriptor;
use crate::storage::EventStore;
use crate::storage::postings_cache::PostingsCache;
use async_trait::async_trait;
use chrono::Utc;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::Session;
use datafusion::datasource::{TableProvider, TableType};
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::empty::EmptyExec;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Plain-value knobs for a scan (KTD2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    /// Most partitions a scan may emit, however many buckets survive pruning (R11).
    pub target_partitions: usize,
    /// Most rows in one `RecordBatch` (R9).
    pub batch_size: usize,
    /// Most estimated decoded bytes in one `RecordBatch`; a row above it is excluded (R9).
    pub batch_max_bytes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            target_partitions: EXECUTOR_TARGET_PARTITIONS,
            batch_size: EXECUTOR_BATCH_SIZE,
            batch_max_bytes: EXECUTOR_BATCH_MAX_BYTES,
        }
    }
}

impl From<&DetectionConfig> for ScanLimits {
    /// The scan limits an operator configured; the values were range-checked by
    /// [`DetectionConfig::validate`], which is what keeps them non-zero here.
    fn from(config: &DetectionConfig) -> Self {
        Self {
            target_partitions: config.executor_target_partitions,
            batch_size: config.executor_batch_size,
            batch_max_bytes: config.executor_batch_max_bytes,
        }
    }
}

/// Why a provider could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The descriptor names a column the encoder cannot back.
    #[error("descriptor cannot be backed: {0}")]
    Schema(#[from] ArrowEncodeError),
    /// A [`ScanLimits`] field was zero.
    #[error("scan limit `{0}` must be at least 1")]
    InvalidLimit(&'static str),
}

/// What the scans of one provider have read, cumulative across scans.
///
/// This is the seam U10's completeness fold reads: after draining a rule's stream, a non-zero
/// [`ScanCounters::oversized_rows`] means the evaluation skipped rows too large to batch and must
/// be degraded with `CompletenessReason::ResourceLimit` naming [`ScanCounters::table`]. A
/// provider is built per evaluation, so the counts are per evaluation.
#[derive(Debug)]
pub struct ScanCounters {
    table: String,
    rows_read: AtomicU64,
    oversized_rows: AtomicU64,
    batches: AtomicU64,
}

impl ScanCounters {
    pub(super) const fn new(table: String) -> Self {
        Self {
            table,
            rows_read: AtomicU64::new(0),
            oversized_rows: AtomicU64::new(0),
            batches: AtomicU64::new(0),
        }
    }
    pub(super) fn add_rows_read(&self, n: u64) {
        self.rows_read.fetch_add(n, Ordering::Relaxed);
    }

    pub(super) fn add_oversized(&self, n: u64) {
        self.oversized_rows.fetch_add(n, Ordering::Relaxed);
    }

    pub(super) fn add_batch(&self) {
        self.batches.fetch_add(1, Ordering::Relaxed);
    }

    /// Name of the catalog table the provider serves.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Rows decoded from the store, before any `FilterExec`. With an index lookup this counts
    /// only the intersected keys.
    #[must_use]
    pub fn rows_read(&self) -> u64 {
        self.rows_read.load(Ordering::Relaxed)
    }

    /// Rows excluded because their own estimated size exceeded `batch_max_bytes` (R9).
    #[must_use]
    pub fn oversized_rows(&self) -> u64 {
        self.oversized_rows.load(Ordering::Relaxed)
    }

    /// `RecordBatch`es sent.
    #[must_use]
    pub fn batches(&self) -> u64 {
        self.batches.load(Ordering::Relaxed)
    }
}

/// A `DataFusion` table over one catalog table of the event store.
#[derive(Debug)]
pub struct EventStoreTableProvider {
    store: Arc<EventStore>,
    cache: Arc<PostingsCache>,
    schema: SchemaRef,
    limits: ScanLimits,
    counters: Arc<ScanCounters>,
}

impl EventStoreTableProvider {
    /// Build a provider for `table`.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Schema`] when the descriptor names a column the encoder cannot back, and
    /// [`ProviderError::InvalidLimit`] when a [`ScanLimits`] field is zero.
    pub fn new(
        store: Arc<EventStore>,
        cache: Arc<PostingsCache>,
        table: &TableDescriptor,
        limits: ScanLimits,
    ) -> Result<Self, ProviderError> {
        for (name, value) in [
            ("target_partitions", limits.target_partitions),
            ("batch_size", limits.batch_size),
            ("batch_max_bytes", limits.batch_max_bytes),
        ] {
            if value == 0 {
                return Err(ProviderError::InvalidLimit(name));
            }
        }
        let schema = schema_for(table)?;
        Ok(Self {
            store,
            cache,
            schema,
            limits,
            counters: Arc::new(ScanCounters::new(table.name.clone())),
        })
    }

    /// Counters shared by every scan this provider plans.
    #[must_use]
    pub const fn counters(&self) -> &Arc<ScanCounters> {
        &self.counters
    }

    /// Id of the bucket holding "now"; it and every later bucket may still change.
    ///
    /// `0` on a clock or arithmetic failure, which makes every bucket count as open, so the cache
    /// is bypassed: the failure direction is a slower read, never a stale one.
    fn open_bucket_id(&self) -> u64 {
        u64::try_from(Utc::now().timestamp_millis())
            .ok()
            .and_then(|now_ms| now_ms.checked_div(self.store.granularity_ms()))
            .unwrap_or(0)
    }
}

/// Split `buckets` into `min(target, len)` contiguous runs whose lengths differ by at most one.
fn split_runs(buckets: &[u64], target: usize) -> Vec<Vec<u64>> {
    let parts = target.min(buckets.len());
    let base = buckets.len().checked_div(parts).unwrap_or(0);
    let extra = buckets.len().checked_rem(parts).unwrap_or(0);
    let mut rest = buckets;
    let mut runs = Vec::with_capacity(parts);
    for index in 0..parts {
        let take = base.saturating_add(usize::from(index < extra));
        let (head, tail) = rest.split_at_checked(take).unwrap_or((rest, &[]));
        runs.push(head.to_vec());
        rest = tail;
    }
    runs
}

#[async_trait]
impl TableProvider for EventStoreTableProvider {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
    fn table_type(&self) -> TableType {
        TableType::Base
    }

    /// Report `Inexact` for every `collection_time` comparison and every equality or `IN` on
    /// `pid`, `ppid`, `name` and `executable_hash` that `scan` consumes, `Unsupported` otherwise.
    ///
    /// **Invariant (R10): this never returns `Exact`.** The index lookups are bucket- or
    /// hash-granular: `idx:name` keys a lowercase 128-bit hash, so `name = 'Bash'` admits a stored
    /// `bash` and a hash collision admits a different name, and the time window is clamped at
    /// bucket and key granularity rather than per row. `Exact` would make `DataFusion` drop its
    /// `FilterExec`, so those rows would leak into results with no re-check. `Inexact` keeps the
    /// `FilterExec` above the scan as the only judge of a row.
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        Ok(filters
            .iter()
            .map(|e| match classify(e, &self.schema) {
                Some(_) => TableProviderFilterPushDown::Inexact,
                None => TableProviderFilterPushDown::Unsupported,
            })
            .collect())
    }

    /// Plan a scan: prune buckets by the time window, group the survivors into at most
    /// `target_partitions` contiguous runs, and return a [`BucketScanExec`], or an `EmptyExec`
    /// when no bucket survives. `limit` is ignored; the `LimitExec` above the scan applies it.
    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        _limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let pushed = PushedFilters::from_filters(filters, &self.schema);
        let projected = match projection {
            Some(p) => Arc::new(self.schema.project(p)?),
            None => Arc::clone(&self.schema),
        };
        let buckets = self
            .store
            .bucket_ids_in(pushed.start_ms(), pushed.end_ms())
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        let runs = split_runs(&buckets, self.limits.target_partitions);
        if runs.is_empty() {
            return Ok(Arc::new(EmptyExec::new(projected)));
        }
        let plan = ScanPlan {
            store: Arc::clone(&self.store),
            cache: Arc::clone(&self.cache),
            table_schema: Arc::clone(&self.schema),
            projection: projection.cloned(),
            filters: pushed,
            runs,
            limits: self.limits,
            now_bucket: self.open_bucket_id(),
            counters: Arc::clone(&self.counters),
        };
        Ok(Arc::new(BucketScanExec::new(plan, projected)))
    }
}

#[cfg(test)]
mod tests;
