//! Single-writer group-commit ingest pipeline (T3 · U5).
//!
//! A bounded [`tokio::sync::mpsc`] channel feeds a dedicated writer task that
//! batches records — up to `batch_records` or `batch_window`, whichever comes
//! first — and group-commits each batch to the [`EventStore`] in one redb write
//! transaction (one fsync per group). The writer maintains a **per-collector
//! watermark** (the highest committed `source_seq`) and discards any record at
//! or below it, plus exact `(collector_id, source_seq)` duplicates within a
//! batch — the idempotency gate (R7) that survives an in-session re-delivery.
//!
//! Backpressure is **bounded-block, never drop**: a full channel records a
//! saturation alert and then blocks the producer until capacity frees, so
//! procmond's WAL absorbs the burst rather than the pipeline losing telemetry
//! (R5). [`IngestMetrics`] exposes the commit/write/discard/saturation counters
//! so callers and tests can assert the mechanism ran, not just the outcome.
//!
//! The writer interface (submit → durable group commit → per-collector
//! watermark) is the stable contract T6 builds on; the tokio-task internals are
//! swappable behind it.

use crate::models::ProcessRecord;
use crate::storage::EventStore;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::warn;

/// One record submitted to the ingest pipeline.
pub struct IngestRecord {
    /// Source collector identity (procmond's `client_id`); the dedup namespace.
    pub collector_id: String,
    /// Monotonic per-collector sequence (procmond's WAL sequence); the dedup key.
    pub source_seq: u64,
    /// Event collection timestamp in milliseconds (the primary-key time).
    pub ts_ms: u64,
    /// Per-writer sequence disambiguator within a bucket.
    pub seq: u32,
    /// The process event payload.
    pub record: ProcessRecord,
}

/// Group-commit and channel tunables (defaults follow the §11.7.7 playbook).
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// Bounded channel capacity (backpressure point).
    pub channel_capacity: usize,
    /// Max records per group commit (`N`).
    pub batch_records: usize,
    /// Max time to wait while filling a batch (`T`).
    pub batch_window: Duration,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 4096,
            batch_records: 2000,
            batch_window: Duration::from_millis(7),
        }
    }
}

/// Observable ingest counters (assert the mechanism, not just the outcome).
#[derive(Debug, Default)]
pub struct IngestMetrics {
    /// Number of group commits performed.
    pub commits: AtomicU64,
    /// Number of records durably written.
    pub records_written: AtomicU64,
    /// Number of records discarded as duplicates (watermark or within-batch).
    pub duplicates_discarded: AtomicU64,
    /// Number of times a full channel forced bounded-block backpressure.
    pub saturation_alerts: AtomicU64,
}

/// Errors surfaced to the producer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IngestError {
    /// The ingest pipeline has shut down and is no longer accepting records.
    #[error("ingest channel closed")]
    Closed,
}

/// Producer-side handle to a running ingest pipeline.
pub struct IngestHandle {
    sender: mpsc::Sender<IngestRecord>,
    metrics: Arc<IngestMetrics>,
    writer: JoinHandle<()>,
}

impl IngestHandle {
    /// Submit a record. Bounded-block backpressure: on a full channel this
    /// records a saturation alert and then awaits capacity — it never drops.
    pub async fn submit(&self, rec: IngestRecord) -> Result<(), IngestError> {
        match self.sender.try_send(rec) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(pending)) => {
                self.metrics
                    .saturation_alerts
                    .fetch_add(1, Ordering::Relaxed);
                self.sender
                    .send(pending)
                    .await
                    .map_err(|_closed| IngestError::Closed)
            }
            Err(mpsc::error::TrySendError::Closed(_rec)) => Err(IngestError::Closed),
        }
    }

    /// Observable counters for operability and tests.
    pub const fn metrics(&self) -> &Arc<IngestMetrics> {
        &self.metrics
    }

    /// Close the channel and wait for the writer to drain and commit the tail.
    pub async fn flush_and_stop(self) {
        drop(self.sender);
        if let Err(join_err) = self.writer.await {
            warn!(error = %join_err, "ingest writer task join failed");
        }
    }
}

/// Spawn the single-writer ingest pipeline over `store`.
pub fn spawn(store: Arc<EventStore>, config: IngestConfig) -> IngestHandle {
    let (sender, receiver) = mpsc::channel(config.channel_capacity);
    let metrics = Arc::new(IngestMetrics::default());
    let writer_metrics = Arc::clone(&metrics);
    let writer = tokio::spawn(run_writer(store, receiver, config, writer_metrics));
    IngestHandle {
        sender,
        metrics,
        writer,
    }
}

/// The dedicated writer loop: batch by `N`/`T`, group-commit, advance watermarks.
async fn run_writer(
    store: Arc<EventStore>,
    mut receiver: mpsc::Receiver<IngestRecord>,
    config: IngestConfig,
    metrics: Arc<IngestMetrics>,
) {
    let mut watermarks: HashMap<String, u64> = HashMap::new();
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        let started = Instant::now();
        while batch.len() < config.batch_records {
            let remaining = config.batch_window.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Some(rec)) => batch.push(rec),
                // Channel closed (None) or the batch window elapsed (Err): commit.
                Ok(None) | Err(_) => break,
            }
        }
        commit_batch(&store, &mut watermarks, batch, &metrics);
    }
}

