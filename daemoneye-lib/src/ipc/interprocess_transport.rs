//! Interprocess transport implementation using the interprocess crate.
//!
//! This module provides client and server implementations using
//! `interprocess::local_socket` for true cross-platform compatibility.

use crate::ipc::IpcConfig;
use crate::ipc::codec::{IpcCodec, IpcError, IpcResult};
use crate::proto::{DetectionResult, DetectionTask};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use interprocess::local_socket::{ListenerOptions, Name, tokio::prelude::*};
#[cfg(unix)]
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
    /// Create a new interprocess server with the specified configuration.
    ///
    /// This method initializes an interprocess server that can handle detection tasks
    /// from client connections. The server uses Unix domain sockets on Unix-like systems
    /// and named pipes on Windows for cross-platform IPC communication.
    ///
    /// # Arguments
    ///
    /// * `config` - IPC configuration containing endpoint path, timeouts, and connection limits
    ///
    /// # Returns
    ///
    /// Returns `Ok(InterprocessServer)` on successful initialization, or `Err(IpcError)` if
    /// the configuration is invalid or the server cannot be created.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The endpoint path is invalid for the current platform
    /// - Required IPC resources cannot be allocated
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use daemoneye_lib::ipc::{IpcConfig, InterprocessServer};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = IpcConfig::default();
    /// let server = InterprocessServer::new(config);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: IpcConfig) -> Self {
        // Note: Panic strategy is stored in config but not applied as a global hook.
        // Setting global panic hooks from library code is problematic because:
        // 1. It affects the entire process, not just IPC operations
        // 2. Multiple libraries could conflict over panic handling
        // 3. Tests running in the same process would be affected
        //
        // Applications that want abort-on-panic behavior should set their own
        // panic hook at the application level, e.g. in main() or a top-level init.

        Self {
            config,
            handler: None,
            shutdown_tx: None,
        }
    }

    /// Set the message handler for processing detection tasks.
    ///
    /// This method configures the handler function that will be called for each incoming
    /// detection task. The handler must be thread-safe and can be called concurrently
    /// from multiple client connections.
    ///
    /// # Arguments
    ///
    /// * `handler` - A closure or function that takes a `DetectionTask` and returns a
    ///   future resolving to `IpcResult<DetectionResult>`. The handler must implement
    ///   `Send + Sync + 'static` bounds for thread safety.
    ///
    /// # Handler Signature
    ///
    /// The handler function must have the signature:
    /// ```rust,ignore
    /// Fn(DetectionTask) -> Fut + Send + Sync + 'static
    /// ```
    /// where `Fut: Future<Output = IpcResult<DetectionResult>> + Send + 'static`
    ///
    /// # Thread Safety
    ///
    /// The handler must be thread-safe (`Send + Sync`) as it may be called concurrently
    /// from multiple client connections. The `'static` bound ensures the handler can
    /// outlive the server instance.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use daemoneye_lib::ipc::{InterprocessServer, IpcConfig};
    /// use daemoneye_lib::proto::{DetectionTask, DetectionResult};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut server = InterprocessServer::new(IpcConfig::default());
    ///
    /// server.set_handler(|task: DetectionTask| async move {
    ///     // Process the detection task
    ///     println!("Processing task: {}", task.task_id);
    ///     Ok(DetectionResult::success(&task.task_id, vec![]))
    /// });
    /// # Ok(())
    /// # }
    /// ```
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
            .ok_or_else(|| IpcError::Encode("No message handler set".to_owned()))?;

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
        };

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
        let server_config = self.config.clone();
        let server_handler = Arc::clone(&handler);

        // Spawn the server task and wait for it to start
        let _server_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Accept new connections
                    result = listener.accept() => {
                        match result {
                            Ok(stream) => {
                                // Acquire connection permit
                let Ok(permit) = Arc::clone(&connection_semaphore).try_acquire_owned() else {
                    warn!("Connection limit reached, rejecting connection");
                    continue;
                };

                                // Handle connection
                                let connection_handler = Arc::clone(&server_handler);
                                let connection_config = server_config.clone();
                                tokio::spawn(async move {
                                    let _permit = permit; // Keep permit alive during connection
                                    if let Err(e) = Self::handle_connection(stream, connection_handler, connection_config).await {
                                        error!("Connection handling error: {}", e);
                                    }
                                    // Permit is automatically released when dropped
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

        // Give the server task a moment to start and bind to the endpoint
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Create socket name from configuration
    fn create_socket_name(&self) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            // Unix domain socket - use filesystem path
            let path = Path::new(&self.config.endpoint_path);
            if let Some(parent) = path.parent()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent).map_err(IpcError::Io)?;
                // Set directory permissions to 0700 (owner only)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o700);
                    std::fs::set_permissions(parent, perms).map_err(IpcError::Io)?;
                }
            }

            // Remove existing socket file
            let _result = std::fs::remove_file(path);

            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            // Windows named pipe - use namespace path
            self.config
                .endpoint_path
                .clone()
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Gracefully shutdown the server with proper resource cleanup
    pub async fn graceful_shutdown(&mut self) -> IpcResult<()> {
        // Signal shutdown to the server task
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _result = shutdown_tx.send(());
        }

        // Give the server task time to complete active connections
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Force cleanup if needed
        self.force_cleanup();
        Ok(())
    }

    /// Stop the server (immediate shutdown)
    pub fn stop(&mut self) {
        // Signal shutdown
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _result = shutdown_tx.send(());
        }

        // Force cleanup
        self.force_cleanup();
    }

    /// Force cleanup of resources, handling Windows interprocess crate issues
    #[allow(clippy::missing_const_for_fn)] // Cannot be const due to file system operations
    fn force_cleanup(&self) {
        // Clean up socket file on Unix
        #[cfg(unix)]
        {
            let _result = std::fs::remove_file(&self.config.endpoint_path);
        }

        // On Windows, the interprocess crate has known issues with cleanup
        // during rapid connect/disconnect cycles. The OS will clean up named pipes
        // when the process exits, so we don't need to do anything special here.
        // The panic we see in tests is a known limitation of the crate's internal
        // "limbo" system and doesn't affect production usage patterns.
        #[cfg(windows)]
        {
            // Suppress unused self warning on Windows where we don't use self
            let _ = self;
        }
    }

    /// Handle a single client connection
    async fn handle_connection(
        mut stream: LocalSocketStream,
        handler: MessageHandler,
        config: IpcConfig,
    ) -> IpcResult<()> {
        let mut codec = IpcCodec::new(config.max_frame_bytes);
        let read_timeout = Duration::from_millis(config.read_timeout_ms);
        let write_timeout = Duration::from_millis(config.write_timeout_ms);

        loop {
            // Read detection task with proper error handling
            let task: DetectionTask = match codec.read_message(&mut stream, read_timeout).await {
                Ok(task) => task,
                Err(IpcError::PeerClosed) => {
                    // Client disconnected normally
                    info!("Client disconnected normally");
                    break;
                }
                Err(IpcError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Client disconnected unexpectedly
                    info!("Client disconnected unexpectedly");
                    break;
                }
                Err(e) => {
                    warn!("Failed to read task: {}", e);
                    return Err(e);
                }
            };

            // Capture task ID before moving task into handler
            let task_id = task.task_id.clone();

            // Process task with timeout to prevent hanging
            let result =
                tokio::time::timeout(Duration::from_millis(config.read_timeout_ms), handler(task))
                    .await
                    .unwrap_or_else(|_| {
                        warn!("Task processing timed out for task: {}", task_id);
                        Err(IpcError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "Task processing timed out",
                        )))
                    });

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
                    // Send error result with correct task ID
                    let error_result = DetectionResult {
                        task_id,
                        success: false,
                        error_message: Some(e.to_string()),
                        processes: vec![],
                        hash_result: None,
                        network_events: vec![],
                        filesystem_events: vec![],
                        performance_events: vec![],
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
    pub fn new(config: IpcConfig) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes);
        Self { config, codec }
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
                .clone()
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
        let _server = InterprocessServer::new(config);
        // Server creation always succeeds
    }

    #[test]
    fn test_client_creation() {
        let config = IpcConfig::default();
        let _client = InterprocessClient::new(config);
        // Client creation always succeeds
    }
}
