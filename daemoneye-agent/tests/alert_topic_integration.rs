//! `events.alert.new`: publish, raw delivery and the delivery-worker wake signal.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_wraps
)]

use async_trait::async_trait;
use daemoneye_agent::alert_publisher::{BrokerPublisher, spawn_wake_subscriber};
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::topics::alert as alert_topic;
use daemoneye_lib::alerting::{
    AlertManager, AlertPublisher, AlertSink, AlertingError, DeliveryResult, NoopPublisher,
    PublishError,
};
use daemoneye_lib::config::AlertingConfig;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord, Uuid};
use daemoneye_lib::storage::EventStore;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

const PROJECTED: &[u8] = br#"{"masked":true}"#;
const MAX_OPEN_ROWS: usize = 1_000;
const OVERSIZE_PAYLOAD: usize = 2 * 1024 * 1024;

fn sample_alert() -> Alert {
    Alert::new(
        AlertSeverity::High,
        "title",
        "description with secrets",
        "rule-1",
        ProcessRecord::new(1, "p".to_owned()),
        Completeness::complete(),
    )
}

fn masked(_alert: &Alert) -> Result<Vec<u8>, PublishError> {
    Ok(PROJECTED.to_vec())
}

fn oversize(_alert: &Alert) -> Result<Vec<u8>, PublishError> {
    Ok(vec![0; OVERSIZE_PAYLOAD])
}

async fn broker(dir: &tempfile::TempDir) -> Arc<DaemoneyeBroker> {
    let socket = dir.path().join("broker.sock");
    Arc::new(
        DaemoneyeBroker::new(socket.to_str().unwrap())
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn publish_delivers_the_projection_to_every_raw_subscriber() {
    let dir = tempfile::tempdir().unwrap();
    let broker = broker(&dir).await;
    let mut first = broker
        .subscribe_raw(alert_topic::NEW, Uuid::new_v4())
        .await
        .unwrap();
    let mut second = broker
        .subscribe_raw(alert_topic::NEW, Uuid::new_v4())
        .await
        .unwrap();
    let publisher = BrokerPublisher::new(Arc::clone(&broker), masked);
    let alert = sample_alert();

    publisher.publish(&alert).await.unwrap();

    for rx in [&mut first, &mut second] {
        let message = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("delivery timed out")
            .expect("channel closed");
        assert_eq!(message.payload, PROJECTED);
        assert_eq!(message.topic, alert_topic::NEW);
        assert_eq!(
            message.correlation_metadata.correlation_id,
            alert.id.to_string()
        );
    }
}

#[tokio::test]
async fn publish_returns_an_error_when_the_broker_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let publisher = BrokerPublisher::new(broker(&dir).await, oversize);

    let error = publisher.publish(&sample_alert()).await.unwrap_err();

    assert!(
        error.to_string().contains("1MB"),
        "unexpected error: {error}"
    );
}

/// Reports each alert id it is asked to send.
struct ReportingSink(Mutex<mpsc::UnboundedSender<Uuid>>);

#[async_trait]
impl AlertSink for ReportingSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        self.0.lock().unwrap().send(alert.id).unwrap();
        Ok(DeliveryResult {
            sink_name: "fake".to_owned(),
            delivered_at: chrono::Utc::now(),
            duration_ms: 0,
        })
    }

    fn name(&self) -> &'static str {
        "fake"
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        Ok(())
    }

    fn is_local(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn wake_subscriber_delivers_a_row_enqueued_behind_the_managers_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("events.redb")).unwrap());
    let (sent_tx, mut sent_rx) = mpsc::unbounded_channel();
    let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let manager = Arc::new(
        AlertManager::start(
            Arc::clone(&store),
            vec![Box::new(ReportingSink(Mutex::new(sent_tx)))],
            &AlertingConfig::default(),
            Arc::new(NoopPublisher),
            cancel_rx,
        )
        .await
        .unwrap(),
    );
    let broker = broker(&dir).await;
    let wake = spawn_wake_subscriber(Arc::clone(&broker), Arc::clone(&manager))
        .await
        .unwrap();
    // Let the worker reach its idle wait before the row exists.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let alert = sample_alert();
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    store
        .enqueue_alert(&alert, &["fake"], None, MAX_OPEN_ROWS, now_ms)
        .unwrap();
    BrokerPublisher::new(Arc::clone(&broker), masked)
        .publish(&alert)
        .await
        .unwrap();

    let delivered = tokio::time::timeout(Duration::from_secs(1), sent_rx.recv())
        .await
        .expect("row was not delivered before the worker tick")
        .expect("sink channel closed");
    assert_eq!(delivered, alert.id);

    wake.shutdown().await;
    manager.shutdown(Duration::from_secs(1)).await;
}
