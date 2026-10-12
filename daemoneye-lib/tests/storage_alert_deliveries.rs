//! `alert_deliveries` outbox: atomic enqueue, compare-and-set write-back, severity-aware cap,
//! redrive, eviction, corrupt-row tolerance, retention and dedup seeding (R9, R12, R13, R14, R16, R25).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{
    Alert, AlertSeverity, Completeness, DeliveryReason, DeliveryRecord, DeliveryState, Uuid,
};
use daemoneye_lib::storage::schema::{SchemaClassification, classify};
use daemoneye_lib::storage::{CompleteOutcome, EnqueueReport, EventStore, StorageError};
use redb::{Database, TableDefinition};
use std::path::PathBuf;
use tempfile::TempDir;

const CAP: usize = 100;

fn open() -> (TempDir, PathBuf, EventStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.redb");
    let store = EventStore::new(&path).unwrap();
    (dir, path, store)
}

fn alert(severity: AlertSeverity, title: &str) -> Alert {
    Alert::new(
        severity,
        title,
        "d",
        "rule-1",
        ProcessRecord::new(1, "p".to_owned()),
        Completeness::complete(),
    )
}

fn enqueue(store: &EventStore, alert: &Alert, sinks: &[&str], now: u64) -> EnqueueReport {
    store.enqueue_alert(alert, sinks, None, CAP, now).unwrap()
}

fn state_of(store: &EventStore, sink: &str, id: Uuid) -> DeliveryRecord {
    store.get_delivery(sink, id).unwrap().unwrap()
}

/// Claim then write a terminal row, the way a worker would.
fn finish(
    store: &EventStore,
    sink: &str,
    id: Uuid,
    now: u64,
    edit: impl FnOnce(&mut DeliveryRecord),
) {
    let claimed = store.claim(sink, id, now).unwrap();
    let mut next = claimed.record.clone();
    edit(&mut next);
    let outcome = store.complete(sink, id, claimed.expected, &next).unwrap();
    assert_eq!(outcome, CompleteOutcome::Written, "worker write-back");
}

fn dead_letter(store: &EventStore, sink: &str, id: Uuid, terminal_at: u64, redrives: u32) {
    finish(store, sink, id, terminal_at, |row| {
        row.state = DeliveryState::DeadLettered;
        row.reason = Some(DeliveryReason::AttemptsExhausted);
        row.terminal_at_ms = Some(terminal_at);
        row.redrives = redrives;
    });
}

fn mark_delivered(store: &EventStore, sink: &str, id: Uuid, at: u64) {
    finish(store, sink, id, at, |row| {
        row.state = DeliveryState::Delivered;
        row.terminal_at_ms = Some(at);
    });
}

#[test]
fn enqueue_writes_alert_and_a_pending_row_per_sink() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::Medium, "t");

    let report = enqueue(&store, &a, &["a", "b"], 100);

    assert_eq!(
        report,
        EnqueueReport::Enqueued {
            rows: vec![
                ("a".to_owned(), DeliveryState::Pending, None),
                ("b".to_owned(), DeliveryState::Pending, None),
            ]
        }
    );
    assert!(store.get_alert(&a.id.to_string()).unwrap().is_some());
    for sink in ["a", "b"] {
        let row = state_of(&store, sink, a.id);
        assert_eq!(row.state, DeliveryState::Pending);
        assert_eq!((row.attempts, row.redrives), (0, 0));
        assert_eq!(row.next_attempt_ms, 100);
        assert_eq!(row.enqueued_at_ms, 100);
        assert_eq!(row.severity, AlertSeverity::Medium);
        assert_eq!(row.deduplication_key, a.deduplication_key);
    }
}

