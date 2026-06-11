//! Broker health status type for the embedded `EventBus` broker.

use crate::health::HealthState;

/// Health status of the embedded broker
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrokerHealth {
    /// Broker is healthy and operational
    Healthy,
    /// Broker is starting up
    Starting,
    /// Broker is shutting down
    ShuttingDown,
    /// Broker has encountered an error
    Unhealthy(String),
    /// Broker is stopped
    Stopped,
}

impl HealthState for BrokerHealth {
    fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    fn is_starting(&self) -> bool {
        matches!(self, Self::Starting)
    }

    fn unhealthy_message(&self) -> Option<&str> {
        match *self {
            Self::Unhealthy(ref msg) => Some(msg),
            Self::Healthy | Self::Starting | Self::ShuttingDown | Self::Stopped => None,
        }
    }

    fn is_stopped_or_shutting_down(&self) -> bool {
        matches!(self, Self::ShuttingDown | Self::Stopped)
    }

    fn service_name() -> &'static str {
        "Broker"
    }
}
