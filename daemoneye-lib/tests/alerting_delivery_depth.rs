//! Worker startup: batched redrive for every sink kind, and dead-letter depth seeded from the store.
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

use daemoneye_lib::models::{AlertSeverity, DeliveryReason, DeliveryState, Uuid};
use daemoneye_lib::storage::EventStore;
use fixtures::{Act, Fake, alert, config, now_ms, rig_over, row, run_for, settle, temp_store};

/// Persist `count` dead-lettered rows for sink `net`, as an earlier run would have left them.
fn seed_dead_letters(store: &EventStore, count: usize) -> Vec<Uuid> {
    (0..count)
        .map(|i| {
            let a = alert(AlertSeverity::High, &format!("old{i}"));
            let now = now_ms();
            store
                .enqueue_alert(&a, &["net"], None, 10_000, now)
                .unwrap();
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
        .collect()
}

/// A local sink has no breaker, so startup is its only redrive: it must drain every batch.
#[tokio::test(start_paused = true)]
async fn startup_redrives_every_batch_for_a_local_sink() {
    let (_dir, store) = temp_store();
    let ids = seed_dead_letters(&store, 250);
    let file = Fake::new("net", true, Act::Ok);

    let r = rig_over(store, vec![file.boxed()], config(|_| {})).await;
    settle().await;

    for id in ids {
        let back = row(&r.store, "net", id);
        assert_eq!((back.state, back.redrives), (DeliveryState::Delivered, 1));
    }
    assert_eq!(r.manager.health().sinks[0].dead_letters, 0);
}

/// Rows that cannot be redriven (cap of zero redrives) stay in the queue; health reports them.
#[tokio::test(start_paused = true)]
async fn dead_letter_depth_is_seeded_from_the_store() {
    let (_dir, store) = temp_store();
    seed_dead_letters(&store, 3);
    let net = Fake::new("net", false, Act::Transient);

    let r = rig_over(
        store,
        vec![net.boxed()],
        config(|c| c.delivery.dlq_max_redrives = 0),
    )
    .await;
    settle().await;

    assert_eq!(r.store.dead_letter_count("net").unwrap(), 3);
    assert_eq!(r.manager.health().sinks[0].dead_letters, 3);
    for (_, dead) in r.manager.dead_letters("net").await {
        assert_eq!(
            dead.reason,
            Some(DeliveryReason::AttemptsExhausted),
            "a cap of zero never spends a redrive budget, so the real reason stays"
        );
    }
}

/// A local sink's startup redrive that the open-row cap cut short continues as rows free up,
/// one batch per pass, until the backlog is drained.
#[tokio::test(start_paused = true)]
async fn a_local_startup_redrive_cut_short_by_the_cap_continues() {
    let (_dir, store) = temp_store();
    let ids = seed_dead_letters(&store, 5);
    let file = Fake::new("net", true, Act::Ok);

    let r = rig_over(
        store,
        vec![file.boxed()],
        config(|c| c.delivery.max_open_rows_per_sink = 2),
    )
    .await;
    settle().await;
    let delivered = |r: &fixtures::Rig| {
        ids.iter()
            .filter(|id| row(&r.store, "net", **id).state == DeliveryState::Delivered)
            .count()
    };
    assert_eq!(
        delivered(&r),
        2,
        "startup redrive moves only the free capacity"
    );

    run_for(12).await;

    assert_eq!(delivered(&r), 5);
    assert_eq!(r.manager.health().sinks[0].dead_letters, 0);
}
