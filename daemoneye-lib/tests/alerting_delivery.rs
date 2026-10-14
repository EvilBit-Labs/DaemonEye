//! Sink workers, the manager and the dedup window over a real store, under paused time.
//!
//! Every assertion reads rows back from the store; the fake sink's log is only a second witness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::missing_assert_message,
    clippy::semicolon_outside_block,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::clone_on_ref_ptr
)]
#![cfg(feature = "alerting")]

#[path = "alerting_delivery/fixtures.rs"]
mod fixtures;

use daemoneye_lib::alerting::{
    AlertSinkFactory, BreakerState, CredentialResolver, DeliveryAggregate, DeliveryErrorClass,
    EnqueueOutcome, REDRIVE_BATCH, SinkBuilder,
};
use daemoneye_lib::config::{AlertSinkConfig, AlertingConfig, DeliveryConfig};
use daemoneye_lib::models::{AlertSeverity, DeliveryReason, DeliveryState, Uuid};
use daemoneye_lib::storage::{CompleteOutcome, EventStore};
use fixtures::{
    Act, CountingPublisher, Fake, StuckPublisher, alert, alert_keyed, config, now_ms, rig,
    rig_over, rig_publishing, row, run_for, settle, temp_store,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const HIGH: AlertSeverity = AlertSeverity::High;

// ---- retry, breaker, classification -------------------------------------------------------

/// AE1: a transient sink gets exactly `max_attempts`, then the row is dead-lettered, and the
/// gaps follow full-jitter backoff.
#[tokio::test(start_paused = true)]
async fn a_transient_sink_is_attempted_max_attempts_times_with_jittered_gaps() {
    let net = Fake::new("net", false, Act::Transient);
    let r = rig(
        vec![net.boxed()],
        config(|c| {
            c.delivery.max_attempts = 3;
            c.delivery.initial_backoff_ms = 1_000;
            c.delivery.max_backoff_secs = 60;
        }),
    )
    .await;
    let alerts: Vec<_> = (0..20).map(|i| alert(HIGH, &format!("a{i}"))).collect();
    for a in &alerts {
        assert_eq!(r.manager.enqueue(a).await, EnqueueOutcome::Enqueued);
    }

    run_for(60).await;

    let log = net.log();
    let (mut first_gaps, mut second_gaps) = (Vec::new(), Vec::new());
    for a in &alerts {
        let stored = row(&r.store, "net", a.id);
        assert_eq!(stored.state, DeliveryState::DeadLettered);
        assert_eq!(stored.reason, Some(DeliveryReason::AttemptsExhausted));
        assert_eq!(stored.attempts, 3);
        assert_eq!(stored.last_error, Some(DeliveryErrorClass::HttpStatus(503)));
        let times: Vec<_> = log.iter().filter(|e| e.id == a.id).map(|e| e.at).collect();
        assert_eq!(times.len(), 3, "exactly max_attempts sends");
        first_gaps.push(times[1] - times[0]);
        second_gaps.push(times[2] - times[1]);
    }
    let health = &r.manager.health().sinks[0];
    assert_eq!(
        (health.attempts, health.delivered, health.dead_letters),
        (60, 0, 20)
    );
    assert_eq!(health.success_rate.to_bits(), 0.0_f64.to_bits());
    assert_eq!(health.last_error, Some(DeliveryErrorClass::HttpStatus(503)));
    let slack = Duration::from_millis(5);
    assert!(
        first_gaps
            .iter()
            .all(|g| *g <= Duration::from_secs(1) + slack)
    );
    assert!(
        second_gaps
            .iter()
            .all(|g| *g <= Duration::from_secs(2) + slack)
    );
    assert!(
        first_gaps.iter().max().unwrap() > &Duration::from_millis(500),
        "jitter spans the range"
    );
    assert!(
        second_gaps.iter().max().unwrap() > &Duration::from_secs(1),
        "the ceiling doubles"
    );
}

/// AE2: failures open the breaker, rows behind it wait, one probe goes out after the cool-down,
/// and dead letters are redriven only after the breaker stayed closed a full cool-down.
#[tokio::test(start_paused = true)]
async fn the_breaker_holds_rows_probes_once_and_redrives_after_a_full_cooldown() {
    let net = Fake::new("net", false, Act::Ok);
    net.script(&[Act::Transient, Act::Transient]);
    let r = rig(
        vec![net.boxed()],
        config(|c| {
            c.delivery.max_attempts = 1;
            c.delivery.breaker_failure_threshold = 2;
            c.delivery.breaker_cooldown_secs = 30;
        }),
    )
    .await;
    let (a, b, c) = (alert(HIGH, "a"), alert(HIGH, "b"), alert(HIGH, "c"));
    for x in [&a, &b, &c] {
        r.manager.enqueue(x).await;
        settle().await;
    }

    for x in [&a, &b] {
        assert_eq!(
            row(&r.store, "net", x.id).state,
            DeliveryState::DeadLettered
        );
    }
    let held = row(&r.store, "net", c.id);
    assert_eq!((held.state, held.attempts), (DeliveryState::Pending, 0));
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);
    assert!(r.manager.health().sinks[0].oldest_pending_age_ms.is_some());
    assert_eq!(net.attempts(), 2);

    run_for(36).await;

    assert_eq!(net.attempts(), 3, "exactly one probe");
    assert_eq!(row(&r.store, "net", c.id).state, DeliveryState::Delivered);
    for x in [&a, &b] {
        let dead = row(&r.store, "net", x.id);
        assert_eq!(
            (dead.state, dead.redrives),
            (DeliveryState::DeadLettered, 0)
        );
    }

    run_for(40).await;

    for x in [&a, &b] {
        let back = row(&r.store, "net", x.id);
        assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
    }
}