/// Filter a batch against the per-collector watermark + within-batch duplicates,
/// group-commit the survivors, then advance the watermarks on success.
fn commit_batch(
    store: &EventStore,
    watermarks: &mut HashMap<String, u64>,
    batch: Vec<IngestRecord>,
    metrics: &IngestMetrics,
) {
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut to_write: Vec<IngestRecord> = Vec::with_capacity(batch.len());
    for rec in batch {
        let already_committed =
            matches!(watermarks.get(&rec.collector_id), Some(&wm) if rec.source_seq <= wm);
        let within_batch_dup = !seen.insert((rec.collector_id.clone(), rec.source_seq));
        if already_committed || within_batch_dup {
            metrics.duplicates_discarded.fetch_add(1, Ordering::Relaxed);
        } else {
            to_write.push(rec);
        }
    }
    if to_write.is_empty() {
        return;
    }
    match store.put_batch(&to_write) {
        Ok(()) => {
            for rec in &to_write {
                let entry = watermarks
                    .entry(rec.collector_id.clone())
                    .or_insert(rec.source_seq);
                *entry = (*entry).max(rec.source_seq);
            }
            metrics.commits.fetch_add(1, Ordering::Relaxed);
            let written = u64::try_from(to_write.len()).unwrap_or(u64::MAX);
            metrics
                .records_written
                .fetch_add(written, Ordering::Relaxed);
        }
        Err(err) => {
            warn!(error = %err, batch_len = to_write.len(), "ingest group commit failed");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(collector: &str, source_seq: u64, ts_ms: u64, seq: u32, pid: u32) -> IngestRecord {
        IngestRecord {
            collector_id: collector.to_owned(),
            source_seq,
            ts_ms,
            seq,
            record: ProcessRecord::new(pid, format!("proc-{pid}")),
        }
    }

    fn store_at(name: &str) -> (tempfile::TempDir, Arc<EventStore>) {
        let dir = tempdir().expect("temp dir");
        let store = EventStore::new(dir.path().join(name)).expect("event store");
        (dir, Arc::new(store))
    }

    #[tokio::test]
    async fn group_commit_batches_many_records_into_one_commit() {
        let (_dir, store) = store_at("group.redb");
        // Large window so all submitted records batch before the channel closes.
        let cfg = IngestConfig {
            channel_capacity: 64,
            batch_records: 2000,
            batch_window: Duration::from_secs(5),
        };
        let handle = spawn(Arc::clone(&store), cfg);
        for s in 1..=5_u64 {
            let n = u32::try_from(s).unwrap_or(0);
            handle
                .submit(record("c", s, 1_000, n, n))
                .await
                .expect("submit");
        }
        let metrics = Arc::clone(handle.metrics());
        handle.flush_and_stop().await;

        // Mechanism: the 5 records landed in exactly ONE group commit.
        assert_eq!(
            metrics.commits.load(Ordering::Relaxed),
            1,
            "single group commit"
        );
        assert_eq!(metrics.records_written.load(Ordering::Relaxed), 5);
        assert_eq!(store.event_count().expect("count"), 5);
    }

    #[tokio::test]
    async fn watermark_discards_redelivered_source_seq() {
        let (_dir, store) = store_at("wm.redb");
        // batch_records = 1 commits each record immediately, advancing the
        // watermark so the next re-delivery is discarded deterministically.
        let cfg = IngestConfig {
            channel_capacity: 16,
            batch_records: 1,
            batch_window: Duration::from_secs(5),
        };
        let handle = spawn(Arc::clone(&store), cfg);
        handle
            .submit(record("c", 5, 1_000, 1, 1))
            .await
            .expect("s1");
        handle
            .submit(record("c", 5, 1_000, 2, 2))
            .await
            .expect("dup1"); // <= wm 5
        handle
            .submit(record("c", 3, 1_000, 3, 3))
            .await
            .expect("dup2"); // <= wm 5
        handle
            .submit(record("c", 6, 1_000, 4, 4))
            .await
            .expect("new");
        let metrics = Arc::clone(handle.metrics());
        handle.flush_and_stop().await;

        // Only source_seq 5 and 6 are durable; 5-again and 3 were discarded.
        assert_eq!(store.event_count().expect("count"), 2);
        assert_eq!(metrics.duplicates_discarded.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn within_batch_duplicate_is_discarded() {
        let (_dir, store) = store_at("batchdup.redb");
        let cfg = IngestConfig {
            channel_capacity: 16,
            batch_records: 2000,
            batch_window: Duration::from_secs(5),
        };
        let handle = spawn(Arc::clone(&store), cfg);
        handle
            .submit(record("c", 1, 1_000, 1, 1))
            .await
            .expect("s1");
        handle
            .submit(record("c", 2, 1_000, 2, 2))
            .await
            .expect("s2");
        handle
            .submit(record("c", 1, 1_000, 3, 3))
            .await
            .expect("dup"); // same (c,1)
        let metrics = Arc::clone(handle.metrics());
        handle.flush_and_stop().await;

        assert_eq!(metrics.duplicates_discarded.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.records_written.load(Ordering::Relaxed), 2);
        assert_eq!(store.event_count().expect("count"), 2);
    }

    #[tokio::test]
    async fn bounded_channel_blocks_but_never_drops() {
        let (_dir, store) = store_at("backpressure.redb");
        // Tiny channel forces backpressure; every record must still persist.
        let cfg = IngestConfig {
            channel_capacity: 1,
            batch_records: 4,
            batch_window: Duration::from_millis(20),
        };
        let handle = spawn(Arc::clone(&store), cfg);
        let total = 50_u64;
        for s in 0..total {
            let seq = u32::try_from(s).unwrap_or(0);
            handle
                .submit(record("c", s, 1_000, seq, seq.max(1)))
                .await
                .expect("submit");
        }
        handle.flush_and_stop().await;

        // No drops: every distinct record is durable despite the 1-slot channel.
        assert_eq!(store.event_count().expect("count"), total);
    }
}
