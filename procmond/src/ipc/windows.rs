//! Windows named pipe implementation for IPC server.

use async_trait::async_trait;
use sentinel_lib::proto::{DetectionResult, DetectionTask};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{Duration, timeout};

use crate::ipc::{
    IpcConfig, IpcError, IpcResult, IpcServer, ServerCommon, ServerStats, SimpleMessageHandler,
};

/// Windows named pipe server implementation
pub struct NamedPipeServer {
    common: Arc<RwLock<ServerCommon>>,
    pipe_handle: Option<tokio::io::DuplexStream>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    handler: Arc<RwLock<Option<SimpleMessageHandler>>>,
    is_running: AtomicBool,
    cached_config: IpcConfig,
}

impl NamedPipeServer {
    /// Create a new named pipe server
    pub fn new(config: IpcConfig) -> IpcResult<Self> {
        // Validate the pipe name
        if config.path.is_empty() {
            return Err(IpcError::configuration_error("Pipe name cannot be empty"));
        }

        // Windows named pipe names should start with \\.\pipe\
        let pipe_name = if config.path.starts_with(r"\\.\pipe\") {
            config.path.clone()
        } else {
            format!(r"\\.\pipe\{}", config.path)
        };

        let mut config = config;
        config.path = pipe_name;

        Ok(Self {
            common: Arc::new(RwLock::new(ServerCommon::new(config.clone()))),
            pipe_handle: None,
            shutdown_tx: None,
            handler: Arc::new(RwLock::new(None)),
            is_running: AtomicBool::new(false),
            cached_config: config,
        })
    }

    /// Handle a single client connection
    async fn handle_connection(
        common: Arc<RwLock<ServerCommon>>,
        handler: Arc<RwLock<Option<SimpleMessageHandler>>>,
        mut stream: tokio::io::DuplexStream,
        client_addr: String,
        message_timeout_secs: u64,
    ) {
        let mut buffer = [0; 4096];

        loop {
            // Read message length (4 bytes)
            let length_result = timeout(
                Duration::from_secs(message_timeout_secs),
                stream.read_exact(&mut buffer[..4]),
            )
            .await;

            let length = match length_result {
                Ok(Ok(_)) => {
                    u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize
                }
                Ok(Err(e)) => {
                    tracing::warn!("Failed to read message length from {}: {}", client_addr, e);
                    break;
                }
                Err(_) => {
                    tracing::warn!("Timeout reading message length from {}", client_addr);
                    break;
                }
            };

            if length > buffer.len() {
                tracing::error!("Message too large from {}: {} bytes", client_addr, length);
                break;
            }

            // Read the actual message
            let message_result = timeout(
                Duration::from_secs(message_timeout_secs),
                stream.read_exact(&mut buffer[..length]),
            )
            .await;

            let message_data = match message_result {
                Ok(Ok(_)) => buffer[..length].to_vec(),
                Ok(Err(e)) => {
                    tracing::warn!("Failed to read message from {}: {}", client_addr, e);
                    break;
                }
                Err(_) => {
                    tracing::warn!("Timeout reading message from {}", client_addr);
                    break;
                }
            };

            // Parse and handle the message
            let response = match Self::process_message(&common, &handler, message_data).await {
                Ok(response) => response,
                Err(e) => {
                    tracing::error!("Error processing message from {}: {}", client_addr, e);
                    continue;
                }
            };

            // Send response
            if let Err(e) = Self::send_response(&mut stream, response).await {
                tracing::warn!("Failed to send response to {}: {}", client_addr, e);
                break;
            }
        }

        // Update statistics
        if let Ok(mut common_guard) = common.write().await {
            common_guard.stats_mut().record_disconnection();
        }
    }

    /// Process an incoming message
    async fn process_message(
        common: &Arc<RwLock<ServerCommon>>,
        handler: &Arc<RwLock<Option<SimpleMessageHandler>>>,
        message_data: Vec<u8>,
    ) -> IpcResult<Vec<u8>> {
        // Parse the detection task
        let task = prost::Message::decode(&message_data[..]).map_err(|e| {
            IpcError::invalid_message(format!("Failed to decode DetectionTask: {}", e))
        })?;

        // Get the handler
        let handler_guard = handler.read().await;
        let handler = handler_guard
            .as_ref()
            .ok_or_else(|| IpcError::invalid_message("No message handler set"))?;

        // Process the task
        let result = handler.handle(task).await?;

        // Update statistics
        drop(handler_guard);
        if let Ok(mut common_guard) = common.write().await {
            common_guard.stats_mut().record_message();
        }

        // Encode the response
        let response_data = result.encode_to_vec();
        Ok(response_data)
    }