/// AE3: a misconfigured sink dead-letters the row, opens the breaker, and holds what follows.
#[tokio::test(start_paused = true)]
async fn a_misconfigured_outcome_opens_the_breaker_at_once() {
    let net = Fake::new("net", false, Act::Misconfigured);
    let r = rig(
        vec![net.boxed()],
        config(|c| c.delivery.breaker_failure_threshold = 5),
    )
    .await;
    let (a, b) = (alert(HIGH, "a"), alert(HIGH, "b"));
    r.manager.enqueue(&a).await;
    settle().await;
    r.manager.enqueue(&b).await;
    settle().await;

    let first = row(&r.store, "net", a.id);
    assert_eq!(first.state, DeliveryState::DeadLettered);
    assert_eq!(first.reason, Some(DeliveryReason::Misconfigured));
    assert_eq!(first.attempts, 1);
    let second = row(&r.store, "net", b.id);
    assert_eq!((second.state, second.attempts), (DeliveryState::Pending, 0));
    let health = r.manager.health();
    assert_eq!(health.sinks[0].breaker, BreakerState::Open);
    assert!(health.sinks[0].misconfigured);
    assert_eq!(health.aggregate, DeliveryAggregate::Degraded);
    assert_eq!(net.attempts(), 1);
}

/// A permanent rejection dead-letters without counting toward the breaker; a local sink is never
/// held back by one, and a failing local sink makes the aggregate unhealthy.
#[tokio::test(start_paused = true)]
async fn permanent_failures_skip_the_breaker_and_local_sinks_ignore_it() {
    let net = Fake::new("net", false, Act::Permanent);
    let local = Fake::new("local", true, Act::Transient);
    let r = rig(
        vec![net.boxed(), local.boxed()],
        config(|c| {
            c.delivery.max_attempts = 1;
            c.delivery.breaker_failure_threshold = 2;
        }),
    )
    .await;
    let alerts: Vec<_> = (0..4).map(|i| alert(HIGH, &format!("p{i}"))).collect();
    for a in &alerts {
        r.manager.enqueue(a).await;
        settle().await;
    }

    for a in &alerts {
        let remote = row(&r.store, "net", a.id);
        assert_eq!(remote.reason, Some(DeliveryReason::Permanent));
        assert_eq!(remote.attempts, 1);
        assert_eq!(
            row(&r.store, "local", a.id).attempts,
            1,
            "never held by a breaker"
        );
    }
    let health = r.manager.health();
    assert!(
        health
            .sinks
            .iter()
            .all(|s| s.breaker == BreakerState::Closed)
    );
    assert_eq!(health.aggregate, DeliveryAggregate::Unhealthy);
}

/// R17: a row whose alert is gone is dead-lettered as `AlertMissing`.
#[tokio::test(start_paused = true)]
async fn a_row_without_its_alert_is_dead_lettered_as_missing() {
    let (dir, store) = temp_store();
    let path = dir.path().join("events.redb");
    let lost = alert(HIGH, "lost");
    store
        .enqueue_alert(&lost, &["net"], None, 100, now_ms())
        .unwrap();
    drop(store);
    {
        let db = redb::Database::open(&path).unwrap();
        let write = db.begin_write().unwrap();
        let alerts: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("alerts");
        write
            .open_table(alerts)
            .unwrap()
            .remove(lost.id.to_string().as_str())
            .unwrap();
        write.commit().unwrap();
    }
    let store = Arc::new(EventStore::open(&path).unwrap());
    let net = Fake::new("net", false, Act::Ok);

    let r = rig_over(store, vec![net.boxed()], config(|_| {})).await;
    settle().await;

    let stored = row(&r.store, "net", lost.id);
    assert_eq!(stored.state, DeliveryState::DeadLettered);
    assert_eq!(stored.reason, Some(DeliveryReason::AlertMissing));
    assert_eq!(net.attempts(), 0, "nothing to send");
}

// ---- dedup window -------------------------------------------------------------------------

/// AE6: the window is measured from the first alert and a suppressed duplicate does not extend it.
#[tokio::test(start_paused = true)]
async fn a_suppressed_duplicate_does_not_refresh_the_window() {
    let net = Fake::new("net", false, Act::Ok);
    let r = rig(vec![net.boxed()], config(|c| c.dedup_window_seconds = 300)).await;
    let (first, second, third) = (
        alert_keyed(HIGH, "k", "K"),
        alert_keyed(HIGH, "k", "K"),
        alert_keyed(HIGH, "k", "K"),
    );
    assert_eq!(r.manager.enqueue(&first).await, EnqueueOutcome::Enqueued);
    run_for(10).await;

    assert_eq!(
        r.manager.enqueue(&second).await,
        EnqueueOutcome::SuppressedDuplicate { by: first.id }
    );
    let suppressed = row(&r.store, "net", second.id);
    assert_eq!(suppressed.state, DeliveryState::SuppressedDuplicate);
    assert_eq!(suppressed.suppressed_by, Some(first.id));

    run_for(291).await;

    assert_eq!(r.manager.enqueue(&third).await, EnqueueOutcome::Enqueued);
    settle().await;
    let sent: Vec<Uuid> = net.log().iter().map(|e| e.id).collect();
    assert_eq!(
        sent,
        vec![first.id, third.id],
        "the duplicate was never sent"
    );
}

