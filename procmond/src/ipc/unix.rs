//! Unix domain socket implementation for IPC server.

use async_trait::async_trait;
// Removed unused imports
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};

use crate::ipc::protocol::{ProtocolConfig, ProtocolManager};
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
    protocol_manager: Arc<ProtocolManager>,
    running: AtomicBool,
    handler: Arc<RwLock<Option<SimpleMessageHandler>>>,
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

        let protocol_config = ProtocolConfig::default();
        let protocol_manager = Arc::new(ProtocolManager::new(protocol_config));

        Ok(Self {
            common: Arc::new(RwLock::new(ServerCommon::new(config.clone()))),
            listener: None,
            shutdown_tx: None,
            config,
            protocol_manager,
            running: AtomicBool::new(false),
            handler: Arc::new(RwLock::new(None)),
        })
    }

    /// Read a varint length prefix from the stream
    async fn read_varint_length(
        stream: &mut UnixStream,
        timeout_secs: u64,
    ) -> Result<usize, std::io::Error> {
        let mut result = 0u64;
        let mut shift = 0;
        let mut buffer = [0u8; 1];

        loop {
            let read_result = timeout(
                Duration::from_secs(timeout_secs),
                stream.read_exact(&mut buffer),
            )
            .await;

            let byte = match read_result {
                Ok(Ok(_)) => buffer[0],
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Timeout reading varint",
                    ));
                }
            };

            result |= ((byte & 0x7F) as u64) << shift;
            if (byte & 0x80) == 0 {
                if result > usize::MAX as u64 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Varint too large",
                    ));
                }
                return Ok(result as usize);
            }
            shift += 7;
            if shift >= 64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Varint too large",
                ));
            }
        }
    }

    /// Handle a single client connection
    async fn handle_connection(
        common: Arc<RwLock<ServerCommon>>,
        protocol_manager: Arc<ProtocolManager>,
        handler: Arc<RwLock<Option<SimpleMessageHandler>>>,
        mut stream: UnixStream,
        client_addr: String,
        message_timeout_secs: u64,
    ) {
        let mut buffer = [0; 4096];

        loop {
            // Read varint length prefix
            let length = match Self::read_varint_length(&mut stream, message_timeout_secs).await {
                Ok(length) => length,
                Err(e) => {
                    tracing::warn!("Failed to read message length from {}: {}", client_addr, e);
                    break;
                }
            };

            // Enforce maximum message size (16KB)
            const MAX_MESSAGE_SIZE: usize = 16 * 1024;
            if length > MAX_MESSAGE_SIZE {
                tracing::error!(
                    "Message too large from {}: {} bytes (max: {})",
                    client_addr,
                    length,
                    MAX_MESSAGE_SIZE
                );
                break;
            }

            // Read the full frame: sequence (4 bytes) + payload + crc32 (4 bytes)
            let frame_size = 4 + length + 4; // sequence + payload + crc32
            if frame_size > buffer.len() {
                tracing::error!(
                    "Frame too large from {}: {} bytes (buffer: {})",
                    client_addr,
                    frame_size,
                    buffer.len()
                );
                break;
            }

            let frame_result = timeout(
                Duration::from_secs(message_timeout_secs),
                stream.read_exact(&mut buffer[..frame_size]),
            )
            .await;

            let frame_data = match frame_result {
                Ok(Ok(_)) => &buffer[..frame_size],
                Ok(Err(e)) => {
                    tracing::warn!("Failed to read frame from {}: {}", client_addr, e);
                    break;
                }
                Err(_) => {
                    tracing::warn!("Timeout reading frame from {}", client_addr);
                    break;
                }
            };

            // Parse frame: sequence (4 bytes) + payload + crc32 (4 bytes)
            let sequence_bytes = &frame_data[0..4];
            let payload = &frame_data[4..4 + length];
            let crc32_bytes = &frame_data[4 + length..4 + length + 4];

            // Convert sequence number from big-endian
            let sequence_number = u32::from_be_bytes([
                sequence_bytes[0],
                sequence_bytes[1],
                sequence_bytes[2],
                sequence_bytes[3],
            ]);

            // Convert CRC32 from big-endian
            let received_crc32 = u32::from_be_bytes([
                crc32_bytes[0],
                crc32_bytes[1],
                crc32_bytes[2],
                crc32_bytes[3],
            ]);

            // Validate CRC32 over sequence number + payload
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(sequence_bytes);
            hasher.update(payload);
            let computed_crc32 = hasher.finalize();

            if received_crc32 != computed_crc32 {
                tracing::error!(
                    "CRC32 mismatch from {}: received={:08x}, computed={:08x}",
                    client_addr,
                    received_crc32,
                    computed_crc32
                );
                break;
            }

            let message_data = payload.to_vec();

            // Parse and handle the message
            let response =
                match Self::process_message(&common, &protocol_manager, &handler, message_data)
                    .await
                {
                    Ok(response) => response,
                    Err(e) => {
                        tracing::error!("Error processing message from {}: {}", client_addr, e);
                        continue;
                    }
                };

            // Send response
            if let Err(e) = Self::send_response(&mut stream, response, sequence_number).await {
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
        protocol_manager: &ProtocolManager,
        handler: &Arc<RwLock<Option<SimpleMessageHandler>>>,
        message_data: Vec<u8>,
    ) -> IpcResult<Vec<u8>> {
        // Decode using protocol manager
        let (task, _sequence_number) =
            protocol_manager.decode_message::<sentinel_lib::proto::DetectionTask>(&message_data)?;

        // Check flow control
        if !protocol_manager.can_send() {
            return Err(IpcError::invalid_message(
                "Flow control: no credits available",
            ));
        }

        // Get the handler from the shared Arc<RwLock<Option<SimpleMessageHandler>>>
        let handler_guard = handler.read().await;
        let handler = handler_guard
            .as_ref()
            .ok_or_else(|| IpcError::invalid_message("No message handler set"))?;

        // Process the task
        let result = handler.handle(task).await?;

        // Update statistics
        let mut common_guard = common.write().await;
        common_guard.stats_mut().record_message();

        // Encode the response using protocol manager
        let response_data = protocol_manager.encode_message(&result)?;
        Ok(response_data)
    }

    /// Send a response to the client with proper framing
    async fn send_response(
        stream: &mut UnixStream,
        response_data: Vec<u8>,
        sequence_number: u32,
    ) -> IpcResult<()> {
        // Encode varint length prefix
        let mut length_bytes = Vec::new();
        let mut length = response_data.len() as u64;
        while length >= 0x80 {
            length_bytes.push((length as u8) | 0x80);
            length >>= 7;
        }
        length_bytes.push(length as u8);

        // Send varint length prefix
        stream.write_all(&length_bytes).await?;

        // Send sequence number (4 bytes, big-endian)
        stream.write_all(&sequence_number.to_be_bytes()).await?;

        // Send payload
        stream.write_all(&response_data).await?;

        // Compute and send CRC32 over sequence + payload
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&sequence_number.to_be_bytes());
        hasher.update(&response_data);
        let crc32 = hasher.finalize();
        stream.write_all(&crc32.to_be_bytes()).await?;

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
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(e) => {
                self.running.store(false, Ordering::SeqCst);
                return Err(IpcError::Io(e));
            }
        };

        // Update server state
        {
            let mut common_guard = self.common.write().await;
            common_guard.record_start();
        }

        self.listener = Some(listener);
        tracing::info!("Unix socket server started on {}", path);

        // Set running flag to true
        self.running.store(true, Ordering::SeqCst);

        // Start the server loop
        let common = Arc::clone(&self.common);
        let protocol_manager = Arc::clone(&self.protocol_manager);
        let handler = Arc::clone(&self.handler);
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
                                let message_timeout = common.read().await.config().message_timeout_secs as u64;
                                tokio::spawn(Self::handle_connection(common, Arc::clone(&protocol_manager), Arc::clone(&handler), stream, client_addr, message_timeout));
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
        // Set running flag to false
        self.running.store(false, Ordering::SeqCst);

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
        self.running.load(Ordering::SeqCst)
    }

    fn config(&self) -> &IpcConfig {
        &self.config
    }

    fn set_handler(&mut self, handler: SimpleMessageHandler) {
        // Store the handler in the shared Arc<RwLock<Option<SimpleMessageHandler>>>
        // This will be accessible to async tasks
        let handler_arc = Arc::clone(&self.handler);
        tokio::spawn(async move {
            let mut handler_guard = handler_arc.write().await;
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
