//! Partition and runtime-isolation scenarios for alert delivery through the agent's wiring.

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
    clippy::shadow_same
)]

mod support;

use daemoneye_agent::detection_cycle::enrich_and_enqueue;
use daemoneye_lib::alerting::{BreakerState, DeliveryAggregate, SHUTDOWN_DRAIN};
use daemoneye_lib::models::DeliveryState;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::mock_http::{MockHttp, Response};
use support::{
    FILE, SYSLOG, WEBHOOK, alert, alerting, closed_tcp_address, ctx, file_sink, lines_with, row,
    start_manager, syslog_sink, temp_store, wait_until, webhook_sink,
};

const DEADLINE: Duration = Duration::from_secs(2);
const BATCHES: [usize; 3] = [4, 3, 3];

/// Raise `BATCHES` through the cycle's entry point; every batch must count as persisted and
/// reach the file within the deadline. Returns every alert id raised.
async fn run_batches(
    store: &Arc<daemoneye_lib::storage::EventStore>,
    manager: &daemoneye_lib::alerting::AlertManager,
    out: &std::path::Path,
) -> Vec<daemoneye_lib::models::Uuid> {
    let mut ids = Vec::new();
    for (batch_no, size) in BATCHES.iter().enumerate() {
        let alerts: Vec<_> = (0..*size)
            .map(|i| alert(&format!("b{batch_no}-a{i}")))
            .collect();
        ids.extend(alerts.iter().map(|a| a.id));
        let persisted = enrich_and_enqueue(store, manager, &alerts, &ctx()).await;
        assert_eq!(persisted, *size, "batch {batch_no} must count as persisted");
        let raised_so_far = ids.len();
        assert!(
            wait_until(DEADLINE, || {
                ids.iter()
                    .filter(|id| lines_with(out, &id.to_string()) == 1)
                    .count()
                    == raised_so_far
            })
            .await,
            "batch {batch_no} did not reach the file within {DEADLINE:?}"
        );
    }
    ids
}

#[tokio::test]
async fn a_network_partition_leaves_local_sinks_delivering_and_only_degrades_health() {
    let (dir, store) = temp_store();
    let out = dir.path().join("out.log");
    let mut mock = MockHttp::start(Vec::new()).await;
    let config = alerting(vec![
        webhook_sink(&mock.url()),
        syslog_sink(&closed_tcp_address()),
        file_sink(&out),
    ]);
    mock.close().await;
    let manager = start_manager(&store, &config).await;

    let ids = run_batches(&store, &manager, &out).await;

    assert!(
        wait_until(DEADLINE, || {
            ids.iter()
                .all(|id| row(&store, FILE, *id).state == DeliveryState::Delivered)
        })
        .await,
        "file rows were not marked delivered"
    );
    for sink in [WEBHOOK, SYSLOG] {
        assert!(
            wait_until(Duration::from_secs(5), || {
                manager
                    .health()
                    .sinks
                    .iter()
                    .any(|s| s.name == sink && s.breaker == BreakerState::Open)
            })
            .await,
            "{sink} breaker never opened"
        );
    }
    let health = manager.health();
    assert_eq!(health.aggregate, DeliveryAggregate::Degraded);
    assert!(
        mock.requests().is_empty(),
        "precondition: the partition held, no request landed"
    );
    for sink in [WEBHOOK, SYSLOG] {
        let delivered = ids
            .iter()
            .filter(|id| row(&store, sink, **id).state == DeliveryState::Delivered)
            .count();
        assert_eq!(delivered, 0, "{sink} delivered through the partition");
    }
    manager.shutdown(SHUTDOWN_DRAIN).await;
}

#[tokio::test]
async fn control_with_the_listener_up_everything_delivers_and_health_is_healthy() {
    let (dir, store) = temp_store();
    let out = dir.path().join("out.log");
    let mock = MockHttp::start(Vec::new()).await;
    let config = alerting(vec![webhook_sink(&mock.url()), file_sink(&out)]);
    let manager = start_manager(&store, &config).await;

    let ids = run_batches(&store, &manager, &out).await;

    assert!(
        wait_until(Duration::from_secs(5), || {
            ids.iter()
                .all(|id| row(&store, WEBHOOK, *id).state == DeliveryState::Delivered)
        })
        .await,
        "webhook rows were not delivered"
    );
    assert_eq!(manager.health().aggregate, DeliveryAggregate::Healthy);
    assert_eq!(mock.requests().len(), ids.len());
    manager.shutdown(SHUTDOWN_DRAIN).await;
}

// ---- runtime isolation --------------------------------------------------------------------

const TICK: Duration = Duration::from_millis(100);
const MAX_GAP: Duration = Duration::from_millis(250);
const HOLD: Duration = Duration::from_millis(1_200);
const OBSERVE: Duration = Duration::from_secs(2);

/// A hung sink and a store kept busy by a foreign writer must not stall the runtime thread.
///
/// Held: a `spawn_blocking` loop committing `enqueue_alert` transactions back to back for 1.2 s,
/// so the cycle's own enqueues contend for redb's single writer. The runtime is single-threaded,
/// so any blocking call on it would show as a gap in the 100 ms ticker.
#[tokio::test]
async fn a_hung_sink_and_a_busy_store_do_not_stall_the_runtime() {
    let (dir, store) = temp_store();
    let mock = MockHttp::start(vec![Response::Hang]).await;
    let config = alerting(vec![
        webhook_sink(&mock.url()),
        file_sink(&dir.path().join("out.log")),
    ]);
    let manager = start_manager(&store, &config).await;
    let stamps = Arc::new(Mutex::new(vec![Instant::now()]));
    let ticker = {
        let stamps = Arc::clone(&stamps);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            loop {
                interval.tick().await;
                stamps.lock().unwrap().push(Instant::now());
            }
        })
    };
    let busy_store = Arc::clone(&store);
    let holder = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut commits = 0_u32;
        while started.elapsed() < HOLD {
            let foreign = alert(&format!("foreign-{commits}"));
            busy_store
                .enqueue_alert(&foreign, &["ghost"], None, 100_000, 0)
                .unwrap();
            commits += 1;
        }
        (started.elapsed(), commits)
    });
    let batch: Vec<_> = (0..20).map(|i| alert(&format!("cycle-{i}"))).collect();

    let persisted = enrich_and_enqueue(&store, &manager, &batch, &ctx()).await;
    let (held_for, commits) = holder.await.unwrap();
    tokio::time::sleep(OBSERVE.saturating_sub(held_for)).await;
    ticker.abort();

    assert_eq!(persisted, batch.len());
    assert!(
        held_for >= Duration::from_secs(1),
        "control: store held only {held_for:?}"
    );
    assert!(
        commits > 1,
        "control: the foreign writer committed {commits} times"
    );
    assert!(
        wait_until(Duration::from_secs(2), || !mock.requests().is_empty()).await,
        "control: the hung sink was never attempted"
    );
    let (tick_count, worst) = {
        let recorded = stamps.lock().unwrap();
        let worst = recorded.windows(2).map(|w| w[1] - w[0]).max().unwrap();
        (recorded.len(), worst)
    };
    assert!(
        tick_count >= 10,
        "control: ticker barely ran ({tick_count} ticks)"
    );
    assert!(worst <= MAX_GAP, "runtime stalled for {worst:?}");
    manager.shutdown(Duration::from_millis(200)).await;
}
