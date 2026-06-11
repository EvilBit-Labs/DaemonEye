//! Error types for the RPC service handler.

use daemoneye_eventbus::rpc::CollectorOperation;
use thiserror::Error;

/// Errors that can occur in the RPC service handler.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RpcServiceError {
    /// Failed to subscribe to the control topic.
    #[error("Failed to subscribe to control topic: {0}")]
    SubscriptionFailed(String),

    /// Failed to publish RPC response.
    #[error("Failed to publish RPC response: {0}")]
    PublishFailed(String),

    /// Failed to forward request to actor.
    #[error("Failed to forward request to actor: {0}")]
    ActorError(String),

    /// Invalid RPC request.
    #[error("Invalid RPC request: {0}")]
    InvalidRequest(String),

    /// Operation not supported.
    #[error("Operation not supported: {operation:?}")]
    UnsupportedOperation { operation: CollectorOperation },

    /// Operation timed out.
    #[error("Operation timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Service is shutting down.
    #[error("Service is shutting down")]
    ShuttingDown,
}

/// Result type for RPC service operations.
pub type RpcServiceResult<T> = Result<T, RpcServiceError>;
