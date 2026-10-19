//! Announces stored alerts on `events.alert.new`.

use async_trait::async_trait;
use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_eventbus::topics::alert;
use daemoneye_lib::alerting::{AlertPublisher, PublishError};
use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags};
use daemoneye_lib::models::Alert;
use std::sync::Arc;

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
