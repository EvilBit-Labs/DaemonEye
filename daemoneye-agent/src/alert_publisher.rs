//! Announces stored alerts on `events.alert.new` and wakes the delivery workers from it.

use anyhow::Context;
use async_trait::async_trait;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::topics::alert;
use daemoneye_lib::alerting::{AlertManager, AlertPublisher, PublishError};
use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags};
use daemoneye_lib::models::Alert;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Serialises an alert for the bus; the caller owns what it exposes. A failure is a publish
/// failure, never an empty payload.
pub type Projection = fn(&Alert) -> Result<Vec<u8>, PublishError>;

/// The allowlist projection with no opt-in fields: what every bus subscriber may see.
///
/// # Errors
///
/// [`PublishError`] when the projection does not serialise.
pub fn masked_projection(alert: &Alert) -> Result<Vec<u8>, PublishError> {
    serde_json::to_vec(&ExternalAlert::project(alert, IncludeFlags::default()))
        .map_err(|error| PublishError::new(format!("alert projection: {error}")))
}

/// Publishes each stored alert's projection on [`alert::NEW`].
pub struct BrokerPublisher {
    broker: Arc<DaemoneyeBroker>,
    project: Projection,
}

impl BrokerPublisher {
    /// Publisher over `broker` that serialises alerts with `project`.
    #[must_use]
    pub const fn new(broker: Arc<DaemoneyeBroker>, project: Projection) -> Self {
        Self { broker, project }
    }
}

#[async_trait]
impl AlertPublisher for BrokerPublisher {
    async fn publish(&self, alert: &Alert) -> Result<(), PublishError> {
        let payload = (self.project)(alert)?;
        self.broker
            .publish(alert::NEW, &alert.id.to_string(), payload)
            .await
            .map_err(|error| PublishError::new(error.to_string()))
    }
}

/// Wakes every sink worker on each message published to [`alert::NEW`].
pub struct WakeSubscriber {
    broker: Arc<DaemoneyeBroker>,
    id: Uuid,
    task: JoinHandle<()>,
}

/// Subscribe to [`alert::NEW`] in-process and call `manager.wake_all()` per message.
///
/// # Errors
///
/// The broker refused the subscription.
pub async fn spawn_wake_subscriber(
    broker: Arc<DaemoneyeBroker>,
    manager: Arc<AlertManager>,
) -> anyhow::Result<WakeSubscriber> {
    let id = Uuid::new_v4();
    let mut rx = broker
        .subscribe_raw(alert::NEW, id)
        .await
        .context("subscribing the alert wake signal")?;
    let task = tokio::spawn(async move {
        while rx.recv().await.is_some() {
            manager.wake_all();
        }
    });
    Ok(WakeSubscriber { broker, id, task })
}

impl WakeSubscriber {
    /// Stop the task and drop the subscription.
    pub async fn shutdown(self) {
        self.task.abort();
        if let Err(error) = self.broker.unsubscribe(self.id).await {
            tracing::warn!(%error, "alert wake unsubscribe failed");
        }
    }
}

/// Stop alert delivery ahead of the bus.
///
/// Workers get `drain` to finish in-flight sends (the rest stay `Retrying` for the next start),
/// then the wake subscription is dropped. The caller shuts the broker down afterwards.
pub async fn shutdown_alert_delivery(
    manager: &AlertManager,
    wake: Option<WakeSubscriber>,
    drain: Duration,
) {
    manager.shutdown(drain).await;
    if let Some(subscriber) = wake {
        subscriber.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::masked_projection;
    use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord};

    /// The bus payload is the masked projection: the command line never leaves the agent this way.
    #[test]
    fn bus_projection_omits_the_command_line() {
        let mut record = ProcessRecord::new(7, "proc".to_owned());
        record.command_line = Some("curl -H 'Authorization: s3cr3t-sentinel'".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "t",
            "d",
            "rule-1",
            record,
            Completeness::complete(),
        );
        let payload = masked_projection(&alert).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert!(
            json.get("command_line").is_none(),
            "command line leaked onto the bus"
        );
        assert_eq!(
            json.get("rule_id").and_then(serde_json::Value::as_str),
            Some("rule-1")
        );
        assert!(!String::from_utf8_lossy(&payload).contains("s3cr3t-sentinel"));
    }
}