/// AE6: a redrive does not consult the window.
#[tokio::test(start_paused = true)]
async fn a_redriven_row_is_delivered_even_while_its_key_is_in_the_window() {
    let net = Fake::new("net", false, Act::Permanent);
    let r = rig(vec![net.boxed()], config(|_| {})).await;
    let first = alert_keyed(HIGH, "k", "K");
    r.manager.enqueue(&first).await;
    settle().await;
    assert_eq!(
        row(&r.store, "net", first.id).state,
        DeliveryState::DeadLettered
    );
    net.set_default(Act::Ok);
    let duplicate = alert_keyed(HIGH, "k", "K");
    assert!(matches!(
        r.manager.enqueue(&duplicate).await,
        EnqueueOutcome::SuppressedDuplicate { .. }
    ));

    assert_eq!(r.manager.redrive("net").await, 1);
    settle().await;

    let back = row(&r.store, "net", first.id);
    assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
}

/// The same rule with different key suffixes is two alerts.
#[tokio::test(start_paused = true)]
async fn alerts_with_different_keys_are_both_enqueued() {
    let r = rig(
        vec![Fake::new("net", false, Act::Ok).boxed()],
        config(|_| {}),
    )
    .await;
    let one = alert_keyed(HIGH, "t", "rule:1");
    let two = alert_keyed(HIGH, "t", "rule:2");

    assert_eq!(r.manager.enqueue(&one).await, EnqueueOutcome::Enqueued);
    assert_eq!(r.manager.enqueue(&two).await, EnqueueOutcome::Enqueued);
}

/// What a started manager does with a new `K` alert over a store holding an older one.
async fn outcome_over_history(age_ms: u64, suppressed_rows: bool) -> (EnqueueOutcome, Uuid) {
    let (_dir, store) = temp_store();
    let old = alert_keyed(HIGH, "old", "K");
    let at = now_ms() - age_ms;
    let survivor = suppressed_rows.then(Uuid::new_v4);
    store
        .enqueue_alert(&old, &["net"], survivor, 100, at)
        .unwrap();
    if survivor.is_none() {
        let claimed = store.claim("net", old.id, at).unwrap();
        let mut done = claimed.record.clone();
        done.state = DeliveryState::Delivered;
        done.terminal_at_ms = Some(at);
        let written = store
            .complete("net", old.id, claimed.expected, &done)
            .unwrap();
        assert_eq!(written, CompleteOutcome::Written);
    }
    let r = rig_over(
        store,
        vec![Fake::new("net", false, Act::Ok).boxed()],
        config(|_| {}),
    )
    .await;
    (
        r.manager.enqueue(&alert_keyed(HIGH, "new", "K")).await,
        old.id,
    )
}

/// The window is seeded from the store at start; stale and all-suppressed history does not seed it.
#[tokio::test(start_paused = true)]
async fn the_window_is_seeded_from_recent_delivered_history_only() {
    let (seeded, old_id) = outcome_over_history(60_000, false).await;
    assert_eq!(seeded, EnqueueOutcome::SuppressedDuplicate { by: old_id });

    let (stale, _) = outcome_over_history(400_000, false).await;
    assert_eq!(stale, EnqueueOutcome::Enqueued, "outside the 300 s window");

    let (suppressed_only, _) = outcome_over_history(60_000, true).await;
    assert_eq!(
        suppressed_only,
        EnqueueOutcome::Enqueued,
        "only suppressed rows"
    );
}

/// A store that cannot take the write: the alert is not persisted, not announced, and its key
/// does not enter the window.
#[tokio::test(start_paused = true)]
async fn a_failed_enqueue_is_not_persisted_and_does_not_hold_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.redb");
    drop(EventStore::new(&path).unwrap());
    {
        let db = redb::Database::open(&path).unwrap();
        let write = db.begin_write().unwrap();
        let real: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("alerts");
        write.delete_table(real).unwrap();
        let wrong: redb::TableDefinition<u64, u64> = redb::TableDefinition::new("alerts");
        write.open_table(wrong).unwrap();
        write.commit().unwrap();
    }
    let store = Arc::new(EventStore::open(&path).unwrap());
    let net = Fake::new("net", false, Act::Ok);
    let bus = CountingPublisher::new(false);
    let r = rig_publishing(store, vec![net.boxed()], config(|_| {}), bus.clone()).await;

    let first = alert_keyed(HIGH, "k", "K");
    assert_eq!(
        r.manager.enqueue(&first).await,
        EnqueueOutcome::NotPersisted
    );
    let again = alert_keyed(HIGH, "k", "K");
    assert_eq!(
        r.manager.enqueue(&again).await,
        EnqueueOutcome::NotPersisted
    );

    let health = r.manager.health();
    assert_eq!(health.store_failures, 2);
    assert_eq!(health.aggregate, DeliveryAggregate::Unhealthy);
    assert_eq!(bus.count(), 0);
    assert_eq!(net.attempts(), 0);
}

