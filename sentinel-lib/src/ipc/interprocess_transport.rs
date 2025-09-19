//! Interprocess transport implementation using the interprocess crate.
//!
//! This module provides client and server implementations using
//! `interprocess::local_socket` for true cross-platform compatibility.

use crate::ipc::IpcConfig;
use crate::ipc::codec::{IpcCodec, IpcError, IpcResult};
use crate::proto::{DetectionResult, DetectionTask};
use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, Name, ToFsName, tokio::prelude::*,
};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Semaphore, oneshot};
use tokio::time::Duration;
use tracing::{error, info, warn};

/// Message handler type for processing detection tasks
type MessageHandler = Arc<
    dyn Fn(
            DetectionTask,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = IpcResult<DetectionResult>> + Send>,
        > + Send
        + Sync,
>;

/// Interprocess-based IPC server implementation
pub struct InterprocessServer {
    config: IpcConfig,
    handler: Option<MessageHandler>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl InterprocessServer {
    /// Create a new interprocess server
    pub fn new(config: IpcConfig) -> IpcResult<Self> {
        Ok(Self {
            config,
            handler: None,
            shutdown_tx: None,
        })
    }

    /// Set the message handler
    pub fn set_handler<F, Fut>(&mut self, handler: F)
    where
        F: Fn(DetectionTask) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = IpcResult<DetectionResult>> + Send + 'static,
    {
        self.handler = Some(Arc::new(move |task| Box::pin(handler(task))));
    }

    /// Start the server
    pub async fn start(&mut self) -> IpcResult<()> {
        let handler = self
            .handler
            .clone()
            .ok_or_else(|| IpcError::Encode("No message handler set".to_string()))?;

        // Create socket name using interprocess crate
        let name = self.create_socket_name()?;

        // Create listener with proper options
        let opts = ListenerOptions::new().name(name);
        let listener = match opts.create_tokio() {
            Ok(listener) => listener,
            Err(e) => {
                error!("Failed to bind local socket: {}", e);
                return Err(IpcError::Io(e));
            }
        };

        #[cfg(unix)]
        {
            // Set socket permissions to 0600 (owner only)
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.config.endpoint_path, perms).map_err(IpcError::Io)?;
        }

        info!(
            "Interprocess server starting on {}",
            self.config.endpoint_path
        );

        // Create semaphore for connection limiting
        let connection_semaphore = Arc::new(Semaphore::new(self.config.max_connections));

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        // Clone config for the server task
        let config = self.config.clone();
        let handler = Arc::clone(&handler);

        // Spawn the server task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Accept new connections
                    result = listener.accept() => {
                        match result {
                            Ok(stream) => {
                                // Acquire connection permit
                                let permit = match connection_semaphore.clone().try_acquire_owned() {
                                    Ok(permit) => permit,
                                    Err(_) => {
                                        warn!("Connection limit reached, rejecting connection");
                                        continue;
                                    }
                                };

                                // Handle connection
                                let handler = Arc::clone(&handler);
                                let config = config.clone();
                                tokio::spawn(async move {
                                    let _permit = permit; // Keep permit alive during connection
                                    if let Err(e) = Self::handle_connection(stream, handler, config).await {
                                        error!("Connection handling error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Failed to accept connection: {}", e);
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("Interprocess server shutdown requested");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Create socket name from configuration
    fn create_socket_name(&self) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            // Unix domain socket - use filesystem path
            let path = Path::new(&self.config.endpoint_path);
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(IpcError::Io)?;
                    // Set directory permissions to 0700 (owner only)
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let perms = std::fs::Permissions::from_mode(0o700);
                        std::fs::set_permissions(parent, perms).map_err(IpcError::Io)?;
                    }
                }
            }

            // Remove existing socket file
            let _ = std::fs::remove_file(path);

            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            // Windows named pipe - use namespace path
            self.config
                .endpoint_path
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Stop the server
    pub async fn stop(&mut self) -> IpcResult<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Clean up socket file on Unix
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.config.endpoint_path);
        }

        Ok(())
    }

    /// Handle a single client connection
    async fn handle_connection(
        mut stream: LocalSocketStream,
        handler: MessageHandler,
        config: IpcConfig,
    ) -> IpcResult<()> {
        let mut codec = IpcCodec::new(config.max_frame_bytes, config.crc32_variant.clone());
        let read_timeout = Duration::from_millis(config.read_timeout_ms);
        let write_timeout = Duration::from_millis(config.write_timeout_ms);

        loop {
            // Read detection task
            let task: DetectionTask = match codec.read_message(&mut stream, read_timeout).await {
                Ok(task) => task,
                Err(IpcError::PeerClosed) => {
                    // Client disconnected normally
                    break;
                }
                Err(e) => {
                    warn!("Failed to read task: {}", e);
                    return Err(e);
                }
            };

            // Process task
            let result = handler(task).await;

            match result {
                Ok(detection_result) => {
                    // Send successful result
                    if let Err(e) = codec
                        .write_message(&mut stream, &detection_result, write_timeout)
                        .await
                    {
                        warn!("Failed to write result: {}", e);
                        return Err(e);
                    }
                }
                Err(e) => {
                    // Send error result
                    let error_result = DetectionResult {
                        task_id: "unknown".to_string(), // We don't have task_id from the error
                        success: false,
                        error_message: Some(e.to_string()),
                        processes: vec![],
                        hash_result: None,
                    };

                    if let Err(write_err) = codec
                        .write_message(&mut stream, &error_result, write_timeout)
                        .await
                    {
                        warn!("Failed to write error result: {}", write_err);
                        return Err(write_err);
                    }
                }
            }
        }

        Ok(())
    }
}

/// Interprocess-based IPC client implementation  
pub struct InterprocessClient {
    config: IpcConfig,
    codec: IpcCodec,
}

impl InterprocessClient {
    /// Create a new interprocess client
    pub fn new(config: IpcConfig) -> IpcResult<Self> {
        let codec = IpcCodec::new(config.max_frame_bytes, config.crc32_variant.clone());
        Ok(Self { config, codec })
    }

    /// Create socket name from configuration
    fn create_socket_name(&self) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            // Unix domain socket - use filesystem path
            let path = Path::new(&self.config.endpoint_path);
            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            // Windows named pipe - use namespace path
            self.config
                .endpoint_path
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Send a detection task and receive the result
    pub async fn send_task(&mut self, task: DetectionTask) -> IpcResult<DetectionResult> {
        // Create socket name and connect
        let name = self.create_socket_name()?;
        let mut stream = LocalSocketStream::connect(name)
            .await
            .map_err(IpcError::Io)?;

        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);
        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);

        // Send task
        self.codec
            .write_message(&mut stream, &task, write_timeout)
            .await?;

        // Receive result
        let result = self.codec.read_message(&mut stream, read_timeout).await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let config = IpcConfig::default();
        let server = InterprocessServer::new(config);
        assert!(server.is_ok());
    }

    #[test]
    fn test_client_creation() {
        let config = IpcConfig::default();
        let client = InterprocessClient::new(config);
        assert!(client.is_ok());
    }
}