#[test]
fn enqueue_writes_nothing_when_any_sink_key_already_exists() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::Medium, "original");
    enqueue(&store, &a, &["b"], 100);
    let mut changed = a.clone();
    changed.title = "changed".to_owned();

    let report = enqueue(&store, &changed, &["a", "b"], 200);

    assert_eq!(report, EnqueueReport::AlreadyEnqueued);
    assert!(store.get_delivery("a", a.id).unwrap().is_none());
    assert_eq!(
        store.get_alert(&a.id.to_string()).unwrap().unwrap().title,
        "original"
    );
}

/// AE9
#[test]
fn re_enqueue_of_a_delivered_alert_is_a_no_op() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::High, "t");
    enqueue(&store, &a, &["a"], 100);
    mark_delivered(&store, "a", a.id, 150);
    let before = state_of(&store, "a", a.id);

    let report = enqueue(&store, &a, &["a"], 900);

    assert_eq!(report, EnqueueReport::AlreadyEnqueued);
    let after = state_of(&store, "a", a.id);
    assert_eq!(after.state, DeliveryState::Delivered);
    assert_eq!(after, before);
    assert_eq!(store.get_all_alerts().unwrap(), vec![a]);
}

#[test]
fn dedup_hit_writes_suppressed_rows_excluded_from_open() {
    let (_dir, _path, store) = open();
    let survivor = alert(AlertSeverity::Medium, "t");
    let dup = alert(AlertSeverity::Medium, "t");
    let fresh = alert(AlertSeverity::Medium, "other");
    enqueue(&store, &survivor, &["a"], 100);

    let report = store
        .enqueue_alert(&dup, &["a"], Some(survivor.id), CAP, 200)
        .unwrap();
    enqueue(&store, &fresh, &["a"], 300);

    assert_eq!(
        report,
        EnqueueReport::Enqueued {
            rows: vec![("a".to_owned(), DeliveryState::SuppressedDuplicate, None)]
        }
    );
    let row = state_of(&store, "a", dup.id);
    assert_eq!(row.suppressed_by, Some(survivor.id));
    assert_eq!(row.terminal_at_ms, Some(200));
    assert!(row.state.is_terminal());
    let open: Vec<Uuid> = store
        .open_deliveries("a")
        .unwrap()
        .rows
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(open.contains(&fresh.id));
    assert!(open.contains(&survivor.id));
    assert!(!open.contains(&dup.id));
}

/// AE11
#[test]
fn critical_displaces_oldest_lowest_severity_pending_at_the_cap() {
    let (_dir, _path, store) = open();
    let (l1, l2, l3) = (
        alert(AlertSeverity::Low, "l1"),
        alert(AlertSeverity::Low, "l2"),
        alert(AlertSeverity::Low, "l3"),
    );
    let critical = alert(AlertSeverity::Critical, "c");
    for (a, now) in [(&l1, 100), (&l2, 200)] {
        store.enqueue_alert(a, &["s"], None, 2, now).unwrap();
    }

    let report = store
        .enqueue_alert(&critical, &["s", "t"], None, 2, 300)
        .unwrap();
    let low_report = store.enqueue_alert(&l3, &["s", "t"], None, 2, 400).unwrap();

    assert_eq!(
        report,
        EnqueueReport::Enqueued {
            rows: vec![
                ("s".to_owned(), DeliveryState::Pending, Some(l1.id)),
                ("t".to_owned(), DeliveryState::Pending, None),
            ]
        }
    );
    let displaced = state_of(&store, "s", l1.id);
    assert_eq!(displaced.state, DeliveryState::DeadLettered);
    assert_eq!(displaced.reason, Some(DeliveryReason::QueueFull));
    assert_eq!(displaced.terminal_at_ms, Some(300));
    assert_eq!(state_of(&store, "s", l2.id).state, DeliveryState::Pending);
    assert_eq!(
        state_of(&store, "s", critical.id).state,
        DeliveryState::Pending
    );

    assert_eq!(
        low_report,
        EnqueueReport::Enqueued {
            rows: vec![
                ("s".to_owned(), DeliveryState::DeadLettered, None),
                ("t".to_owned(), DeliveryState::Pending, None),
            ]
        }
    );
    let refused = state_of(&store, "s", l3.id);
    assert_eq!(refused.reason, Some(DeliveryReason::QueueFull));
    assert_eq!(refused.terminal_at_ms, Some(400));
    assert_eq!(state_of(&store, "t", l3.id).state, DeliveryState::Pending);
}

