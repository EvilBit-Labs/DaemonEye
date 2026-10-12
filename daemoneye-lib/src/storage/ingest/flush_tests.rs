//! Flush barrier, gap detection, durable watermark, and the `ts_ms` derivation invariant.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use chrono::{TimeZone, Utc};
use tempfile::tempdir;

const BASE_MS: u64 = 36_000_000;

fn process_at(ts_ms: u64, pid: u32) -> ProcessRecord {
    let mut record = ProcessRecord::new(pid, format!("proc-{pid}"));
    record.collection_time = Utc
        .timestamp_millis_opt(i64::try_from(ts_ms).unwrap())
        .unwrap();
    record
}

fn rec(collector: &str, source_seq: u64, seq: u32, ts_ms: u64) -> IngestRecord {
    IngestRecord::new(collector, source_seq, seq, process_at(ts_ms, seq.max(1))).unwrap()
}

fn store_at(name: &str) -> (tempfile::TempDir, Arc<EventStore>) {
    let dir = tempdir().unwrap();
    let store = EventStore::new(dir.path().join(name)).unwrap();
    (dir, Arc::new(store))
}

/// A window long enough that only an explicit flush (or close) commits.
fn slow_window() -> IngestConfig {
    IngestConfig {
        channel_capacity: 64,
        batch_records: 2000,
        batch_window: Duration::from_secs(30),
    }
}

#[test]
fn ts_ms_is_the_record_collection_time_and_cannot_be_supplied() {
    let record = IngestRecord::new("c", 1, 0, process_at(BASE_MS + 7, 1)).unwrap();
    assert_eq!(record.ts_ms(), BASE_MS + 7);
}

#[test]
fn a_collection_time_before_the_epoch_is_rejected() {
    let mut record = ProcessRecord::new(1, "old".to_owned());
    record.collection_time = Utc.timestamp_millis_opt(-1).unwrap();
    let err = IngestRecord::new("c", 1, 0, record).expect_err("rejected");
    assert!(matches!(err, IngestError::InvalidTimestamp { .. }));
}

#[tokio::test]
async fn an_ingested_row_is_keyed_by_its_collection_time() {
    let (_dir, store) = store_at("key.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    handle.submit(rec("c", 1, 3, BASE_MS + 42)).await.unwrap();
    handle.flush().await.unwrap();

    // The pruning key a query derives from `collection_time` finds the row.
    assert!(store.get_event(BASE_MS + 42, 3).unwrap().is_some());
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn flush_returns_only_after_the_batch_is_committed() {
    let (_dir, store) = store_at("barrier.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    for i in 0..10_u32 {
        let seq = u64::from(i);
        handle
            .submit(rec("c", seq, i, BASE_MS + seq))
            .await
            .unwrap();
    }
    handle.flush().await.unwrap();
    assert_eq!(store.event_count().unwrap(), 10);
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn redelivery_is_a_duplicate_not_a_gap() {
    let (_dir, store) = store_at("dup.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    for round in 0..2_u32 {
        for i in 0..10_u32 {
            let seq = u64::from(i);
            handle
                .submit(rec("c", seq, i + round * 100, BASE_MS + seq))
                .await
                .unwrap();
        }
        let report = handle.flush().await.unwrap();
        assert!(report.gaps.is_empty());
    }
    assert_eq!(store.event_count().unwrap(), 10);
    assert_eq!(
        handle
            .metrics()
            .duplicates_discarded
            .load(Ordering::Relaxed),
        10
    );
    assert_eq!(
        handle
            .metrics()
            .sequence_gaps_detected
            .load(Ordering::Relaxed),
        0
    );
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn a_skipped_sequence_is_a_gap_naming_expected_and_observed() {
    let (_dir, store) = store_at("gap.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    handle.submit(rec("c", 10, 0, BASE_MS)).await.unwrap();
    handle.submit(rec("c", 13, 1, BASE_MS + 1)).await.unwrap();
    let report = handle.flush().await.unwrap();

    assert_eq!(
        report.gaps,
        vec![SequenceGap {
            collector_id: "c".to_owned(),
            expected_seq: 11,
            observed_seq: 13,
        }]
    );
    assert_eq!(
        handle
            .metrics()
            .sequence_gaps_detected
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        handle
            .metrics()
            .duplicates_discarded
            .load(Ordering::Relaxed),
        0
    );
    // The gap is reported once: the next flush starts clean.
    assert!(handle.flush().await.unwrap().gaps.is_empty());
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn the_first_record_of_a_collector_is_not_a_gap() {
    let (_dir, store) = store_at("first.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    handle.submit(rec("c", 9_000, 0, BASE_MS)).await.unwrap();
    assert!(handle.flush().await.unwrap().gaps.is_empty());
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn consecutive_epochs_are_contiguous_but_a_skipped_epoch_is_a_gap() {
    let (_dir, store) = store_at("epoch.redb");
    let handle = spawn(Arc::clone(&store), slow_window());
    let epoch = |n: u64| n << 32;
    handle
        .submit(rec("c", epoch(4) | 1, 0, BASE_MS))
        .await
        .unwrap();
    // The next epoch's first row follows the previous epoch's last.
    handle
        .submit(rec("c", epoch(5), 1, BASE_MS + 1))
        .await
        .unwrap();
    assert!(handle.flush().await.unwrap().gaps.is_empty());
    handle
        .submit(rec("c", epoch(7), 2, BASE_MS + 2))
        .await
        .unwrap();
    let report = handle.flush().await.unwrap();
    assert_eq!(report.gaps.len(), 1);
    assert_eq!(
        report.gaps.first().map(|g| g.expected_seq),
        Some(epoch(5) | 1)
    );
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn the_watermark_survives_a_restart_and_discards_redelivery() {
    let (_dir, store) = store_at("restart.redb");
    let first = spawn(Arc::clone(&store), slow_window());
    first.submit(rec("c", 5, 0, BASE_MS)).await.unwrap();
    first.flush_and_stop().await;
    assert_eq!(store.ingest_watermarks().unwrap().get("c"), Some(&5));

    // A fresh pipeline over the same store starts from the persisted watermark.
    let second = spawn(Arc::clone(&store), slow_window());
    second.submit(rec("c", 5, 1, BASE_MS + 1)).await.unwrap();
    second.submit(rec("c", 6, 2, BASE_MS + 2)).await.unwrap();
    second.flush().await.unwrap();
    assert_eq!(
        second
            .metrics()
            .duplicates_discarded
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(store.event_count().unwrap(), 2);
    second.flush_and_stop().await;
}
