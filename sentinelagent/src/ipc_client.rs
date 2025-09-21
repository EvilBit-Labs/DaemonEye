//! IPC client implementation for sentinelagent.
//!
//! This module provides a high-level IPC client for sentinelagent to communicate
//! with procmond using the interprocess crate with protobuf messages and CRC32
//! integrity validation. It includes connection management, automatic reconnection,
//! and robust error handling.

use anyhow::{Context, Result as AnyhowResult};
use sentinel_lib::ipc::{IpcCodec, IpcConfig};
use sentinel_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

// Import interprocess types
use interprocess::local_socket::Name;
use interprocess::local_socket::tokio::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};

/// IPC client manager for sentinelagent
pub struct IpcClientManager {
    config: IpcConfig,
    codec: IpcCodec,
    is_connected: bool,
    reconnect_attempts: u32,
    max_reconnect_attempts: u32,
    base_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

impl IpcClientManager {
    /// Create a new IPC client manager with the given configuration
    pub fn new(config: IpcConfig) -> AnyhowResult<Self> {
        let codec = IpcCodec::new(config.max_frame_bytes);

        Ok(Self {
            config,
            codec,
            is_connected: false,
            reconnect_attempts: 0,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        })
    }

    /// Wait for procmond to become available with the given timeout
    pub async fn wait_for_procmond(&mut self, timeout_duration: Duration) -> AnyhowResult<()> {
        let start_time = std::time::Instant::now();
        let mut attempt = 0;

        while start_time.elapsed() < timeout_duration {
            attempt += 1;
            debug!(attempt, "Attempting to connect to procmond");

            match self.test_connection().await {
                Ok(()) => {
                    info!("Successfully connected to procmond");
                    self.is_connected = true;
                    self.reconnect_attempts = 0;
                    return Ok(());
                }
                Err(e) => {
                    debug!(error = %e, attempt, "Connection attempt failed");

                    if start_time.elapsed() + Duration::from_millis(500) < timeout_duration {
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        anyhow::bail!(
            "Failed to connect to procmond within {:?}",
            timeout_duration
        )
    }

    /// Test the connection to procmond
    async fn test_connection(&mut self) -> AnyhowResult<()> {
        // Create socket name from configuration
        let name = self
            .create_socket_name()
            .context("Failed to create socket name")?;

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let mut stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .context("Connection timeout")?
            .context("Failed to connect to procmond socket")?;

        // Test the connection by sending a simple ping task
        let ping_task = DetectionTask::new_test_task(
            format!("ping-{}", chrono::Utc::now().timestamp_millis()),
            TaskType::EnumerateProcesses,
            Some("connection_test".to_string()),
        );

        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);
        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);

        // Send ping task
        timeout(
            write_timeout,
            self.codec
                .write_message(&mut stream, &ping_task, write_timeout),
        )
        .await
        .context("Write timeout during connection test")?
        .with_context(|| {
            error!("Failed to write ping task");
            "Failed to write ping task to procmond"
        })?;

        // Read response
        let _response: DetectionResult = timeout(
            read_timeout,
            self.codec.read_message(&mut stream, read_timeout),
        )
        .await
        .context("Read timeout during connection test")?
        .with_context(|| {
            error!("Failed to read ping response");
            "Failed to read ping response from procmond"
        })?;

        debug!("Connection test successful");
        Ok(())
    }

    /// Create socket name from configuration
    fn create_socket_name(&self) -> AnyhowResult<Name<'_>> {
        #[cfg(unix)]
        {
            use std::path::Path;
            let path = Path::new(&self.config.endpoint_path);
            path.to_fs_name::<GenericFilePath>()
                .with_context(|| format!("Invalid Unix socket path: {}", self.config.endpoint_path))
        }
        #[cfg(windows)]
        {
            self.config
                .endpoint_path
                .clone()
                .to_ns_name::<GenericNamespaced>()
                .with_context(|| {
                    format!(
                        "Invalid Windows named pipe path: {}",
                        self.config.endpoint_path
                    )
                })
        }
    }

    /// Enumerate processes by sending a task to procmond
    pub async fn enumerate_processes(&mut self) -> AnyhowResult<DetectionResult> {
        if !self.is_connected {
            anyhow::bail!("Not connected to procmond");
        }

        let task = DetectionTask::new_test_task(
            format!("enumerate-{}", chrono::Utc::now().timestamp_millis()),
            TaskType::EnumerateProcesses,
            Some("process_enumeration".to_string()),
        );

        self.send_task_with_retry(task).await
    }

    /// Send a detection task with automatic retry and reconnection logic
    async fn send_task_with_retry(&mut self, task: DetectionTask) -> AnyhowResult<DetectionResult> {
        let task_id = task.task_id.clone();
        let mut last_error = None;

        for attempt in 0..=self.max_reconnect_attempts {
            match self.attempt_send_task(&task).await {
                Ok(result) => {
                    self.handle_success().await;
                    return Ok(result);
                }
                Err(e) => {
                    last_error = Some(e);
                    self.handle_failure(attempt, &task_id).await;
                }
            }
        }

        // All retry attempts failed
        anyhow::bail!(
            "All retry attempts failed for task {}: {}",
            task_id,
            last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Unknown error".to_string())
        )
    }

    /// Attempt to send a task with a single connection attempt
    async fn attempt_send_task(&mut self, task: &DetectionTask) -> AnyhowResult<DetectionResult> {
        let name = self
            .create_socket_name()
            .context("Failed to create socket name for task")?;

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let mut stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .context("Connection timeout")?
            .context("Failed to connect to procmond socket")?;

        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);
        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);

