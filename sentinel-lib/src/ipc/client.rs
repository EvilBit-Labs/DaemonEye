//! Advanced IPC client implementation with connection management and resilience.
//!
//! This module provides a robust client implementation for sentinelagent that includes
//! automatic reconnection, connection pooling, circuit breaker patterns, and comprehensive
//! error handling for reliable communication with procmond.

use crate::ipc::IpcConfig;
use crate::ipc::codec::{IpcCodec, IpcError, IpcResult};
use crate::proto::{DetectionResult, DetectionTask};
use interprocess::local_socket::Name;
use interprocess::local_socket::tokio::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, warn};

/// Connection state for the IPC client
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionState {
    /// Client is disconnected
    Disconnected,
    /// Client is attempting to connect
    Connecting,
    /// Client is connected and ready
    Connected,
    /// Client is in a failed state and will retry
    Failed,
    /// Client is in circuit breaker state (temporarily disabled)
    CircuitBreakerOpen,
}

/// Circuit breaker state for failure handling
#[derive(Debug, Clone)]
struct CircuitBreaker {
    failure_count: u32,
    last_failure_time: Option<Instant>,
    failure_threshold: u32,
    recovery_timeout: Duration,
    is_open: bool,
}

impl CircuitBreaker {
    const fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            failure_count: 0,
            last_failure_time: None,
            failure_threshold,
            recovery_timeout,
            is_open: false,
        }
    }

    const fn record_success(&mut self) {
        self.failure_count = 0;
        self.last_failure_time = None;
        self.is_open = false;
    }

    fn record_failure(&mut self) {
        self.failure_count = self.failure_count.saturating_add(1);
        self.last_failure_time = Some(Instant::now());

        if self.failure_count >= self.failure_threshold {
            self.is_open = true;
            warn!(
                failure_count = self.failure_count,
                threshold = self.failure_threshold,
                "Circuit breaker opened due to failure threshold"
            );
        }
    }

    fn should_attempt_connection(&self) -> bool {
        if !self.is_open {
            return true;
        }

        if let Some(last_failure) = self.last_failure_time {
            if last_failure.elapsed() >= self.recovery_timeout {
                debug!("Circuit breaker recovery timeout reached, allowing connection attempt");
                return true;
            }
        }

        false
    }

    const fn is_open(&self) -> bool {
        self.is_open
    }
}

/// Connection pool for managing multiple connections
#[derive(Debug)]
struct ConnectionPool {
    #[allow(dead_code)]
    max_connections: usize,
}

impl ConnectionPool {
    const fn new(max_connections: usize) -> Self {
        Self { max_connections }
    }
}

/// Advanced IPC client with connection management and resilience
pub struct ResilientIpcClient {
    config: IpcConfig,
    codec: IpcCodec,
    state: Arc<Mutex<ConnectionState>>,
    circuit_breaker: Arc<Mutex<CircuitBreaker>>,
    #[allow(dead_code)]
    connection_pool: Arc<Mutex<ConnectionPool>>,
    reconnect_attempts: u32,
    max_reconnect_attempts: u32,
    base_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

impl ResilientIpcClient {
    /// Create a new resilient IPC client
    pub fn new(config: IpcConfig) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes, config.crc32_variant.clone());

        Self {
            config,
            codec,
            state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
            circuit_breaker: Arc::new(Mutex::new(CircuitBreaker::new(
                5,                       // failure threshold
                Duration::from_secs(30), // recovery timeout
            ))),
            connection_pool: Arc::new(Mutex::new(ConnectionPool::new(4))), // max 4 connections
            reconnect_attempts: 0,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }

