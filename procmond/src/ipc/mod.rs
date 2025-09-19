//! IPC (Inter-Process Communication) module for procmond.
//!
//! This module provides the server-side IPC implementation for communication
//! between procmond and sentinelagent. It supports both Unix domain sockets
//! (Linux/macOS) and named pipes (Windows) with proper error handling and
//! graceful shutdown capabilities.

use sentinel_lib::proto::{DetectionResult, DetectionTask};

/// Type alias for the message handler function to reduce complexity
pub type MessageHandlerFn = Box<
    dyn Fn(
            DetectionTask,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = IpcResult<DetectionResult>> + Send>,
        > + Send
        + Sync,
>;

pub mod error;
pub mod server;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;

// Re-export commonly used types
pub use error::{IpcError, IpcResult};
pub use server::IpcServer;

/// Configuration for IPC server setup
#[derive(Debug, Clone)]
pub struct IpcConfig {
    /// Path for Unix socket or named pipe
    pub path: String,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Connection timeout in seconds
    #[allow(dead_code)]
    pub connection_timeout_secs: u64,
    /// Message timeout in seconds
    #[allow(dead_code)]
    pub message_timeout_secs: u64,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            path: "/tmp/sentineld-procmond.sock".to_string(),
            max_connections: 10,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        }
    }
}

/// Simple message handler for IPC messages
pub struct SimpleMessageHandler {
    pub handler: MessageHandlerFn,
    pub name: String,
}

impl std::fmt::Debug for SimpleMessageHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleMessageHandler")
            .field("name", &self.name)
            .field("handler", &"<closure>")
            .finish()
    }
}

impl SimpleMessageHandler {
    pub fn new<F, Fut>(name: String, handler: F) -> Self
    where
        F: Fn(DetectionTask) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = IpcResult<DetectionResult>> + Send + 'static,
    {
        Self {
            handler: Box::new(move |task| Box::pin(handler(task))),
            name,
        }
    }

    pub async fn handle(&self, task: DetectionTask) -> IpcResult<DetectionResult> {
        (self.handler)(task).await
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Platform-specific IPC server factory
#[cfg(unix)]
pub fn create_ipc_server(config: IpcConfig) -> IpcResult<unix::UnixSocketServer> {
    unix::UnixSocketServer::new(config)
}

#[cfg(windows)]
pub fn create_ipc_server(config: IpcConfig) -> IpcResult<windows::NamedPipeServer> {
    windows::NamedPipeServer::new(config)
}

#[cfg(not(any(unix, windows)))]
pub fn create_ipc_server(_config: IpcConfig) -> IpcResult<()> {
    Err(IpcError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_config_default() {
        let config = IpcConfig::default();
        assert_eq!(config.path, "/tmp/sentineld-procmond.sock");
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.connection_timeout_secs, 30);
        assert_eq!(config.message_timeout_secs, 60);
    }

    #[test]
    fn test_ipc_config_custom() {
        let config = IpcConfig {
            path: "/custom/path.sock".to_string(),
            max_connections: 5,
            connection_timeout_secs: 15,
            message_timeout_secs: 30,
        };

        assert_eq!(config.path, "/custom/path.sock");
        assert_eq!(config.max_connections, 5);
        assert_eq!(config.connection_timeout_secs, 15);
        assert_eq!(config.message_timeout_secs, 30);
    }
}