/// A publisher failure is counted and never fails the delivery.
#[tokio::test(start_paused = true)]
async fn a_publish_failure_is_counted_and_the_alert_still_delivers() {
    let (_dir, store) = temp_store();
    let net = Fake::new("net", false, Act::Ok);
    let bus = CountingPublisher::new(true);
    let r = rig_publishing(store, vec![net.boxed()], config(|_| {}), bus.clone()).await;
    let a = alert(HIGH, "a");

    assert_eq!(r.manager.enqueue(&a).await, EnqueueOutcome::Enqueued);
    settle().await;

    assert_eq!(bus.count(), 1);
    assert_eq!(r.manager.health().publish_failures, 1);
    assert_eq!(row(&r.store, "net", a.id).state, DeliveryState::Delivered);
}

/// ADR-0015: a stuck publisher must not hold the enqueue; the row still delivers.
#[tokio::test(start_paused = true)]
async fn enqueue_returns_while_the_publisher_is_stuck_and_the_row_delivers() {
    let (_dir, store) = temp_store();
    let net = Fake::new("net", false, Act::Ok);
    let r = rig_publishing(
        store,
        vec![net.boxed()],
        config(|_| {}),
        Arc::new(StuckPublisher),
    )
    .await;

    for i in 0..3 {
        let a = alert(HIGH, &format!("a{i}"));
        let outcome = tokio::time::timeout(Duration::from_secs(5), r.manager.enqueue(&a))
            .await
            .expect("enqueue waited on the publisher");
        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        settle().await;
        assert_eq!(row(&r.store, "net", a.id).state, DeliveryState::Delivered);
    }
}

/// R13: unpersisted delivery reaches local sinks only, marked, and never touches the window.
#[tokio::test(start_paused = true)]
async fn unpersisted_delivery_goes_to_local_sinks_only_and_skips_the_window() {
    let local = Fake::new("local", true, Act::Ok);
    let net = Fake::new("net", false, Act::Ok);
    let r = rig(vec![local.boxed(), net.boxed()], config(|_| {})).await;
    let a = alert_keyed(HIGH, "k", "K");

    r.manager.deliver_unpersisted(&a).await;

    assert_eq!(local.attempts(), 1);
    assert!(local.log()[0].marked_unpersisted);
    assert_eq!(net.attempts(), 0);
    assert!(r.store.get_alert(&a.id.to_string()).unwrap().is_none());
    let next = alert_keyed(HIGH, "k", "K");
    assert_eq!(r.manager.enqueue(&next).await, EnqueueOutcome::Enqueued);
}

// ---- crash safety, concurrency ------------------------------------------------------------

/// AE5 and shutdown: an in-flight send is aborted at the drain bound, the attempt stays counted
/// on a `Retrying` row, and a new manager over the same store resends the same alert.
#[tokio::test(start_paused = true)]
async fn shutdown_leaves_the_in_flight_row_retrying_and_a_new_manager_resumes_it() {
    let net = Fake::new("net", false, Act::Hang);
    let r = rig(vec![net.boxed()], config(|_| {})).await;
    let a = alert(HIGH, "a");
    r.manager.enqueue(&a).await;
    settle().await;
    let claimed = row(&r.store, "net", a.id);
    assert_eq!(
        (claimed.state, claimed.attempts),
        (DeliveryState::Retrying, 1)
    );

    let drain = Duration::from_millis(100);
    let began = tokio::time::Instant::now();
    r.manager.shutdown(drain).await;
    assert!(began.elapsed() <= drain + Duration::from_millis(5));

    let interrupted = row(&r.store, "net", a.id);
    assert_eq!(
        (interrupted.state, interrupted.attempts),
        (DeliveryState::Retrying, 1)
    );
    let again = Fake::new("net", false, Act::Ok);
    let resumed = rig_over(Arc::clone(&r.store), vec![again.boxed()], config(|_| {})).await;
    settle().await;

    let done = row(&resumed.store, "net", a.id);
    assert_eq!((done.state, done.attempts), (DeliveryState::Delivered, 2));
    assert_eq!(again.log()[0].id, a.id);
    let health = &resumed.manager.health().sinks[0];
    assert_eq!((health.delivered, health.attempts), (1, 1));
    assert!(health.last_success_ms.is_some());
    assert_eq!(health.success_rate.to_bits(), 1.0_f64.to_bits());
}

/// Dead letters left by an earlier run are redriven once at start.
#[tokio::test(start_paused = true)]
async fn dead_letters_from_an_earlier_run_are_redriven_at_start() {
    let (_dir, store) = temp_store();
    let ids: Vec<Uuid> = (0..3)
        .map(|i| {
            let a = alert(HIGH, &format!("old{i}"));
            let now = now_ms();
            store.enqueue_alert(&a, &["net"], None, 100, now).unwrap();
            let claimed = store.claim("net", a.id, now).unwrap();
            let mut dead = claimed.record.clone();
            dead.state = DeliveryState::DeadLettered;
            dead.reason = Some(DeliveryReason::AttemptsExhausted);
            dead.terminal_at_ms = Some(now);
            store
                .complete("net", a.id, claimed.expected, &dead)
                .unwrap();
            a.id
        })
        .collect();
    let net = Fake::new("net", false, Act::Ok);

    let r = rig_over(store, vec![net.boxed()], config(|_| {})).await;
    settle().await;

    for id in ids {
        let back = row(&r.store, "net", id);
        assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
    }
}

