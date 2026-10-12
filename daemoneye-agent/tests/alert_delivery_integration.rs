//! Agent wiring of alert delivery: sink construction, enrichment, the store-failure fallback,
//! restart resume, retention and ordered shutdown.

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

use daemoneye_agent::alert_publisher::{
    BrokerPublisher, masked_projection, shutdown_alert_delivery, spawn_wake_subscriber,
};
use daemoneye_agent::alert_sinks::{DEFAULT_SINK_NAME, build_sinks};
use daemoneye_agent::detection_cycle::{enrich_and_enqueue, run_retention_sweep};
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_lib::alerting::{
    AlertManager, CredentialResolver, DeliveryAggregate, NoopPublisher, SHUTDOWN_DRAIN,
};
use daemoneye_lib::config::{AlertSinkConfig, AlertingConfig};
use daemoneye_lib::models::{DeliveryReason, DeliveryState, Uuid};
use daemoneye_lib::storage::{CompleteOutcome, EventStore};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::mock_http::{MockHttp, Response};
use support::{
    TOKEN_VAR, WEBHOOK, alert, alerting, ctx, file_sink, lines_with, resolver, row, start_manager,
    temp_store, wait_until, webhook_sink,
};

const DEADLINE: Duration = Duration::from_secs(5);
const MAX_OPEN: usize = 1_000;

// ---- AE7: sink construction ---------------------------------------------------------------

#[test]
fn an_enabled_webhook_with_an_unset_token_variable_refuses_startup_naming_both() {
    let config = alerting(vec![webhook_sink("http://127.0.0.1:1/hook")]);

    let error = build_sinks(&config, &CredentialResolver::new(|_| None))
        .err()
        .expect("startup must be refused");

    let text = format!("{error:#}");
    assert!(text.contains(WEBHOOK), "sink name missing: {text}");
    assert!(text.contains(TOKEN_VAR), "variable name missing: {text}");
}

#[test]
fn positive_control_the_same_webhook_builds_when_the_variable_resolves() {
    let config = alerting(vec![webhook_sink("http://127.0.0.1:1/hook")]);

    let sinks = build_sinks(&config, &resolver()).unwrap();

    assert_eq!(sinks.len(), 1);
    assert_eq!(sinks[0].name(), WEBHOOK);
}

#[test]
fn a_disabled_sink_is_not_resolved_so_a_missing_variable_does_not_refuse_startup() {
    let mut sink = webhook_sink("http://127.0.0.1:1/hook");
    sink.enabled = false;
    let config = alerting(vec![sink]);

    let sinks = build_sinks(&config, &CredentialResolver::new(|_| None)).unwrap();

    assert!(sinks.is_empty());
}

#[test]
fn no_configured_sink_yields_the_default_stdout_sink() {
    let sinks = build_sinks(
        &AlertingConfig::default(),
        &CredentialResolver::new(|_| None),
    )
    .unwrap();

    assert_eq!(sinks.len(), 1);
    assert_eq!(sinks[0].name(), DEFAULT_SINK_NAME);
    assert!(sinks[0].is_local());
}

#[test]
fn an_unknown_sink_type_is_refused_not_dropped() {
    let config = alerting(vec![AlertSinkConfig {
        sink_type: "carrier-pigeon".to_owned(),
        config: json!({ "name": "bird" }),
        enabled: true,
    }]);

    let error = build_sinks(&config, &resolver())
        .err()
        .expect("must refuse");

    assert!(format!("{error:#}").contains("carrier-pigeon"));
}

// ---- enrichment wiring --------------------------------------------------------------------

#[tokio::test]
async fn an_alert_enqueued_through_the_cycle_is_stored_enriched() {
    let (dir, store) = temp_store();
    let config = alerting(vec![file_sink(&dir.path().join("out.log"))]);
    let (manager, _cancel) = start_manager(&store, &config).await;
    let raised = alert("enrich-me");

    let persisted =
        enrich_and_enqueue(&store, &manager, std::slice::from_ref(&raised), &ctx()).await;

    assert_eq!(persisted, 1);
    let stored = store.get_alert(&raised.id.to_string()).unwrap().unwrap();
    assert_eq!(
        stored.context.data.get("host").map(String::as_str),
        Some("test-host")
    );
    assert_eq!(
        stored.context.data.get("agent_version").map(String::as_str),
        Some("9.9.9")
    );
    assert!(stored.context.tags.iter().any(|tag| tag == "enriched"));
    assert!(
        !raised.context.data.contains_key("host"),
        "control: the raised alert was not enriched before the call"
    );
    manager.shutdown(SHUTDOWN_DRAIN).await;
}

// ---- AE4: store failure -------------------------------------------------------------------

/// A store whose `alerts` table has the wrong key type, so every enqueue transaction fails.
fn broken_alerts_store(dir: &std::path::Path) -> Arc<EventStore> {
    let path = dir.join("broken.redb");
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
    Arc::new(EventStore::open(&path).unwrap())
}

