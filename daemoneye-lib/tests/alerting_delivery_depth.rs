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
use fixtures::{Act, Fake, alert, config, now_ms, rig_over, row, settle, temp_store};

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
}