/// The caller's cancellation stops the workers: later rows are not attempted.
#[tokio::test(start_paused = true)]
async fn the_external_cancellation_stops_delivery() {
    let net = Fake::new("net", false, Act::Ok);
    let r = rig(vec![net.boxed()], config(|_| {})).await;
    r.cancel.send(true).unwrap();
    settle().await;
    let a = alert(HIGH, "a");

    r.manager.enqueue(&a).await;
    run_for(20).await;

    let waiting = row(&r.store, "net", a.id);
    assert_eq!(
        (waiting.state, waiting.attempts),
        (DeliveryState::Pending, 0)
    );
}

/// AE10: a redrive while a send is in flight wins; the worker's late write-back is dropped.
#[tokio::test(start_paused = true)]
async fn a_redrive_during_a_send_beats_the_workers_write_back() {
    let net = Fake::new("net", false, Act::Ok);
    net.script(&[Act::Hang]);
    let r = rig(
        vec![net.boxed()],
        config(|c| c.delivery.attempt_timeout_secs = 10),
    )
    .await;
    let a = alert(HIGH, "a");
    r.manager.enqueue(&a).await;
    settle().await;
    let claimed = row(&r.store, "net", a.id);
    assert_eq!(
        (claimed.state, claimed.attempts, claimed.redrives),
        (DeliveryState::Retrying, 1, 0)
    );
    let mut dead = claimed.clone();
    dead.state = DeliveryState::DeadLettered;
    dead.reason = Some(DeliveryReason::Permanent);
    dead.terminal_at_ms = Some(now_ms());
    let expected = (claimed.state, claimed.attempts, claimed.redrives);
    assert_eq!(
        r.store.complete("net", a.id, expected, &dead).unwrap(),
        CompleteOutcome::Written
    );

    assert_eq!(r.manager.redrive("net").await, 1);

    let redriven = row(&r.store, "net", a.id);
    assert_eq!(
        (redriven.state, redriven.attempts, redriven.redrives),
        (DeliveryState::Pending, 0, 1)
    );

    run_for(15).await;

    let done = row(&r.store, "net", a.id);
    assert_eq!(
        (done.state, done.attempts, done.redrives),
        (DeliveryState::Delivered, 1, 1)
    );
    assert_eq!(net.attempts(), 2, "the hung send and the redriven one");
}

/// A hung sink costs one attempt per timeout and never delays a local sink.
#[tokio::test(start_paused = true)]
async fn a_hung_sink_does_not_delay_another_sink() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("alerts.log");
    let hung = Fake::new("hung", false, Act::Hang);
    let file = daemoneye_lib::alerting::FileSink::new(
        "file".to_owned(),
        file_path.clone(),
        daemoneye_lib::alerting::OutputFormat::Json,
    );
    let r = rig(
        vec![hung.boxed(), Box::new(file)],
        config(|c| c.delivery.attempt_timeout_secs = 10),
    )
    .await;
    let a = alert(HIGH, "a");
    r.manager.enqueue(&a).await;
    settle().await;

    let delivered = row(&r.store, "file", a.id);
    assert_eq!(delivered.state, DeliveryState::Delivered);
    assert!(
        delivered.terminal_at_ms.unwrap() - delivered.enqueued_at_ms < 1_000,
        "within one tick"
    );
    assert_eq!(hung.attempts(), 1, "still inside its first attempt");
    assert!(
        std::fs::read_to_string(&file_path)
            .unwrap()
            .contains(&a.id.to_string())
    );

    run_for(25).await;

    assert!(
        hung.attempts() <= 3,
        "one attempt per timeout, bounded by max_attempts"
    );
    assert!(hung.attempts() >= 2);
}

// ---- pacing, caps, eviction, clamp --------------------------------------------------------

/// R15: the rate limit paces delivery; nothing is dropped.
#[tokio::test(start_paused = true)]
async fn the_rate_limit_paces_and_drops_nothing() {
    let net = Fake::new("net", false, Act::Ok);
    let r = rig(
        vec![net.boxed()],
        config(|c| c.max_alerts_per_minute = Some(2)),
    )
    .await;
    let alerts: Vec<_> = (0..3).map(|i| alert(HIGH, &format!("r{i}"))).collect();
    for a in &alerts {
        r.manager.enqueue(a).await;
    }
    settle().await;

    assert_eq!(net.attempts(), 2, "the bucket starts full");
    run_for(15).await;
    assert_eq!(net.attempts(), 2, "the third waits for a token");

    run_for(20).await;

    assert_eq!(net.attempts(), 3);
    for a in &alerts {
        assert_eq!(row(&r.store, "net", a.id).state, DeliveryState::Delivered);
    }
}

/// R25: the open-row cap refuses a Low row, a Critical displaces the oldest Low, health counts both.
#[tokio::test(start_paused = true)]
async fn the_open_row_cap_refuses_low_rows_and_a_critical_displaces_one() {
    let dead = Fake::new("dead", false, Act::Hang);
    let live = Fake::new("live", false, Act::Ok);
    let r = rig(
        vec![dead.boxed(), live.boxed()],
        config(|c| {
            c.delivery.max_open_rows_per_sink = 2;
            c.delivery.dlq_max_rows_per_sink = 10;
        }),
    )
    .await;
    let (l1, l2, l3) = (
        alert(AlertSeverity::Low, "l1"),
        alert(AlertSeverity::Low, "l2"),
        alert(AlertSeverity::Low, "l3"),
    );
    for a in [&l1, &l2] {
        r.manager.enqueue(a).await;
        settle().await;
    }

    r.manager.enqueue(&l3).await;
    let refused = row(&r.store, "dead", l3.id);
    assert_eq!(
        (refused.state, refused.reason),
        (DeliveryState::DeadLettered, Some(DeliveryReason::QueueFull))
    );
    assert_eq!(row(&r.store, "live", l3.id).state, DeliveryState::Pending);

    let critical = alert(AlertSeverity::Critical, "c");
    r.manager.enqueue(&critical).await;

    let displaced = row(&r.store, "dead", l2.id);
    assert_eq!(
        (displaced.state, displaced.reason),
        (DeliveryState::DeadLettered, Some(DeliveryReason::QueueFull))
    );
    assert_eq!(
        row(&r.store, "dead", critical.id).state,
        DeliveryState::Pending
    );
    let by_name: HashMap<_, _> = r
        .manager
        .health()
        .sinks
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    assert_eq!(by_name["dead"].queue_full, 2);
    assert_eq!(by_name["live"].queue_full, 0);
}