    /// Send a response to the client
    async fn send_response(
        stream: &mut tokio::io::DuplexStream,
        response_data: Vec<u8>,
    ) -> IpcResult<()> {
        // Send message length
        let length = response_data.len() as u32;
        stream.write_all(&length.to_be_bytes()).await?;

        // Send the actual message
        stream.write_all(&response_data).await?;
        stream.flush().await?;

        Ok(())
    }
}

#[async_trait]
impl IpcServer for NamedPipeServer {
    async fn start(&mut self) -> IpcResult<()> {
        let common_guard = self.common.read().await;
        if common_guard.is_running() {
            return Err(IpcError::ServerAlreadyRunning);
        }
        drop(common_guard);

        // Ensure we start with running flag set to false
        self.is_running.store(false, Ordering::SeqCst);

        // For Windows named pipes, we'll use a simplified approach
        // In a real implementation, you would use the Windows API to create named pipes
        // For now, we'll simulate the behavior with a duplex stream

        // Update server state
        {
            let mut common_guard = self.common.write().await;
            common_guard.record_start();
        }

        // Set running flag to true
        self.is_running.store(true, Ordering::SeqCst);

        let pipe_name = &self.common.read().await.config().path;
        tracing::info!("Named pipe server started on {}", pipe_name);

        // Start the server loop
        let common = Arc::clone(&self.common);
        let handler = Arc::clone(&self.handler);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        tracing::info!("Named pipe server shutdown requested");
                        break;
                    }

                    // For now, we'll just wait for shutdown
                    // In a real implementation, you would handle pipe connections here
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // This is a placeholder - in reality you would handle pipe connections
                        // When a connection is accepted, it would be handled like:
                        // let message_timeout = common.read().await.config().message_timeout_secs as u64;
                        // tokio::spawn(Self::handle_connection(common, handler, stream, client_addr, message_timeout));
                        continue;
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&mut self) -> IpcResult<()> {
        // Set running flag to false
        self.is_running.store(false, Ordering::SeqCst);

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(pipe_handle) = self.pipe_handle.take() {
            drop(pipe_handle);
            tracing::info!("Named pipe server stopped and cleaned up");
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    fn config(&self) -> &IpcConfig {
        &self.cached_config
    }

    fn set_handler(&mut self, handler: SimpleMessageHandler) {
        // Store the handler in the Arc<RwLock<Option<SimpleMessageHandler>>>
        // We need to use a blocking approach since this is a sync method
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let mut handler_guard = self.handler.write().await;
            *handler_guard = Some(handler);
        });
    }

    async fn get_stats(&self) -> IpcResult<ServerStats> {
        let common_guard = self.common.read().await;
        let mut stats = common_guard.stats().clone();

        // Update uptime
        if let Some(duration) = common_guard.uptime() {
            stats.uptime_seconds = duration.as_secs();
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_named_pipe_server_creation() {
        let config = IpcConfig {
            path: "test-pipe".to_string(),
            max_connections: 5,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        };

        let server = NamedPipeServer::new(config).unwrap();
        assert!(!server.is_running());

        // Check that the pipe name was properly formatted
        let server_config = server.config();
        assert!(server_config.path.starts_with(r"\\.\pipe\"));
    }

    #[tokio::test]
    async fn test_named_pipe_server_empty_name() {
        let config = IpcConfig {
            path: "".to_string(),
            max_connections: 5,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        };

        let result = NamedPipeServer::new(config);
        assert!(result.is_err());
        match result.unwrap_err() {
            IpcError::ConfigurationError { .. } => {} // Expected
            _ => panic!("Expected ConfigurationError"),
        }
    }

    #[test]
    fn test_named_pipe_server_config() {
        let config = IpcConfig {
            path: "test-pipe".to_string(),
            max_connections: 10,
            connection_timeout_secs: 15,
            message_timeout_secs: 30,
        };

        let server = NamedPipeServer::new(config).unwrap();
        let server_config = server.config();

        assert_eq!(server_config.max_connections, 10);
        assert_eq!(server_config.connection_timeout_secs, 15);
        assert_eq!(server_config.message_timeout_secs, 30);
        assert!(server_config.path.starts_with(r"\\.\pipe\"));
    }

    #[test]
    fn test_named_pipe_server_existing_pipe_name() {
        let config = IpcConfig {
            path: r"\\.\pipe\existing-pipe".to_string(),
            max_connections: 5,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        };

        let server = NamedPipeServer::new(config).unwrap();
        let server_config = server.config();

        // Should not double-prefix the pipe name
        assert_eq!(server_config.path, r"\\.\pipe\existing-pipe");
    }
}
