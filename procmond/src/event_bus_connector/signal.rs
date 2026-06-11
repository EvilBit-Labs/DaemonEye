//! Backpressure and process-event-type signal enums for the event bus connector.

use super::EventBusConnectorError;

/// Backpressure signal indicating buffer pressure state.
///
/// These signals are emitted when the buffer crosses threshold levels,
/// allowing upstream producers to adjust their event generation rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackpressureSignal {
    /// Buffer has reached high-water mark (70% full).
    /// Upstream should slow down event production.
    Activated,

    /// Buffer has dropped below low-water mark (50% full).
    /// Normal event production can resume.
    Released,
}

/// Type of process event for topic routing.
///
/// Determines which topic the event will be published to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcessEventType {
    /// Process started - published to `events.process.start`.
    Start,
    /// Process stopped - published to `events.process.stop`.
    Stop,
    /// Process modified (e.g., name change) - published to `events.process.modify`.
    Modify,
}

impl ProcessEventType {
    /// Get the topic string for this event type.
    pub(super) const fn topic(self) -> &'static str {
        match self {
            Self::Start => "events.process.start",
            Self::Stop => "events.process.stop",
            Self::Modify => "events.process.modify",
        }
    }

    /// Get the event type as a short string for WAL persistence.
    pub(super) const fn to_type_string(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Modify => "modify",
        }
    }

    /// Parse event type from a string stored in WAL.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusConnectorError::UnknownEventType`] when `s` does not
    /// match any known variant.  The caller must not silently substitute a
    /// default: a corrupted `"stop"` entry reclassified as `Start` would keep
    /// a terminated process alive in the tracking state.
    pub(super) fn from_type_string(s: &str) -> Result<Self, EventBusConnectorError> {
        match s {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "modify" => Ok(Self::Modify),
            _ => Err(EventBusConnectorError::UnknownEventType(s.to_owned())),
        }
    }
}