/// R9: eviction keeps the cap, drops the lowest severity first, and never touches open rows.
#[tokio::test(start_paused = true)]
async fn eviction_keeps_the_cap_drops_low_severity_and_spares_open_rows() {
    let net = Fake::new("net", false, Act::Permanent);
    let r = rig(
        vec![net.boxed()],
        config(|c| c.delivery.dlq_max_rows_per_sink = 10),
    )
    .await;
    let highs: Vec<_> = (0..10).map(|i| alert(HIGH, &format!("h{i}"))).collect();
    let lows: Vec<_> = (0..2)
        .map(|i| alert(AlertSeverity::Low, &format!("l{i}")))
        .collect();
    for a in highs.iter().chain(&lows) {
        r.manager.enqueue(a).await;
    }
    run_for(1).await;
    net.set_default(Act::Hang);
    let open: Vec<_> = (0..2).map(|i| alert(HIGH, &format!("o{i}"))).collect();
    for a in &open {
        r.manager.enqueue(a).await;
    }
    run_for(1).await;

    let kept: Vec<Uuid> = r
        .manager
        .dead_letters("net")
        .await
        .into_iter()
        .map(|d| d.0)
        .collect();
    assert_eq!(kept.len(), 10);
    assert!(highs.iter().all(|a| kept.contains(&a.id)));
    assert!(
        lows.iter()
            .all(|a| r.store.get_delivery("net", a.id).unwrap().is_none())
    );
    assert_eq!(r.manager.health().sinks[0].evicted, 2);
    for a in &open {
        assert!(
            !row(&r.store, "net", a.id).state.is_terminal(),
            "open rows survive"
        );
    }
}

/// A far-future `next_attempt_ms` is clamped to the backoff cap; a near one is honoured.
#[tokio::test(start_paused = true)]
async fn a_retry_scheduled_beyond_the_cap_is_claimed_at_once() {
    let (_dir, store) = temp_store();
    let (far, near) = (alert(HIGH, "far"), alert(HIGH, "near"));
    for (a, ahead_ms) in [(&far, 3_600_000_u64), (&near, 30_000)] {
        let now = now_ms();
        store.enqueue_alert(a, &["net"], None, 100, now).unwrap();
        let claimed = store.claim("net", a.id, now).unwrap();
        let mut scheduled = claimed.record.clone();
        scheduled.next_attempt_ms = now + ahead_ms;
        store
            .complete("net", a.id, claimed.expected, &scheduled)
            .unwrap();
    }
    let net = Fake::new("net", false, Act::Ok);

    let r = rig_over(
        store,
        vec![net.boxed()],
        config(|c| c.delivery.max_backoff_secs = 60),
    )
    .await;
    settle().await;

    assert_eq!(row(&r.store, "net", far.id).state, DeliveryState::Delivered);
    assert_eq!(
        row(&r.store, "net", near.id).state,
        DeliveryState::Retrying,
        "30 s is inside the cap"
    );

    run_for(31).await;

    assert_eq!(
        row(&r.store, "net", near.id).state,
        DeliveryState::Delivered
    );
}

// ---- redrive batching ---------------------------------------------------------------------

/// 300 dead letters on a sink whose breaker then trips once and recovers: two transient
/// failures open it, the probe and the next send succeed, and `after` is the default from then on.
async fn three_hundred_dead_letters_then_recovery(net: &Arc<Fake>, after: Act) -> fixtures::Rig {
    let r = rig(
        vec![net.boxed()],
        config(|c| {
            c.delivery.breaker_failure_threshold = 2;
            c.delivery.breaker_cooldown_secs = 30;
            c.delivery.dlq_max_rows_per_sink = 1_000;
        }),
    )
    .await;
    for i in 0..300 {
        r.manager.enqueue(&alert(HIGH, &format!("d{i}"))).await;
    }
    run_for(1).await;
    assert_eq!(r.manager.dead_letters("net").await.len(), 300);
    net.script(&[Act::Transient, Act::Transient, Act::Ok, Act::Ok]);
    net.set_default(after);
    for i in 0..2 {
        r.manager.enqueue(&alert(HIGH, &format!("t{i}"))).await;
    }
    run_for(1).await;
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);
    r
}

fn redriven(store: &EventStore, ids: &[Uuid]) -> usize {
    ids.iter()
        .filter(|id| store.get_delivery("net", **id).unwrap().unwrap().redrives == 1)
        .count()
}

