//! Type definitions for busrt client implementation.

use crate::busrt_types::BusrtEvent;
use std::time::Duration;
use tokio::sync::oneshot;

/// Outbound message for internal queuing.
#[derive(Debug)]
#[allow(dead_code)] // Full implementation in progress
pub(super) struct OutboundMessage {
    /// Message type
    pub message_type: OutboundMessageType,
    /// Message payload
    pub payload: serde_json::Value,
    /// Response channel for RPC calls
    pub response_channel:
        Option<oneshot::Sender<Result<serde_json::Value, crate::busrt_types::BusrtError>>>,
}

/// Outbound message types.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Full implementation in progress
pub(super) enum OutboundMessageType {
    /// Publish message to topic
    Publish {
        topic: String,
        event: Box<BusrtEvent>,
    },
    /// Subscribe to topic pattern
    Subscribe { pattern: String },
    /// Unsubscribe from topic pattern
    Unsubscribe { pattern: String },
    /// RPC call
    RpcCall { service: String, method: String },
    /// Heartbeat message
    Heartbeat,
}

/// Reconnection configuration.
#[derive(Debug, Clone)]
pub struct ReconnectionConfig {
    /// Enable automatic reconnection
    pub enabled: bool,
    /// Initial reconnection delay
    pub initial_delay: Duration,
    /// Maximum reconnection delay
    pub max_delay: Duration,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
    /// Maximum reconnection attempts (0 = unlimited)
    pub max_attempts: u32,
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            max_attempts: 0, // Unlimited
        }
    }
}
