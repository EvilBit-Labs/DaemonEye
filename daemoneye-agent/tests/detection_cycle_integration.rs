//! Agent storage wiring (T6 · U12): the ingest cycle, the flush barrier, ordinal restore, rule
//! loading, and alert persistence, each over a real `tempfile` event store.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{TimeZone, Utc};
use daemoneye_agent::detection_cycle::{
    PROCMOND_COLLECTOR_ID, ingest_cycle, load_persisted_rules, next_cycle_ordinal, next_window,
    open_event_store, persist_alerts,
};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::models::{
    Alert, AlertSeverity, Completeness, CompletenessReason, DetectionRule, ProcessRecord,
};
use daemoneye_lib::storage::EventStore;
use daemoneye_lib::storage::ingest::{self, IngestConfig, IngestHandle};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

const BASE_MS: u64 = 36_000_000;

struct Fx {
    _dir: TempDir,
    store: Arc<EventStore>,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("agent.redb")).unwrap());
    Fx { _dir: dir, store }
}

fn pipeline(fx: &Fx) -> IngestHandle {
    // A long window: only the flush barrier can make rows visible in time.
    ingest::spawn(
        Arc::clone(&fx.store),
        IngestConfig {
            batch_window: std::time::Duration::from_secs(30),
            ..IngestConfig::default()
        },
    )
}

fn processes(count: u32, first_ms: u64) -> Vec<ProcessRecord> {
    (0..count)
        .map(|i| {
            let mut record = ProcessRecord::new(i.saturating_add(1), format!("proc-{i}"));
            record.collection_time = Utc
                .timestamp_millis_opt(i64::try_from(first_ms.saturating_add(u64::from(i))).unwrap())
                .unwrap();
            record
        })
        .collect()
}