/// R9: a sink that fails again after recovery has had at most one batch redriven.
#[tokio::test(start_paused = true)]
async fn a_flapping_sink_sees_at_most_one_redrive_batch() {
    let net = Fake::new("net", false, Act::Permanent);
    let r = three_hundred_dead_letters_then_recovery(&net, Act::Transient).await;
    let ids: Vec<Uuid> = r
        .manager
        .dead_letters("net")
        .await
        .into_iter()
        .map(|d| d.0)
        .collect();
    run_for(40).await;
    assert_eq!(
        redriven(&r.store, &ids),
        0,
        "no redrive before a full closed cool-down"
    );
    let before = net.attempts();

    run_for(60).await;

    assert_eq!(redriven(&r.store, &ids), REDRIVE_BATCH);
    assert!(
        net.attempts() - before <= REDRIVE_BATCH,
        "only the batch was re-attempted"
    );
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);
}

/// Control: a sink that stays up drains every dead letter, one batch per tick.
#[tokio::test(start_paused = true)]
async fn a_healthy_sink_drains_all_dead_letters_over_successive_ticks() {
    let net = Fake::new("net", false, Act::Permanent);
    let r = three_hundred_dead_letters_then_recovery(&net, Act::Ok).await;
    let ids: Vec<Uuid> = r
        .manager
        .dead_letters("net")
        .await
        .into_iter()
        .map(|d| d.0)
        .collect();

    run_for(40).await;
    assert_eq!(
        redriven(&r.store, &ids),
        0,
        "no redrive before a full closed cool-down"
    );
    run_for(60).await;

    assert_eq!(redriven(&r.store, &ids), 300);
    for id in &ids {
        assert_eq!(row(&r.store, "net", *id).state, DeliveryState::Delivered);
    }
}

/// A redrive that capacity cut short stays pending: the rows it left behind are redriven once the
/// breaker has been closed a full cool-down, not stranded because fewer than a full batch moved.
#[tokio::test(start_paused = true)]
async fn a_capacity_limited_redrive_is_retried_after_the_cooldown() {
    let (dir, store) = temp_store();
    let down = Fake::new("net", false, Act::Transient);
    let first = rig_over(
        Arc::clone(&store),
        vec![down.boxed()],
        config(|c| {
            c.delivery.max_attempts = 1;
            c.delivery.breaker_failure_threshold = 100;
        }),
    )
    .await;
    for i in 0..4 {
        first.manager.enqueue(&alert(HIGH, &format!("c{i}"))).await;
    }
    run_for(1).await;
    let dead: Vec<Uuid> = first
        .manager
        .dead_letters("net")
        .await
        .into_iter()
        .map(|d| d.0)
        .collect();
    assert_eq!(dead.len(), 4);
    first.manager.shutdown(Duration::from_millis(100)).await;

    let up = Fake::new("net", false, Act::Ok);
    let second = rig_over(
        store,
        vec![up.boxed()],
        config(|c| {
            c.delivery.max_open_rows_per_sink = 2;
            c.delivery.breaker_cooldown_secs = 30;
        }),
    )
    .await;
    settle().await;
    let delivered = |r: &fixtures::Rig| {
        dead.iter()
            .filter(|id| row(&r.store, "net", **id).state == DeliveryState::Delivered)
            .count()
    };
    assert_eq!(
        delivered(&second),
        2,
        "startup redrive moves only the free capacity"
    );

    run_for(36).await;

    for id in &dead {
        let back = row(&second.store, "net", *id);
        assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
    }
    drop(dir);
}

// ---- concurrent enqueue -------------------------------------------------------------------

/// Two alerts with one dedup key enqueued at once: exactly one is delivered, the other is
/// suppressed. Without the enqueue gate both would pass the window check before either recorded.
#[tokio::test(start_paused = true)]
async fn concurrent_enqueues_with_one_dedup_key_deliver_once() {
    let net = Fake::new("net", false, Act::Ok);
    let r = rig(vec![net.boxed()], config(|_| {})).await;
    let a = alert_keyed(HIGH, "a", "K");
    let b = alert_keyed(HIGH, "b", "K");

    let (first, second) = tokio::join!(r.manager.enqueue(&a), r.manager.enqueue(&b));

    let (winner, loser) = match (first, second) {
        (EnqueueOutcome::Enqueued, EnqueueOutcome::SuppressedDuplicate { by }) => {
            assert_eq!(by, a.id);
            (&a, &b)
        }
        (EnqueueOutcome::SuppressedDuplicate { by }, EnqueueOutcome::Enqueued) => {
            assert_eq!(by, b.id);
            (&b, &a)
        }
        other => panic!("expected one Enqueued and one SuppressedDuplicate, got {other:?}"),
    };
    settle().await;
    assert_eq!(
        row(&r.store, "net", winner.id).state,
        DeliveryState::Delivered
    );
    assert_eq!(
        row(&r.store, "net", loser.id).state,
        DeliveryState::SuppressedDuplicate
    );
    assert_eq!(net.attempts(), 1);
}

// ---- factory ------------------------------------------------------------------------------

struct StubBuilder;

impl SinkBuilder for StubBuilder {
    fn build(
        &self,
        resolved: &daemoneye_lib::alerting::ResolvedSink,
        _delivery: &DeliveryConfig,
    ) -> Result<Box<dyn daemoneye_lib::alerting::AlertSink>, daemoneye_lib::alerting::AlertingError>
    {
        Ok(Fake::new(resolved.spec.name(), false, Act::Ok).boxed())
    }
}

