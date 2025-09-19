//! Unix domain socket implementation for IPC server.

use async_trait::async_trait;
// Removed unused imports
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};

use crate::ipc::server::{ServerCommon, ServerStats};
use crate::ipc::{IpcConfig, IpcError, IpcResult, IpcServer, SimpleMessageHandler};

/// Unix domain socket server implementation
#[derive(Debug)]
pub struct UnixSocketServer {
    common: Arc<RwLock<ServerCommon>>,
    listener: Option<UnixListener>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    #[allow(dead_code)]
    config: IpcConfig,
}

impl UnixSocketServer {
    /// Create a new Unix socket server
    pub fn new(config: IpcConfig) -> IpcResult<Self> {
        // Validate the socket path
        let path = Path::new(&config.path);
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            return Err(IpcError::configuration_error(format!(
                "Socket directory does not exist: {}",
                parent.display()
            )));
        }

        Ok(Self {
            common: Arc::new(RwLock::new(ServerCommon::new(config.clone()))),
            listener: None,
            shutdown_tx: None,
            config,
        })
    }

    /// Handle a single client connection
    async fn handle_connection(
        common: Arc<RwLock<ServerCommon>>,
        mut stream: UnixStream,
        client_addr: String,
    ) {
        let mut buffer = [0; 4096];

        loop {
            // Read message length (4 bytes)
            let length_result = timeout(
                Duration::from_secs(30), // 30 second timeout
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
                Duration::from_secs(30),
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
            let response = match Self::process_message(&common, message_data).await {
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
        let mut common_guard = common.write().await;
        common_guard.stats_mut().record_disconnection();
    }

    /// Process an incoming message
    async fn process_message(
        common: &Arc<RwLock<ServerCommon>>,
        message_data: Vec<u8>,
    ) -> IpcResult<Vec<u8>> {
        // Parse the detection task
        let task = prost::Message::decode(&message_data[..]).map_err(|e| {
            IpcError::invalid_message(format!("Failed to decode DetectionTask: {}", e))
        })?;

        // Get the handler
        let common_guard = common.read().await;
        let handler = common_guard
            .handler()
            .ok_or_else(|| IpcError::invalid_message("No message handler set"))?;

        // Process the task
        let result = handler.handle(task).await?;

        // Update statistics
        drop(common_guard);
        let mut common_guard = common.write().await;
        common_guard.stats_mut().record_message();

        // Encode the response
        let response_data = prost::Message::encode_to_vec(&result);
        Ok(response_data)
    }

    /// Send a response to the client
    async fn send_response(stream: &mut UnixStream, response_data: Vec<u8>) -> IpcResult<()> {
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
impl IpcServer for UnixSocketServer {
    async fn start(&mut self) -> IpcResult<()> {
        let common_guard = self.common.read().await;
        if common_guard.is_running() {
            return Err(IpcError::ServerAlreadyRunning);
        }
        drop(common_guard);

        // Create the Unix listener
        let path = self.common.read().await.config().path.clone();
        let _ = std::fs::remove_file(&path); // Remove existing socket file
        let listener = UnixListener::bind(&path).map_err(IpcError::Io)?;

        // Update server state
        {
            let mut common_guard = self.common.write().await;
            common_guard.record_start();
        }

        self.listener = Some(listener);
        tracing::info!("Unix socket server started on {}", path);

        // Start the server loop
        let common = Arc::clone(&self.common);
        let listener = self.listener.take().unwrap();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Accept new connections
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                let client_addr = format!("{:?}", addr);
                                let common = Arc::clone(&common);

                                // Check connection limit
                                {
                                    let common_guard = common.read().await;
                                    if common_guard.stats().active_connections >= common_guard.config().max_connections {
                                        tracing::warn!("Connection limit exceeded, rejecting connection from {}", client_addr);
                                        continue;
                                    }
                                }

                                // Update statistics
                                {
                                    let mut common_guard = common.write().await;
                                    common_guard.stats_mut().record_connection();
                                }

                                // Handle the connection
                                tokio::spawn(Self::handle_connection(common, stream, client_addr));
                            }
                            Err(e) => {
                                tracing::error!("Failed to accept connection: {}", e);
                                // Update error statistics
                                let mut common_guard = common.write().await;
                                common_guard.stats_mut().record_error();
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        tracing::info!("Unix socket server shutdown requested");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&mut self) -> IpcResult<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Clean up socket file
        if let Some(listener) = self.listener.take() {
            drop(listener);
            let path = self.common.read().await.config().path.clone();
            let _ = std::fs::remove_file(&path);
            tracing::info!("Unix socket server stopped and cleaned up");
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        // For now, we'll return false since we can't use blocking_read in async context
        // In a real implementation, we'd need to track this state differently
        false
    }

    fn config(&self) -> &IpcConfig {
        &self.config
    }

    fn set_handler(&mut self, _handler: SimpleMessageHandler) {
        // This is a bit tricky with Arc<RwLock<>>, we need to handle it in the async context
        // For now, we'll store it in a way that can be accessed by the async tasks
        // In a real implementation, you might want to use a different approach
        tracing::warn!("set_handler called but not fully implemented for UnixSocketServer");
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
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_unix_socket_server_creation() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let config = IpcConfig {
            path: socket_path.to_string_lossy().to_string(),
            max_connections: 5,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        };

        let server = UnixSocketServer::new(config).unwrap();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_unix_socket_server_invalid_path() {
        let config = IpcConfig {
            path: "/nonexistent/directory/socket.sock".to_string(),
            max_connections: 5,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        };

        let result = UnixSocketServer::new(config);
        assert!(result.is_err());
        match result.unwrap_err() {
            IpcError::ConfigurationError { .. } => {} // Expected
            _ => panic!("Expected ConfigurationError"),
        }
    }

    #[test]
    fn test_unix_socket_server_config() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let config = IpcConfig {
            path: socket_path.to_string_lossy().to_string(),
            max_connections: 10,
            connection_timeout_secs: 15,
            message_timeout_secs: 30,
        };

        let server = UnixSocketServer::new(config).unwrap();
        let server_config = server.config();

        assert_eq!(server_config.max_connections, 10);
        assert_eq!(server_config.connection_timeout_secs, 15);
        assert_eq!(server_config.message_timeout_secs, 30);
    }
}
