//! Error types for the event bus connector.

use crate::wal::WalError;
use thiserror::Error;

/// Errors that can occur during event bus connector operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EventBusConnectorError {
    /// WAL operation failed.
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),

    /// EventBus operation failed.
    #[error("EventBus error: {0}")]
    EventBus(String),

    /// Connection to broker failed.
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Buffer has reached capacity.
    #[error("Buffer overflow: buffer is at capacity")]
    BufferOverflow,

    /// Required environment variable is not set.
    #[error("Environment variable not set: {0}")]
    EnvNotSet(String),

    /// Serialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Event type string read from WAL is not a recognised variant.
    #[error("Unknown event type in WAL: '{0}'")]
    UnknownEventType(String),
}

/// Result type for event bus connector operations.
pub type EventBusConnectorResult<T> = Result<T, EventBusConnectorError>;