#[test]
fn high_never_displaces_an_equal_or_higher_severity_pending_row() {
    let (_dir, _path, store) = open();
    let h1 = alert(AlertSeverity::High, "h1");
    let h2 = alert(AlertSeverity::High, "h2");
    let high = alert(AlertSeverity::High, "h");
    store.enqueue_alert(&h1, &["s"], None, 2, 100).unwrap();
    store.enqueue_alert(&h2, &["s"], None, 2, 200).unwrap();

    store.enqueue_alert(&high, &["s"], None, 2, 300).unwrap();

    assert_eq!(state_of(&store, "s", h1.id).state, DeliveryState::Pending);
    assert_eq!(state_of(&store, "s", h2.id).state, DeliveryState::Pending);
    assert_eq!(
        state_of(&store, "s", high.id).reason,
        Some(DeliveryReason::QueueFull)
    );
}

/// AE10
#[test]
fn stale_write_back_is_rejected_after_a_redrive() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::Medium, "t");
    enqueue(&store, &a, &["s"], 100);
    let claimed = store.claim("s", a.id, 110).unwrap();
    assert_eq!(claimed.record.state, DeliveryState::Retrying);
    assert_eq!(claimed.record.attempts, 1);
    assert_eq!(claimed.record.next_attempt_ms, 110);
    // Another worker dead-letters the row and an operator redrives it.
    let mut dead = claimed.record.clone();
    dead.state = DeliveryState::DeadLettered;
    dead.reason = Some(DeliveryReason::Permanent);
    dead.terminal_at_ms = Some(120);
    let dead_claim = claimed.expected;
    assert_eq!(
        store.complete("s", a.id, dead_claim, &dead).unwrap(),
        CompleteOutcome::Written
    );
    assert_eq!(store.redrive_batch("s", 10, 5, 200).unwrap(), 1);

    let mut late = claimed.record.clone();
    late.state = DeliveryState::Delivered;
    late.terminal_at_ms = Some(210);
    let stale = store.complete("s", a.id, claimed.expected, &late).unwrap();

    assert_eq!(stale, CompleteOutcome::Stale);
    let row = state_of(&store, "s", a.id);
    assert_eq!(row.state, DeliveryState::Pending);
    assert_eq!((row.attempts, row.redrives), (0, 1));

    let fresh = store.claim("s", a.id, 220).unwrap();
    assert_eq!(
        store.complete("s", a.id, fresh.expected, &late).unwrap(),
        CompleteOutcome::Written
    );
    assert_eq!(state_of(&store, "s", a.id).state, DeliveryState::Delivered);
}

#[test]
fn a_sink_scan_does_not_see_rows_of_a_sink_sharing_its_name_prefix() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::Medium, "t");
    enqueue(&store, &a, &["s", "s2"], 100);

    let open_rows = store.open_deliveries("s").unwrap();

    assert_eq!(open_rows.rows.len(), 1);
    assert_eq!(open_rows.corrupt, 0);
}

#[test]
fn claim_of_a_terminal_row_is_stale() {
    let (_dir, _path, store) = open();
    let a = alert(AlertSeverity::Medium, "t");
    enqueue(&store, &a, &["s"], 100);
    mark_delivered(&store, "s", a.id, 150);

    let err = store.claim("s", a.id, 200).unwrap_err();

    assert!(matches!(err, StorageError::DeliveryStale { .. }));
}