#[tokio::test]
async fn a_store_failure_delivers_once_to_local_sinks_marked_not_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockHttp::start(Vec::new()).await;
    let out = dir.path().join("out.log");
    let config = alerting(vec![webhook_sink(&mock.url()), file_sink(&out)]);
    let store = broken_alerts_store(dir.path());
    let (manager, _cancel) = start_manager(&store, &config).await;
    let raised = alert("store-down");

    let persisted =
        enrich_and_enqueue(&store, &manager, std::slice::from_ref(&raised), &ctx()).await;

    assert_eq!(persisted, 0, "a failed enqueue does not count as persisted");
    assert!(
        wait_until(DEADLINE, || lines_with(&out, "not_persisted") == 1).await,
        "the file sink got no marked copy"
    );
    assert_eq!(
        lines_with(&out, &raised.id.to_string()),
        1,
        "delivered exactly once"
    );
    assert!(
        mock.requests().is_empty(),
        "the network sink must not be called"
    );
    let health = manager.health();
    assert_eq!(health.store_failures, 1);
    assert_eq!(health.aggregate, DeliveryAggregate::Unhealthy);
    manager.shutdown(SHUTDOWN_DRAIN).await;
}

#[tokio::test]
async fn control_with_a_healthy_store_the_same_logical_alert_is_enqueued_not_suppressed() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockHttp::start(Vec::new()).await;
    let out = dir.path().join("out.log");
    let config = alerting(vec![webhook_sink(&mock.url()), file_sink(&out)]);
    let (store, next_cycle) = {
        let broken = broken_alerts_store(dir.path());
        let (manager, _cancel) = start_manager(&broken, &config).await;
        let first = enrich_and_enqueue(&broken, &manager, &[alert("store-down")], &ctx()).await;
        assert_eq!(first, 0);
        manager.shutdown(SHUTDOWN_DRAIN).await;
        let (_healthy_dir, healthy) = temp_store();
        (healthy, alert("store-down"))
    };
    let healthy_out = dir.path().join("healthy.log");
    let config = alerting(vec![webhook_sink(&mock.url()), file_sink(&healthy_out)]);
    let (manager, _cancel) = start_manager(&store, &config).await;

    let persisted =
        enrich_and_enqueue(&store, &manager, std::slice::from_ref(&next_cycle), &ctx()).await;

    assert_eq!(persisted, 1);
    assert!(
        wait_until(DEADLINE, || mock.requests().len() == 1).await,
        "the webhook got no request"
    );
    let webhook_row = row(&store, WEBHOOK, next_cycle.id);
    assert_ne!(webhook_row.state, DeliveryState::SuppressedDuplicate);
    assert!(
        wait_until(DEADLINE, || lines_with(
            &healthy_out,
            &next_cycle.id.to_string()
        ) == 1)
        .await
    );
    assert_eq!(
        lines_with(&healthy_out, "not_persisted"),
        0,
        "healthy copy carries no marker"
    );
    manager.shutdown(SHUTDOWN_DRAIN).await;
}

// ---- AE5: restart resumes -----------------------------------------------------------------

#[tokio::test]
async fn a_row_left_retrying_by_shutdown_is_redelivered_with_the_same_identity_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockHttp::start(vec![Response::Status(503)]).await;
    let config = alerting(vec![webhook_sink(&mock.url())]);
    let (store, _keep) = {
        let (d, s) = temp_store();
        (s, d)
    };
    let raised = alert("restart-me");
    let (first, cancel) = start_manager(&store, &config).await;
    assert_eq!(
        enrich_and_enqueue(&store, &first, std::slice::from_ref(&raised), &ctx()).await,
        1
    );
    assert!(wait_until(DEADLINE, || mock.requests().len() == 1).await);

    first.shutdown(SHUTDOWN_DRAIN).await;
    drop(cancel);

    let mid = row(&store, WEBHOOK, raised.id);
    assert_eq!(
        mid.state,
        DeliveryState::Retrying,
        "precondition: left mid-retry"
    );
    assert_eq!(mid.attempts, 1);
    assert_eq!(mock.requests().len(), 1, "precondition: no second send yet");

    let (second, _cancel2) = start_manager(&store, &config).await;
    assert!(
        wait_until(DEADLINE, || row(&store, WEBHOOK, raised.id).state
            == DeliveryState::Delivered)
        .await,
        "the restarted manager did not redeliver"
    );

    let seen = mock.requests();
    assert_eq!(seen.len(), 2);
    let id = raised.id.to_string();
    for request in &seen {
        assert_eq!(request.header("x-daemoneye-alert-id"), Some(id.as_str()));
        assert_eq!(
            request.header("x-daemoneye-dedup-key"),
            Some(raised.deduplication_key.as_str())
        );
    }
    assert_eq!(row(&store, WEBHOOK, raised.id).attempts, 2);
    second.shutdown(SHUTDOWN_DRAIN).await;
    drop(dir);
}

// ---- retention ----------------------------------------------------------------------------

const GHOST: &str = "ghost";

