//! Error types for the DaemonEye EventBus

use thiserror::Error;

/// Result type alias for EventBus operations
pub type Result<T> = std::result::Result<T, EventBusError>;

/// Errors that can occur in EventBus operations
#[derive(Error, Debug)]
pub enum EventBusError {
    /// Transport layer errors (socket, pipe, network)
    #[error("Transport error: {0}")]
    Transport(String),

    /// Serialization/deserialization errors
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Topic or subscription related errors
    #[error("Topic error: {0}")]
    Topic(String),

    /// Broker lifecycle errors
    #[error("Broker error: {0}")]
    Broker(String),

    /// Client connection errors
    #[error("Client error: {0}")]
    Client(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Timeout errors
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// Shutdown errors
    #[error("Shutdown error: {0}")]
    Shutdown(String),

    /// RPC operation errors
    #[error("RPC error: {0}")]
    Rpc(String),
}

impl EventBusError {
    /// Create a transport error
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Create a serialization error
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }

    /// Create a topic error
    pub fn topic(msg: impl Into<String>) -> Self {
        Self::Topic(msg.into())
    }

    /// Create a broker error
    pub fn broker(msg: impl Into<String>) -> Self {
        Self::Broker(msg.into())
    }

    /// Create a client error
    pub fn client(msg: impl Into<String>) -> Self {
        Self::Client(msg.into())
    }

    /// Create a configuration error
    pub fn configuration(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    /// Create a timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Create a shutdown error
    pub fn shutdown(msg: impl Into<String>) -> Self {
        Self::Shutdown(msg.into())
    }

    /// Create an RPC error
    pub fn rpc(msg: impl Into<String>) -> Self {
        Self::Rpc(msg.into())
    }
}

impl From<crate::topic::TopicError> for EventBusError {
    fn from(err: crate::topic::TopicError) -> Self {
        Self::Topic(err.to_string())
    }
}
