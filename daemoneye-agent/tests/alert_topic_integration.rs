//! `events.alert.new`: publish and raw delivery.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_wraps
)]

use daemoneye_agent::alert_publisher::BrokerPublisher;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::topics::alert as alert_topic;
use daemoneye_lib::alerting::{AlertPublisher, PublishError};
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord, Uuid};
use std::sync::Arc;
use std::time::Duration;

const PROJECTED: &[u8] = br#"{"masked":true}"#;
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