    /// Create socket name from configuration
    fn create_socket_name(&self) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            use std::path::Path;
            let path = Path::new(&self.config.endpoint_path);
            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            self.config
                .endpoint_path
                .clone()
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Establish a new connection
    async fn establish_connection(&self) -> IpcResult<LocalSocketStream> {
        let name = self.create_socket_name()?;

        // Check circuit breaker
        {
            let breaker = self.circuit_breaker.lock().await;
            if !breaker.should_attempt_connection() {
                return Err(IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "Circuit breaker is open",
                )));
            }
        }

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .map_err(|_err| {
                IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Connection timeout",
                ))
            })?
            .map_err(IpcError::Io)?;

        // Update state to connected
        {
            let mut state = self.state.lock().await;
            *state = ConnectionState::Connected;
        };

        // Record success in circuit breaker
        {
            let mut breaker = self.circuit_breaker.lock().await;
            breaker.record_success();
        };

        debug!("Successfully established IPC connection");
        Ok(stream)
    }

    /// Get a connection from the pool or create a new one
    async fn get_connection(&self) -> IpcResult<LocalSocketStream> {
        // For now, always create a new connection
        // In a more sophisticated implementation, we could reuse connections
        self.establish_connection().await
    }

    /// Release a connection back to the pool
    fn release_connection(stream: LocalSocketStream) {
        // Connection is automatically released when dropped
        drop(stream);
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff_delay(&self) -> Duration {
        let delay_ms = u64::try_from(self.base_reconnect_delay.as_millis())
            .unwrap_or(100)
            .saturating_mul(2_u64.pow(self.reconnect_attempts.min(10))); // Cap at 2^10 to prevent overflow

        let delay = Duration::from_millis(delay_ms);
        std::cmp::min(delay, self.max_reconnect_delay)
    }

    /// Send a detection task with automatic reconnection and retry logic
    pub async fn send_task(&mut self, task: DetectionTask) -> IpcResult<DetectionResult> {
        let task_id = task.task_id.clone();
        let mut last_error = None;

        for attempt in 0..=self.max_reconnect_attempts {
            // Check circuit breaker before attempting
            if !self.should_attempt_connection().await {
                return Err(IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "Circuit breaker is open",
                )));
            }

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
        Err(last_error
            .unwrap_or_else(|| IpcError::Io(std::io::Error::other("All retry attempts failed"))))
    }

    /// Check if connection should be attempted based on circuit breaker
    async fn should_attempt_connection(&self) -> bool {
        let breaker = self.circuit_breaker.lock().await;
        !breaker.is_open() || breaker.should_attempt_connection()
    }

    /// Handle successful operation
    async fn handle_success(&mut self) {
        self.reconnect_attempts = 0;
        let mut breaker = self.circuit_breaker.lock().await;
        breaker.record_success();
    }

    /// Handle operation failure
    async fn handle_failure(&mut self, attempt: u32, task_id: &str) {
        // Record failure in circuit breaker
        {
            let mut breaker = self.circuit_breaker.lock().await;
            breaker.record_failure();
        };

        // Update state to failed
        {
            let mut state = self.state.lock().await;
            *state = ConnectionState::Failed;
        };

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

    /// Attempt to send a task with a single connection attempt
    async fn attempt_send_task(&mut self, task: &DetectionTask) -> IpcResult<DetectionResult> {
        let mut stream = self.get_connection().await?;

        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);
        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);

        // Send task with timeout
        timeout(
            write_timeout,
            self.codec.write_message(&mut stream, task, write_timeout),
        )
        .await
        .map_err(|_err| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Write timeout",
            ))
        })?
        .map_err(|e| {
            error!("Failed to write task: {}", e);
            e
        })?;

        // Receive result with timeout
        let result = timeout(
            read_timeout,
            self.codec.read_message(&mut stream, read_timeout),
        )
        .await
        .map_err(|_err| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Read timeout",
            ))
        })?
        .map_err(|e| {
            error!("Failed to read result: {}", e);
            e
        })?;

        // Release connection back to pool
        Self::release_connection(stream);

        Ok(result)
    }

    /// Get the current connection state
    pub async fn get_connection_state(&self) -> ConnectionState {
        let state = self.state.lock().await;
        state.clone()
    }

    /// Check if the client is healthy
    pub async fn health_check(&self) -> bool {
        let state = self.state.lock().await;
        matches!(*state, ConnectionState::Connected)
    }

    /// Force a reconnection attempt
    pub async fn force_reconnect(&mut self) -> IpcResult<()> {
        {
            let mut state = self.state.lock().await;
            *state = ConnectionState::Disconnected;
        };

        self.reconnect_attempts = 0;
        self.establish_connection().await?;
        Ok(())
    }

    /// Get connection statistics
    pub async fn get_stats(&self) -> ClientStats {
        let state = self.state.lock().await;
        let breaker = self.circuit_breaker.lock().await;

        ClientStats {
            connection_state: state.clone(),
            failure_count: breaker.failure_count,
            is_circuit_breaker_open: breaker.is_open(),
            reconnect_attempts: self.reconnect_attempts,
        }
    }
}

/// Statistics about the client's state
#[derive(Debug, Clone)]
pub struct ClientStats {
    pub connection_state: ConnectionState,
    pub failure_count: u32,
    pub is_circuit_breaker_open: bool,
    pub reconnect_attempts: u32,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (IpcConfig, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: crate::ipc::TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            crc32_variant: crate::ipc::Crc32Variant::Ieee,
        };

        (config, temp_dir)
    }

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-resilient-client.sock")
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
            format!(r"\\.\pipe\sentineld\test-resilient-{}", dir_name)
        }
    }

    #[test]
    fn test_client_creation() {
        let (config, _temp_dir) = create_test_config();
        let _client = ResilientIpcClient::new(config);
        // Client creation always succeeds
    }

    #[test]
    fn test_circuit_breaker_creation() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(10));
        assert_eq!(breaker.failure_count, 0);
        assert!(!breaker.is_open());
        assert!(breaker.should_attempt_connection());
    }

    #[test]
    fn test_circuit_breaker_failure_threshold() {
        let mut breaker = CircuitBreaker::new(2, Duration::from_secs(1));

        // Record failures up to threshold
        breaker.record_failure();
        assert!(!breaker.is_open());

        breaker.record_failure();
        assert!(breaker.is_open());
        assert!(!breaker.should_attempt_connection());
    }

    #[test]
    fn test_circuit_breaker_recovery() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(100));

        // Trigger circuit breaker
        breaker.record_failure();
        assert!(breaker.is_open());

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(150));
        assert!(breaker.should_attempt_connection());
    }

    #[test]
    fn test_backoff_calculation() {
        let mut client = ResilientIpcClient::new(IpcConfig::default());

        // Test exponential backoff
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(100));

        // Simulate multiple attempts
        client.reconnect_attempts = 1;
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(200));

        client.reconnect_attempts = 2;
        assert_eq!(client.calculate_backoff_delay(), Duration::from_millis(400));
    }
}