fn finish(store: &EventStore, id: Uuid, state: DeliveryState, terminal_at_ms: u64) {
    let claimed = store.claim(GHOST, id, 1).unwrap();
    let mut done = claimed.record.clone();
    done.state = state;
    done.reason =
        (state == DeliveryState::DeadLettered).then_some(DeliveryReason::AttemptsExhausted);
    done.terminal_at_ms = Some(terminal_at_ms);
    assert_eq!(
        store.complete(GHOST, id, claimed.expected, &done).unwrap(),
        CompleteOutcome::Written
    );
}

#[test]
fn the_sweep_prunes_old_terminal_delivery_rows_and_nothing_else() {
    let (_dir, store) = temp_store();
    let (old, young_dead, retrying) = (alert("old"), alert("young-dead"), alert("retrying"));
    for a in [&old, &young_dead, &retrying] {
        store.enqueue_alert(a, &[GHOST], None, MAX_OPEN, 0).unwrap();
    }
    finish(&store, old.id, DeliveryState::Delivered, 100);
    finish(&store, young_dead.id, DeliveryState::DeadLettered, 4_500);
    store.claim(GHOST, retrying.id, 1).unwrap();
    let dead_before = store.dead_letters(GHOST).unwrap().len();
    assert_eq!(dead_before, 1, "precondition: one dead letter");

    let (buckets, pruned) = run_retention_sweep(&store, 5_000, 1_000).unwrap();

    assert_eq!((buckets, pruned), (0, 1));
    assert!(store.get_delivery(GHOST, old.id).unwrap().is_none());
    assert!(
        store.get_alert(&old.id.to_string()).unwrap().is_some(),
        "alert rows are never pruned"
    );
    assert_eq!(
        store
            .get_delivery(GHOST, retrying.id)
            .unwrap()
            .unwrap()
            .state,
        DeliveryState::Retrying
    );
    assert_eq!(store.dead_letters(GHOST).unwrap().len(), dead_before);
}

#[test]
fn control_a_sweep_inside_the_retention_window_prunes_nothing() {
    let (_dir, store) = temp_store();
    let old = alert("old");
    store
        .enqueue_alert(&old, &[GHOST], None, MAX_OPEN, 0)
        .unwrap();
    finish(&store, old.id, DeliveryState::Delivered, 100);

    let (_buckets, pruned) = run_retention_sweep(&store, 500, 1_000).unwrap();

    assert_eq!(pruned, 0);
    assert!(store.get_delivery(GHOST, old.id).unwrap().is_some());
}

// ---- shutdown -----------------------------------------------------------------------------

async fn broker(dir: &tempfile::TempDir) -> Arc<DaemoneyeBroker> {
    let socket = dir.path().join("broker.sock");
    Arc::new(
        DaemoneyeBroker::new(socket.to_str().unwrap())
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn shutdown_returns_within_the_drain_with_a_send_in_flight_and_drops_the_subscription_first()
{
    let dir = tempfile::tempdir().unwrap();
    let mock = MockHttp::start(vec![Response::Hang]).await;
    let mut config = alerting(vec![webhook_sink(&mock.url())]);
    config.delivery.attempt_timeout_secs = 30;
    let (store, _keep) = {
        let (d, s) = temp_store();
        (s, d)
    };
    let broker = broker(&dir).await;
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let manager = Arc::new(
        AlertManager::start(
            Arc::clone(&store),
            build_sinks(&config, &resolver()).unwrap(),
            &config,
            Arc::new(BrokerPublisher::new(Arc::clone(&broker), masked_projection)),
            cancel_rx,
        )
        .await
        .unwrap(),
    );
    let wake = spawn_wake_subscriber(Arc::clone(&broker), Arc::clone(&manager))
        .await
        .unwrap();
    let raised = alert("in-flight");
    assert_eq!(
        enrich_and_enqueue(&store, &manager, std::slice::from_ref(&raised), &ctx()).await,
        1
    );
    assert!(
        wait_until(DEADLINE, || mock.requests().len() == 1).await,
        "send never started"
    );
    assert_eq!(
        broker.statistics().await.active_subscribers,
        1,
        "precondition: subscribed"
    );
    let drain = Duration::from_millis(300);

    let started = Instant::now();
    shutdown_alert_delivery(&manager, Some(wake), drain).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < drain + Duration::from_secs(2),
        "shutdown took {elapsed:?}"
    );
    let stuck = row(&store, WEBHOOK, raised.id);
    assert_eq!((stuck.state, stuck.attempts), (DeliveryState::Retrying, 1));
    assert_eq!(
        broker.statistics().await.active_subscribers,
        0,
        "the subscription is gone while the broker is still up"
    );
    broker.shutdown().await.unwrap();
    drop(cancel);
}

#[tokio::test]
async fn control_without_shutdown_the_subscription_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, store) = temp_store();
    let broker = broker(&dir).await;
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let manager = Arc::new(
        AlertManager::start(
            store,
            Vec::new(),
            &AlertingConfig::default(),
            Arc::new(NoopPublisher),
            cancel_rx,
        )
        .await
        .unwrap(),
    );
    let _wake = spawn_wake_subscriber(Arc::clone(&broker), Arc::clone(&manager))
        .await
        .unwrap();

    assert_eq!(broker.statistics().await.active_subscribers, 1);
    manager.shutdown(SHUTDOWN_DRAIN).await;
}