#[test]
fn the_factory_builds_local_sinks_and_needs_a_builder_for_network_ones() {
    let sink = |sink_type: &str, config: serde_json::Value| AlertSinkConfig {
        sink_type: sink_type.to_owned(),
        config,
        enabled: true,
    };
    let alerting = AlertingConfig {
        sinks: vec![
            sink("stdout", serde_json::json!({"name": "out"})),
            sink(
                "file",
                serde_json::json!({"name": "log", "path": "alerts.jsonl"}),
            ),
            sink(
                "webhook",
                serde_json::json!({"name": "hook", "url": "https://h.example/x"}),
            ),
        ],
        ..AlertingConfig::default()
    };
    let resolver = CredentialResolver::new(|_| None);
    let resolved: Vec<_> = alerting
        .enabled_specs()
        .unwrap()
        .iter()
        .map(|spec| resolver.resolve(spec).unwrap())
        .collect();
    let delivery = DeliveryConfig::default();

    let out = AlertSinkFactory::build(&resolved[0], &delivery, None).unwrap();
    let log = AlertSinkFactory::build(&resolved[1], &delivery, None).unwrap();
    assert_eq!((out.name(), out.is_local()), ("out", true));
    assert_eq!((log.name(), log.is_local()), ("log", true));

    let missing = AlertSinkFactory::build(&resolved[2], &delivery, None)
        .err()
        .unwrap();
    assert!(missing.to_string().contains("hook"), "{missing}");
    let built = AlertSinkFactory::build(&resolved[2], &delivery, Some(&StubBuilder)).unwrap();
    assert_eq!(built.name(), "hook");
}

/// A probe whose alert cannot be loaded must not leave the breaker half-open forever: the
/// breaker reopens, and the next cool-down probes again.
#[tokio::test(start_paused = true)]
async fn a_probe_that_cannot_load_its_alert_reopens_the_breaker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.redb");
    let unloadable = alert(HIGH, "unloadable");
    {
        let store = EventStore::new(&path).unwrap();
        // Enqueued 20 s ahead so it is not due until after the breaker has opened.
        store
            .enqueue_alert(&unloadable, &["net"], None, 10, now_ms() + 20_000)
            .unwrap();
    }
    {
        let db = redb::Database::open(&path).unwrap();
        let write = db.begin_write().unwrap();
        let alerts: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("alerts");
        write
            .open_table(alerts)
            .unwrap()
            .insert(
                unloadable.id.to_string().as_str(),
                b"not an alert".as_slice(),
            )
            .unwrap();
        write.commit().unwrap();
    }
    let store = Arc::new(EventStore::open(&path).unwrap());
    assert!(
        store.get_alert(&unloadable.id.to_string()).is_err(),
        "precondition: the corrupt row must fail to load, not read as missing"
    );

    let net = Fake::new("net", false, Act::Transient);
    let r = rig_over(
        store,
        vec![net.boxed()],
        config(|c| {
            c.delivery.max_attempts = 1;
            c.delivery.breaker_failure_threshold = 1;
            c.delivery.breaker_cooldown_secs = 30;
        }),
    )
    .await;
    let tripwire = alert(HIGH, "tripwire");
    r.manager.enqueue(&tripwire).await;
    settle().await;
    assert_eq!(
        row(&r.store, "net", tripwire.id).state,
        DeliveryState::DeadLettered
    );
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);

    run_for(36).await;
    let probed = row(&r.store, "net", unloadable.id);
    assert_eq!(
        (probed.state, probed.attempts),
        (DeliveryState::Retrying, 1),
        "the probe claimed the unloadable row once"
    );
    assert_eq!(
        r.manager.health().sinks[0].breaker,
        BreakerState::Open,
        "a probe that sent nothing reopens the breaker"
    );

    run_for(36).await;
    assert_eq!(
        row(&r.store, "net", unloadable.id).attempts,
        2,
        "the next cool-down probes again"
    );
    assert_eq!(net.attempts(), 1, "the sink only ever saw the tripwire");
}

/// A breaker whose last open row was dead-lettered has nothing to probe on: a health probe
/// closes it after the cool-down, and the redrive still waits a further full cool-down.
#[tokio::test(start_paused = true)]
async fn a_stranded_breaker_recovers_by_health_probe_then_redrives() {
    let net = Fake::new("net", false, Act::Transient);
    net.set_healthy(false);
    let r = rig(
        vec![net.boxed()],
        config(|c| {
            c.delivery.max_attempts = 1;
            c.delivery.breaker_failure_threshold = 1;
            c.delivery.breaker_cooldown_secs = 30;
        }),
    )
    .await;
    let a = alert(HIGH, "a");
    r.manager.enqueue(&a).await;
    settle().await;
    assert_eq!(
        row(&r.store, "net", a.id).state,
        DeliveryState::DeadLettered
    );
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);

    run_for(36).await;
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Open);
    let dead = row(&r.store, "net", a.id);
    assert_eq!(
        (dead.state, dead.redrives),
        (DeliveryState::DeadLettered, 0)
    );

    net.set_healthy(true);
    net.set_default(Act::Ok);
    run_for(36).await;
    assert_eq!(r.manager.health().sinks[0].breaker, BreakerState::Closed);
    assert_eq!(
        row(&r.store, "net", a.id).redrives,
        0,
        "redrive waits a cool-down"
    );

    run_for(36).await;
    let back = row(&r.store, "net", a.id);
    assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
    assert_eq!(net.attempts(), 2, "probes are health checks, not sends");
}
