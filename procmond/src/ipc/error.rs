//! IPC-specific error types and handling.

use std::error::Error;
use std::io;

/// Result type for IPC operations
pub type IpcResult<T> = Result<T, IpcError>;

/// IPC-specific error types
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// I/O error during IPC operations
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Protocol buffer serialization/deserialization error
    #[error("Protobuf error: {0}")]
    Protobuf(#[from] prost::DecodeError),

    /// Connection timeout
    #[error("Connection timeout after {timeout_secs} seconds")]
    #[allow(dead_code)]
    ConnectionTimeout { timeout_secs: u64 },

    /// Message timeout
    #[error("Message timeout after {timeout_secs} seconds")]
    #[allow(dead_code)]
    MessageTimeout { timeout_secs: u64 },

    /// Server is not running
    #[error("Server is not running")]
    #[allow(dead_code)]
    ServerNotRunning,

    /// Server is already running
    #[error("Server is already running")]
    ServerAlreadyRunning,

    /// Invalid message format
    #[error("Invalid message format: {reason}")]
    InvalidMessage { reason: String },

    /// Connection limit exceeded
    #[error("Connection limit exceeded: {current}/{max}")]
    #[allow(dead_code)]
    ConnectionLimitExceeded { current: usize, max: usize },

    /// Platform not supported
    #[error("Platform not supported for IPC operations")]
    #[allow(dead_code)]
    UnsupportedPlatform,

    /// Server shutdown in progress
    #[error("Server shutdown in progress")]
    #[allow(dead_code)]
    ShutdownInProgress,

    /// Handler error
    #[error("Message handler error: {0}")]
    HandlerError(#[from] Box<dyn Error + Send + Sync>),

    /// Configuration error
    #[error("Configuration error: {reason}")]
    ConfigurationError { reason: String },
}

impl IpcError {
    /// Create a new connection timeout error
    #[allow(dead_code)]
    pub fn connection_timeout(timeout_secs: u64) -> Self {
        Self::ConnectionTimeout { timeout_secs }
    }

    /// Create a new message timeout error
    #[allow(dead_code)]
    pub fn message_timeout(timeout_secs: u64) -> Self {
        Self::MessageTimeout { timeout_secs }
    }

    /// Create a new invalid message error
    pub fn invalid_message(reason: impl Into<String>) -> Self {
        Self::InvalidMessage {
            reason: reason.into(),
        }
    }

    /// Create a new connection limit exceeded error
    #[allow(dead_code)]
    pub fn connection_limit_exceeded(current: usize, max: usize) -> Self {
        Self::ConnectionLimitExceeded { current, max }
    }

    /// Create a new configuration error
    pub fn configuration_error(reason: impl Into<String>) -> Self {
        Self::ConfigurationError {
            reason: reason.into(),
        }
    }

    /// Check if this error is retryable
    #[allow(dead_code)]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            IpcError::Io(_) | IpcError::ConnectionTimeout { .. } | IpcError::MessageTimeout { .. }
        )
    }

    /// Check if this error indicates the server should shutdown
    #[allow(dead_code)]
    pub fn should_shutdown(&self) -> bool {
        matches!(
            self,
            IpcError::ShutdownInProgress | IpcError::ServerNotRunning
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_ipc_error_creation() {
        let io_error = IpcError::Io(io::Error::new(io::ErrorKind::NotFound, "File not found"));
        assert!(io_error.is_retryable());

        let timeout_error = IpcError::connection_timeout(30);
        assert!(timeout_error.is_retryable());

        let message_timeout = IpcError::message_timeout(60);
        assert!(message_timeout.is_retryable());

        let invalid_message = IpcError::invalid_message("Invalid format");
        assert!(!invalid_message.is_retryable());

        let connection_limit = IpcError::connection_limit_exceeded(10, 5);
        assert!(!connection_limit.is_retryable());
    }

    #[test]
    fn test_ipc_error_shutdown() {
        let shutdown_error = IpcError::ShutdownInProgress;
        assert!(shutdown_error.should_shutdown());

        let server_not_running = IpcError::ServerNotRunning;
        assert!(server_not_running.should_shutdown());

        let io_error = IpcError::Io(io::Error::new(io::ErrorKind::NotFound, "File not found"));
        assert!(!io_error.should_shutdown());
    }

    #[test]
    fn test_ipc_error_display() {
        let timeout_error = IpcError::connection_timeout(30);
        assert!(timeout_error.to_string().contains("Connection timeout"));
        assert!(timeout_error.to_string().contains("30"));

        let message_timeout = IpcError::message_timeout(60);
        assert!(message_timeout.to_string().contains("Message timeout"));
        assert!(message_timeout.to_string().contains("60"));

        let invalid_message = IpcError::invalid_message("Invalid format");
        assert!(
            invalid_message
                .to_string()
                .contains("Invalid message format")
        );
        assert!(invalid_message.to_string().contains("Invalid format"));
    }

    #[test]
    fn test_ipc_error_from_io() {
        let io_error = io::Error::new(io::ErrorKind::PermissionDenied, "Permission denied");
        let ipc_error: IpcError = io_error.into();

        match ipc_error {
            IpcError::Io(_) => {} // Expected
            _ => panic!("Expected Io error variant"),
        }
    }
}
