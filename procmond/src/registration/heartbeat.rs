//! Heartbeat message and metric types for the registration manager.

use daemoneye_eventbus::HealthStatus;
use std::time::SystemTime;

/// Connection status for heartbeat metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ConnectionStatus {
    /// Connected to the event bus.
    Connected,
    /// Disconnected from the event bus.
    Disconnected,
    /// Reconnecting to the event bus.
    Reconnecting,
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Connected => write!(f, "connected"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

/// Heartbeat metrics included in each heartbeat message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeartbeatMetrics {
    /// Number of processes collected since last heartbeat.
    pub processes_collected: u64,
    /// Number of events published since last heartbeat.
    pub events_published: u64,
    /// Current buffer level as a percentage (0-100).
    pub buffer_level_percent: f64,
    /// Current connection status.
    pub connection_status: ConnectionStatus,
}

impl Default for HeartbeatMetrics {
    fn default() -> Self {
        Self {
            processes_collected: 0,
            events_published: 0,
            buffer_level_percent: 0.0,
            connection_status: ConnectionStatus::Disconnected,
        }
    }
}

/// Heartbeat message published periodically.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeartbeatMessage {
    /// Collector identifier.
    pub collector_id: String,
    /// Sequence number for this heartbeat.
    pub sequence: u64,
    /// Timestamp of this heartbeat.
    pub timestamp: SystemTime,
    /// Overall health status.
    pub health_status: HealthStatus,
    /// Current metrics.
    pub metrics: HeartbeatMetrics,
    /// Operational sub-status (e.g., "idle-awaiting-begin" when waiting for agent).
    pub operational_sub_status: Option<String>,
}