#[tokio::test]
async fn ingest_cycle_is_visible_the_instant_it_returns() {
    let fx = fx();
    let handle = pipeline(&fx);
    let rows = processes(10, BASE_MS);

    let outcome = ingest_cycle(&handle, PROCMOND_COLLECTOR_ID, 0, &rows)
        .await
        .unwrap();
    assert_eq!(fx.store.event_count().unwrap(), 10);

    assert_eq!(outcome.submitted, 10);
    assert_eq!(outcome.high_water_ms, BASE_MS + 9);
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn redelivering_a_cycle_is_discarded_and_counted() {
    let fx = fx();
    let handle = pipeline(&fx);
    let rows = processes(10, BASE_MS);
    ingest_cycle(&handle, PROCMOND_COLLECTOR_ID, 0, &rows)
        .await
        .unwrap();
    let again = ingest_cycle(&handle, PROCMOND_COLLECTOR_ID, 0, &rows)
        .await
        .unwrap();

    assert_eq!(fx.store.event_count().unwrap(), 10);
    assert_eq!(
        handle
            .metrics()
            .duplicates_discarded
            .load(Ordering::Relaxed),
        10
    );
    assert!(again.gaps.is_empty(), "a re-delivery is not a gap");
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn consecutive_cycles_are_not_gaps_but_a_skipped_cycle_is() {
    let fx = fx();
    let handle = pipeline(&fx);
    let first = ingest_cycle(&handle, PROCMOND_COLLECTOR_ID, 0, &processes(3, BASE_MS))
        .await
        .unwrap();
    let second = ingest_cycle(
        &handle,
        PROCMOND_COLLECTOR_ID,
        1,
        &processes(3, BASE_MS + 100),
    )
    .await
    .unwrap();
    assert!(first.gaps.is_empty() && second.gaps.is_empty());

    let skipped = ingest_cycle(
        &handle,
        PROCMOND_COLLECTOR_ID,
        3,
        &processes(3, BASE_MS + 200),
    )
    .await
    .unwrap();
    assert_eq!(skipped.gaps.len(), 1);
    let gap = skipped.gaps.first().unwrap();
    assert_eq!(gap.collector_id, PROCMOND_COLLECTOR_ID);
    assert_eq!(gap.expected_seq, (1_u64 << 32) | 3);
    assert_eq!(gap.observed_seq, 3_u64 << 32);
    assert_eq!(
        handle
            .metrics()
            .sequence_gaps_detected
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(fx.store.event_count().unwrap(), 9);
    handle.flush_and_stop().await;
}

#[tokio::test]
async fn a_restart_resumes_above_the_stored_ordinal_and_its_rows_land() {
    let fx = fx();
    assert_eq!(
        next_cycle_ordinal(&fx.store, PROCMOND_COLLECTOR_ID).unwrap(),
        0
    );

    let before = pipeline(&fx);
    for ordinal in 0..3 {
        ingest_cycle(
            &before,
            PROCMOND_COLLECTOR_ID,
            ordinal,
            &processes(4, BASE_MS),
        )
        .await
        .unwrap();
    }
    before.flush_and_stop().await;
    let rows_before = fx.store.event_count().unwrap();

    // A fresh process: new pipeline, ordinal read back from the store.
    let restored = next_cycle_ordinal(&fx.store, PROCMOND_COLLECTOR_ID).unwrap();
    assert_eq!(restored, 3);
    let after = pipeline(&fx);
    let outcome = ingest_cycle(
        &after,
        PROCMOND_COLLECTOR_ID,
        restored,
        &processes(4, BASE_MS + 1_000),
    )
    .await
    .unwrap();

    assert_eq!(fx.store.event_count().unwrap(), rows_before + 4);
    assert_eq!(
        after.metrics().duplicates_discarded.load(Ordering::Relaxed),
        0
    );
    assert!(outcome.gaps.is_empty(), "resuming one above is contiguous");

    // The same rows under a prior ordinal are what the stored watermark discards.
    ingest_cycle(&after, PROCMOND_COLLECTOR_ID, 2, &processes(4, BASE_MS))
        .await
        .unwrap();
    assert_eq!(
        after.metrics().duplicates_discarded.load(Ordering::Relaxed),
        4
    );
    after.flush_and_stop().await;
}

fn rule(id: &str, sql: &str) -> DetectionRule {
    DetectionRule::new(
        id,
        format!("Rule {id}"),
        "fixture",
        sql,
        "test",
        AlertSeverity::Low,
    )
}

#[tokio::test]
#[tracing_test::traced_test]
async fn startup_loads_the_valid_rule_and_warns_about_the_rejected_one() {
    let fx = fx();
    fx.store
        .store_rule(&rule(
            "rule-good",
            "SELECT pid FROM processes WHERE name = 'nc'",
        ))
        .unwrap();
    fx.store
        .store_rule(&rule("rule-bad", "DROP TABLE processes"))
        .unwrap();

    let mut engine = DetectionEngine::new();
    let loaded = load_persisted_rules(&fx.store, &mut engine).unwrap();

    assert_eq!(loaded, 1);
    assert!(engine.get_rule("rule-good").is_some());
    assert!(engine.get_rule("rule-bad").is_none());
    logs_assert(|lines: &[&str]| {
        let warnings = lines
            .iter()
            .filter(|line| line.contains("WARN") && line.contains("rule-bad"))
            .count();
        if warnings == 1 {
            Ok(())
        } else {
            Err("expected exactly one warning naming the rejected rule".to_owned())
        }
    });
}

#[test]
fn persisted_alerts_keep_their_completeness() {
    let fx = fx();
    let degraded = Completeness::degraded(vec![CompletenessReason::SequenceGapDetected {
        collector_id: "procmond".to_owned(),
        expected_seq: 11,
        observed_seq: 13,
    }])
    .unwrap();
    let alerts = vec![
        Alert::new(
            AlertSeverity::High,
            "complete",
            "d",
            "r1",
            ProcessRecord::new(1, "a".to_owned()),
            Completeness::complete(),
        ),
        Alert::new(
            AlertSeverity::High,
            "degraded",
            "d",
            "r2",
            ProcessRecord::new(2, "b".to_owned()),
            degraded.clone(),
        ),
    ];

    assert_eq!(persist_alerts(&fx.store, &alerts), 2);

    let stored = fx.store.get_all_alerts().unwrap();
    assert_eq!(stored.len(), 2);
    let stored_degraded = stored.iter().find(|a| a.detection_rule_id == "r2").unwrap();
    assert_eq!(stored_degraded.completeness, degraded);
}

fn write_old_schema(path: &std::path::Path) {
    let db = redb::Database::create(path).unwrap();
    let txn = db.begin_write().unwrap();
    let def: redb::TableDefinition<&str, u32> = redb::TableDefinition::new("schema_version");
    txn.open_table(def).unwrap().insert("version", 999).unwrap();
    txn.commit().unwrap();
}

#[test]
fn a_schema_mismatch_names_the_rebuild_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.redb");
    write_old_schema(&path);

    let message = open_event_store(&path, &daemoneye_lib::config::DatabaseConfig::default())
        .err()
        .unwrap()
        .to_string();

    assert!(message.contains("schema version 999"));
    assert!(message.contains("storage::schema::migrate"));
    assert!(message.contains("signed bundle"));
}

/// The agent opens the store with the page cache its config names, not the default.
#[test]
fn open_event_store_applies_the_configured_page_cache() {
    let dir = tempfile::tempdir().unwrap();
    let database = daemoneye_lib::config::DatabaseConfig {
        page_cache_mb: 4,
        ..daemoneye_lib::config::DatabaseConfig::default()
    };

    let store = open_event_store(&dir.path().join("sized.redb"), &database).unwrap();

    assert_eq!(store.page_cache_bytes(), database.page_cache_bytes());
    assert_ne!(
        store.page_cache_bytes(),
        daemoneye_lib::config::DatabaseConfig::default().page_cache_bytes()
    );
}

/// A failed ingest still consumes its ordinal and reports why, so a retry cannot alias a
/// partially committed sequence space and the cycle's evaluations are degraded rather than
/// reported complete over rows that were never stored.
#[test]
fn a_failed_ingest_consumes_its_ordinal_and_carries_the_failure() {
    use daemoneye_agent::detection_cycle::{CycleIngestError, IngestOutcome, ingest_step};

    let failed = ingest_step(7, 1_000, Err(CycleIngestError::TooManyRows));
    assert_eq!(failed.next_ordinal, Some(8));
    assert_eq!(
        failed.window,
        next_window(1_000, 1_000),
        "nothing durable, so the window does not move"
    );
    assert!(failed.gaps.is_empty());
    assert!(
        failed
            .failure
            .as_deref()
            .is_some_and(|e| e.contains("32-bit row index"))
    );

    let committed = ingest_step(
        7,
        1_000,
        Ok(IngestOutcome {
            high_water_ms: 2_500,
            low_water_ms: 1_001,
            submitted: 3,
            gaps: Vec::new(),
        }),
    );
    assert_eq!(committed.next_ordinal, Some(8));
    assert_eq!(committed.window, next_window(1_000, 2_500));
    assert_eq!(committed.failure, None);

    // A row stamped at or before the mark (a clock step-back) is not skipped: the window reaches
    // back to it, re-evaluating the overlap rather than never evaluating the row.
    let stepped_back = ingest_step(
        8,
        2_500,
        Ok(IngestOutcome {
            high_water_ms: 2_600,
            low_water_ms: 2_000,
            submitted: 2,
            gaps: Vec::new(),
        }),
    );
    assert_eq!(stepped_back.window, next_window(1_999, 2_600));
    assert_eq!(
        ingest_step(u32::MAX, 0, Err(CycleIngestError::TooManyRows)).next_ordinal,
        None
    );
}

/// The restart cursor the agent seeds its first window from is written by the store and read
/// back after a reopen.
#[test]
fn the_evaluated_through_mark_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor.redb");
    let store = open_event_store(&path, &daemoneye_lib::config::DatabaseConfig::default()).unwrap();
    assert_eq!(
        store.evaluated_through_ms(PROCMOND_COLLECTOR_ID).unwrap(),
        None
    );
    store
        .set_evaluated_through_ms(PROCMOND_COLLECTOR_ID, 42_000)
        .unwrap();
    drop(store);

    let reopened =
        open_event_store(&path, &daemoneye_lib::config::DatabaseConfig::default()).unwrap();
    assert_eq!(
        reopened
            .evaluated_through_ms(PROCMOND_COLLECTOR_ID)
            .unwrap(),
        Some(42_000)
    );
}