#[test]
fn redrive_moves_only_the_oldest_dead_letters_up_to_the_limit() {
    let (_dir, _path, store) = open();
    let alerts: Vec<Alert> = (0..5)
        .map(|i| alert(AlertSeverity::Medium, &format!("t{i}")))
        .collect();
    // Terminal times are deliberately not in alert order.
    let times = [50_u64, 10, 40, 20, 30];
    for (a, t) in alerts.iter().zip(times) {
        enqueue(&store, a, &["s"], 1);
        dead_letter(&store, "s", a.id, t, 0);
    }

    let moved = store.redrive_batch("s", 2, 5, 1000).unwrap();

    assert_eq!(moved, 2);
    for idx in [1_usize, 3] {
        let row = state_of(&store, "s", alerts[idx].id);
        assert_eq!(row.state, DeliveryState::Pending);
        assert_eq!((row.attempts, row.redrives), (0, 1));
        assert_eq!(row.next_attempt_ms, 1000);
        assert_eq!(row.terminal_at_ms, None);
    }
    for idx in [0_usize, 2, 4] {
        let row = state_of(&store, "s", alerts[idx].id);
        assert_eq!(row.state, DeliveryState::DeadLettered);
        assert_eq!(row.redrives, 0);
    }
    assert_eq!(store.dead_letters("s").unwrap().len(), 3);
}

#[test]
fn redrive_skips_a_row_at_the_cap_and_marks_it_exhausted() {
    let (_dir, _path, store) = open();
    let spent = alert(AlertSeverity::Medium, "spent");
    let fresh = alert(AlertSeverity::Medium, "fresh");
    enqueue(&store, &spent, &["s"], 1);
    enqueue(&store, &fresh, &["s"], 1);
    dead_letter(&store, "s", spent.id, 10, 5);
    dead_letter(&store, "s", fresh.id, 20, 0);

    let moved = store.redrive_batch("s", 1, 5, 1000).unwrap();

    assert_eq!(
        moved, 1,
        "an exhausted row must not consume the batch limit"
    );
    let row = state_of(&store, "s", spent.id);
    assert_eq!(row.state, DeliveryState::DeadLettered);
    assert_eq!(row.reason, Some(DeliveryReason::RedrivesExhausted));
    assert_eq!(row.redrives, 5);
    assert_eq!(
        state_of(&store, "s", fresh.id).state,
        DeliveryState::Pending
    );
}

#[test]
fn evict_removes_lowest_severity_then_oldest_and_never_a_pending_row() {
    let (_dir, _path, store) = open();
    let specs = [
        (AlertSeverity::Low, 10_u64),
        (AlertSeverity::Low, 20),
        (AlertSeverity::Medium, 5),
        (AlertSeverity::High, 1),
        (AlertSeverity::Critical, 2),
    ];
    let alerts: Vec<Alert> = specs
        .iter()
        .enumerate()
        .map(|(i, spec)| alert(spec.0, &format!("t{i}")))
        .collect();
    for (a, (_, t)) in alerts.iter().zip(specs) {
        enqueue(&store, a, &["s"], 1);
        dead_letter(&store, "s", a.id, t, 0);
    }
    let pending = alert(AlertSeverity::Low, "pending");
    enqueue(&store, &pending, &["s"], 1);

    let removed = store.evict_dead_letters("s", 2).unwrap();

    assert_eq!(removed, 3);
    for evicted in alerts.iter().take(3) {
        assert!(store.get_delivery("s", evicted.id).unwrap().is_none());
    }
    for kept in alerts.iter().skip(3) {
        assert!(store.get_delivery("s", kept.id).unwrap().is_some());
    }
    assert_eq!(
        state_of(&store, "s", pending.id).state,
        DeliveryState::Pending
    );
}

const DELIVERIES: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("alert_deliveries");

