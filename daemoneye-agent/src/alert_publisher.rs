//! Announces stored alerts on `events.alert.new` and wakes the delivery workers from it.

use anyhow::Context;
use async_trait::async_trait;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::topics::alert;
use daemoneye_lib::alerting::{AlertManager, AlertPublisher, PublishError};
use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags};
use daemoneye_lib::models::Alert;
use std::sync::Arc;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Serialises an alert for the bus; the caller owns what it exposes.
pub type Projection = fn(&Alert) -> Vec<u8>;

/// The allowlist projection with no opt-in fields: what every bus subscriber may see.
#[must_use]
pub fn masked_projection(alert: &Alert) -> Vec<u8> {
    serde_json::to_vec(&ExternalAlert::project(alert, IncludeFlags::default())).unwrap_or_default()
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
        let payload = (self.project)(alert);
        self.broker
            .publish(alert::NEW, &alert.id.to_string(), payload)
            .await
            .map_err(|error| PublishError(error.to_string()))
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
        let json: serde_json::Value = serde_json::from_slice(&masked_projection(&alert)).unwrap();
        assert!(
            json.get("command_line").is_none(),
            "command line leaked onto the bus"
        );
        assert_eq!(
            json.get("rule_id").and_then(serde_json::Value::as_str),
            Some("rule-1")
        );
        assert!(!String::from_utf8_lossy(&masked_projection(&alert)).contains("s3cr3t-sentinel"));
    }
}