        // Send task with timeout
        timeout(
            write_timeout,
            self.codec.write_message(&mut stream, task, write_timeout),
        )
        .await
        .context("Write timeout")?
        .with_context(|| {
            error!("Failed to write task: {}", task.task_id);
            "Failed to write task to procmond"
        })?;

        // Receive result with timeout
        let result = timeout(
            read_timeout,
            self.codec.read_message(&mut stream, read_timeout),
        )
        .await
        .context("Read timeout")?
        .with_context(|| {
            error!("Failed to read result for task: {}", task.task_id);
            "Failed to read result from procmond"
        })?;

        Ok(result)
    }

    /// Handle successful operation
    async fn handle_success(&mut self) {
        self.reconnect_attempts = 0;
        self.is_connected = true;
        debug!("Operation successful, connection is healthy");
    }

    /// Handle operation failure
    async fn handle_failure(&mut self, attempt: u32, task_id: &str) {
        self.is_connected = false;

        if attempt < self.max_reconnect_attempts {
            self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
            let backoff_delay = self.calculate_backoff_delay();

            warn!(
                task_id = %task_id,
                attempt = attempt.saturating_add(1),
                max_attempts = self.max_reconnect_attempts,
                backoff_ms = backoff_delay.as_millis(),
                "Task send failed, retrying with backoff"
            );

            sleep(backoff_delay).await;
        } else {
            error!(
                task_id = %task_id,
                attempts = self.max_reconnect_attempts.saturating_add(1),
                "Task send failed after all retry attempts"
            );
        }
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff_delay(&self) -> Duration {
        let delay_ms = u64::try_from(self.base_reconnect_delay.as_millis())
            .unwrap_or(100)
            .saturating_mul(2_u64.pow(self.reconnect_attempts.min(10))); // Cap at 2^10 to prevent overflow

        let delay = Duration::from_millis(delay_ms);
        std::cmp::min(delay, self.max_reconnect_delay)
    }

    /// Check if the client is healthy
    pub async fn is_healthy(&self) -> bool {
        self.is_connected
    }

    /// Force a reconnection attempt
    pub async fn force_reconnect(&mut self) -> AnyhowResult<()> {
        self.is_connected = false;
        self.reconnect_attempts = 0;

        info!("Forcing reconnection to procmond");
        self.test_connection()
            .await
            .context("Failed to force reconnection to procmond")?;

        self.is_connected = true;
        info!("Forced reconnection successful");
        Ok(())
    }
}

/// Create a default IPC configuration for sentinelagent
///
/// This configuration follows the coding guidelines with:
/// - Max message size: 16KB (default 8KB)
/// - Request timeout: 10 seconds
/// - Conservative settings for production use
pub fn create_default_ipc_config() -> IpcConfig {
    IpcConfig {
        transport: sentinel_lib::ipc::TransportType::Interprocess,
        endpoint_path: default_endpoint_path(),
        max_frame_bytes: 16 * 1024, // 16KB - follows coding guidelines
        accept_timeout_ms: 10000,   // 10 seconds - follows coding guidelines
        read_timeout_ms: 30000,     // 30 seconds
        write_timeout_ms: 10000,    // 10 seconds
        max_connections: 4,
        panic_strategy: sentinel_lib::ipc::PanicStrategy::Abort, // Production configuration
    }
}

/// Create an IPC configuration with maximum allowed values
///
/// This configuration uses the maximum allowed values for high-throughput
/// scenarios where larger messages and longer timeouts are acceptable.
#[allow(dead_code)] // Utility function for future use
pub fn create_maximum_ipc_config() -> IpcConfig {
    IpcConfig {
        transport: sentinel_lib::ipc::TransportType::Interprocess,
        endpoint_path: default_endpoint_path(),
        max_frame_bytes: 1024 * 1024, // 1MB - maximum allowed
        accept_timeout_ms: 30000,     // 30 seconds - maximum timeout
        read_timeout_ms: 60000,       // 60 seconds - maximum read timeout
        write_timeout_ms: 30000,      // 30 seconds - maximum write timeout
        max_connections: 16,          // Maximum connections
        panic_strategy: sentinel_lib::ipc::PanicStrategy::Abort, // Production configuration
    }
}

/// Get the default endpoint path based on the platform
fn default_endpoint_path() -> String {
    #[cfg(unix)]
    {
        "/var/run/sentineld/procmond.sock".to_owned()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\sentineld\procmond".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (IpcConfig, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: sentinel_lib::ipc::TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            panic_strategy: sentinel_lib::ipc::PanicStrategy::Unwind,
        };

        (config, temp_dir)
    }

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-ipc-client.sock")
                .to_string_lossy()
                .to_string()
        }
        #[cfg(windows)]
        {
            let dir_name = temp_dir
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("test");
            format!(r"\\.\pipe\sentineld\test-ipc-client-{}", dir_name)
        }
    }

    #[test]
    fn test_client_creation() {
        let (config, _temp_dir) = create_test_config();
        let _client = IpcClientManager::new(config).expect("Failed to create client");
        // Client creation always succeeds
    }

    #[test]
    fn test_default_config_creation() {
        let config = create_default_ipc_config();
        assert_eq!(config.max_frame_bytes, 16 * 1024); // 16KB - follows coding guidelines
        assert_eq!(config.accept_timeout_ms, 10000); // 10 seconds - follows coding guidelines
        assert_eq!(config.read_timeout_ms, 30000);
        assert_eq!(config.write_timeout_ms, 10000);
        assert_eq!(config.max_connections, 4);
    }

    #[test]
    fn test_backoff_calculation() {
        let (config, _temp_dir) = create_test_config();
        let mut client = IpcClientManager::new(config).expect("Failed to create client");

        // Test exponential backoff
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(100));

        // Simulate multiple attempts
        client.reconnect_attempts = 1;
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(200));

        client.reconnect_attempts = 2;
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(400));

        // Test max delay cap
        client.reconnect_attempts = 20; // Very high number
        let delay = client.calculate_backoff_delay();
        assert!(delay <= client.max_reconnect_delay);
    }

    #[test]
    fn test_default_endpoint_path() {
        let path = default_endpoint_path();
        #[cfg(unix)]
        assert!(path.contains("procmond.sock"));
        #[cfg(windows)]
        assert!(path.contains(r"\\.\pipe\"));
    }
}