#[test]
fn undecodable_row_is_counted_and_good_rows_still_return() {
    let (_dir, path, store) = open();
    let a = alert(AlertSeverity::Medium, "a");
    let b = alert(AlertSeverity::Medium, "b");
    enqueue(&store, &a, &["s"], 1);
    enqueue(&store, &b, &["s"], 2);
    drop(store);
    let db = Database::open(&path).unwrap();
    let txn = db.begin_write().unwrap();
    let mut table = txn.open_table(DELIVERIES).unwrap();
    let key = format!("s\u{1f}{}", Uuid::new_v4());
    table
        .insert(key.as_str(), [0xFF_u8, 1, 2].as_slice())
        .unwrap();
    drop(table);
    txn.commit().unwrap();
    drop(db);
    let reopened = EventStore::open(&path).unwrap();

    let open_rows = reopened.open_deliveries("s").unwrap();

    assert_eq!(open_rows.corrupt, 1);
    let mut ids: Vec<Uuid> = open_rows.rows.iter().map(|entry| entry.0).collect();
    ids.sort();
    let mut expected = vec![a.id, b.id];
    expected.sort();
    assert_eq!(ids, expected);
}

#[test]
fn a_store_without_the_table_reads_empty_and_still_matches_the_schema() {
    let (_dir, path, store) = open();

    let open_rows = store.open_deliveries("s").unwrap();
    assert!(open_rows.rows.is_empty());
    assert_eq!(open_rows.corrupt, 0);
    assert!(store.dead_letters("s").unwrap().is_empty());
    assert!(store.seed_dedup_window(1_000, 500).unwrap().is_empty());
    drop(store);

    let db = Database::open(&path).unwrap();
    assert_eq!(classify(&db).unwrap(), SchemaClassification::Match);
}

#[test]
fn prune_removes_old_terminal_rows_only_and_leaves_alerts() {
    let (_dir, _path, store) = open();
    let old = alert(AlertSeverity::Medium, "old");
    let recent = alert(AlertSeverity::Medium, "recent");
    let retrying = alert(AlertSeverity::Medium, "retrying");
    for a in [&old, &recent, &retrying] {
        enqueue(&store, a, &["s"], 1);
    }
    mark_delivered(&store, "s", old.id, 100);
    dead_letter(&store, "s", recent.id, 900, 0);
    // A retry after a redrive can still carry an old terminal time; it must survive anyway.
    let claimed = store.claim("s", retrying.id, 2).unwrap();
    let mut aged = claimed.record.clone();
    aged.terminal_at_ms = Some(50);
    assert_eq!(
        store
            .complete("s", retrying.id, claimed.expected, &aged)
            .unwrap(),
        CompleteOutcome::Written,
        "retrying row with an old terminal time"
    );
    let alerts_before = store.get_all_alerts().unwrap().len();

    let pruned = store.prune_deliveries(1_000, 500).unwrap();

    assert_eq!(pruned, 1);
    assert!(store.get_delivery("s", old.id).unwrap().is_none());
    assert!(store.get_delivery("s", recent.id).unwrap().is_some());
    assert_eq!(
        state_of(&store, "s", retrying.id).state,
        DeliveryState::Retrying
    );
    assert_eq!(store.get_all_alerts().unwrap().len(), alerts_before);
}

#[test]
fn seed_returns_keys_with_a_live_row_and_skips_all_suppressed_and_stale_alerts() {
    let (_dir, _path, store) = open();
    let delivered = alert(AlertSeverity::Medium, "delivered");
    let suppressed = alert(AlertSeverity::Medium, "suppressed");
    let outside = alert(AlertSeverity::Medium, "outside");
    enqueue(&store, &delivered, &["a", "b"], 900);
    mark_delivered(&store, "a", delivered.id, 910);
    store
        .enqueue_alert(&suppressed, &["a", "b"], Some(delivered.id), CAP, 950)
        .unwrap();
    enqueue(&store, &outside, &["a"], 100);

    let seeded = store.seed_dedup_window(1_000, 500).unwrap();

    assert_eq!(seeded, vec![(delivered.deduplication_key, 900)]);
}
