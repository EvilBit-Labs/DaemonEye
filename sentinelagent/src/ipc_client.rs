//! IPC client integration for sentinelagent
//!
//! This module provides the integration between sentinelagent and procmond
//! using the resilient IPC client for reliable communication.

use sentinel_lib::ipc::{ConnectionState, IpcConfig, ResilientIpcClient};
use sentinel_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::time::Duration;
use tracing::{debug, info};

/// IPC client manager for sentinelagent
pub struct IpcClientManager {
    client: ResilientIpcClient,
}

impl IpcClientManager {
    /// Create a new IPC client manager
    pub fn new(config: IpcConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let client = ResilientIpcClient::new(config);
        Ok(Self { client })
    }

    /// Send a detection task to procmond and receive the result
    pub async fn send_detection_task(
        &mut self,
        task: DetectionTask,
    ) -> Result<DetectionResult, Box<dyn std::error::Error>> {
        debug!(task_id = %task.task_id, "Sending detection task to procmond");

        let result = self.client.send_task(task).await?;

        debug!(
            task_id = %result.task_id,
            success = result.success,
            "Received detection result from procmond"
        );

        Ok(result)
    }

    /// Request process enumeration from procmond
    pub async fn enumerate_processes(
        &mut self,
    ) -> Result<DetectionResult, Box<dyn std::error::Error>> {
        let task = DetectionTask {
            task_id: format!("enumerate-{}", chrono::Utc::now().timestamp_millis()),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some("sentinelagent process enumeration".to_string()),
        };

        self.send_detection_task(task).await
    }

    /// Check if the IPC client is healthy
    pub async fn is_healthy(&self) -> bool {
        self.client.health_check().await
    }

    /// Get the current connection state
    #[allow(dead_code)]
    pub async fn get_connection_state(&self) -> ConnectionState {
        self.client.get_connection_state().await
    }

    /// Force a reconnection attempt
    pub async fn force_reconnect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Forcing IPC client reconnection");
        self.client.force_reconnect().await?;
        Ok(())
    }

    /// Get client statistics for monitoring
    #[allow(dead_code)]
    pub async fn get_stats(&self) -> sentinel_lib::ipc::ClientStats {
        self.client.get_stats().await
    }

    /// Wait for procmond to become available
    pub async fn wait_for_procmond(
        &mut self,
        max_wait_time: Duration,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let start_time = std::time::Instant::now();
        let check_interval = Duration::from_millis(500);

        info!("Waiting for procmond to become available...");

        while start_time.elapsed() < max_wait_time {
            match self.enumerate_processes().await {
                Ok(_) => {
                    info!("Procmond is now available");
                    return Ok(());
                }
                Err(e) => {
                    debug!("Procmond not yet available: {}", e);
                    tokio::time::sleep(check_interval).await;
                }
            }
        }

        Err(format!(
            "Procmond did not become available within {:?}",
            max_wait_time
        )
        .into())
    }
}

/// Create a default IPC configuration for sentinelagent
pub fn create_default_ipc_config() -> IpcConfig {
    IpcConfig {
        transport: sentinel_lib::ipc::TransportType::Interprocess,
        endpoint_path: default_procmond_endpoint(),
        max_frame_bytes: 1024 * 1024, // 1MB
        accept_timeout_ms: 5000,      // 5 seconds
        read_timeout_ms: 30000,       // 30 seconds
        write_timeout_ms: 10000,      // 10 seconds
        max_connections: 4,
        crc32_variant: sentinel_lib::ipc::Crc32Variant::Ieee,
    }
}

/// Get the default procmond endpoint path
fn default_procmond_endpoint() -> String {
    #[cfg(unix)]
    {
        "/var/run/sentineld/procmond.sock".to_string()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\sentineld\procmond".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (IpcConfig, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: sentinel_lib::ipc::TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            crc32_variant: sentinel_lib::ipc::Crc32Variant::Ieee,
        };

        (config, temp_dir)
    }

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-ipc-manager.sock")
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
            format!(r"\\.\pipe\sentineld\test-ipc-manager-{}", dir_name)
        }
    }

    #[test]
    fn test_ipc_manager_creation() {
        let (config, _temp_dir) = create_test_config();
        let manager = IpcClientManager::new(config);
        assert!(manager.is_ok());
    }

    #[test]
    fn test_default_config() {
        let config = create_default_ipc_config();
        assert_eq!(config.max_frame_bytes, 1024 * 1024);
        assert_eq!(config.max_connections, 4);
    }

    #[tokio::test]
    async fn test_connection_state() {
        let (config, _temp_dir) = create_test_config();
        let manager = IpcClientManager::new(config).unwrap();

        let state = manager.get_connection_state().await;
        assert_eq!(state, ConnectionState::Disconnected);

        assert!(!manager.is_healthy().await);
    }

    #[tokio::test]
    async fn test_enumerate_processes() {
        let (config, _temp_dir) = create_test_config();
        let mut manager = IpcClientManager::new(config).unwrap();

        // This will fail because there's no server, but we can test the method
        let result = manager.enumerate_processes().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_detection_task() {
        let (config, _temp_dir) = create_test_config();
        let mut manager = IpcClientManager::new(config).unwrap();

        let task = DetectionTask {
            task_id: "test-task".to_string(),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some("test metadata".to_string()),
        };

        // This will fail because there's no server, but we can test the method
        let result = manager.send_detection_task(task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_force_reconnect() {
        let (config, _temp_dir) = create_test_config();
        let mut manager = IpcClientManager::new(config).unwrap();

        // This will fail because there's no server, but we can test the method
        let result = manager.force_reconnect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wait_for_procmond_timeout() {
        let (config, _temp_dir) = create_test_config();
        let mut manager = IpcClientManager::new(config).unwrap();

        // Test with a very short timeout to ensure it times out
        let result = manager.wait_for_procmond(Duration::from_millis(100)).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Procmond did not become available")
        );
    }

    #[tokio::test]
    async fn test_get_stats() {
        let (config, _temp_dir) = create_test_config();
        let manager = IpcClientManager::new(config).unwrap();

        let stats = manager.get_stats().await;
        // Stats should be available even when disconnected
        assert_eq!(stats.connection_state, ConnectionState::Disconnected);
    }

    #[test]
    fn test_default_procmond_endpoint_unix() {
        // Test the endpoint path creation logic
        let endpoint = default_procmond_endpoint();

        #[cfg(unix)]
        {
            assert!(endpoint.contains("procmond.sock"));
            assert!(endpoint.starts_with("/var/run/sentineld/"));
        }

        #[cfg(windows)]
        {
            assert!(endpoint.contains("procmond"));
            assert!(endpoint.starts_with(r"\\.\pipe\sentineld\"));
        }
    }

    #[test]
    fn test_create_test_endpoint() {
        let temp_dir = TempDir::new().unwrap();
        let endpoint = create_test_endpoint(&temp_dir);

        #[cfg(unix)]
        {
            assert!(endpoint.contains("test-ipc-manager.sock"));
            assert!(endpoint.contains(&temp_dir.path().to_string_lossy().to_string()));
        }

        #[cfg(windows)]
        {
            assert!(endpoint.contains("test-ipc-manager"));
            assert!(endpoint.starts_with(r"\\.\pipe\sentineld\"));
        }
    }

    #[test]
    fn test_create_test_config() {
        let (config, temp_dir) = create_test_config();

        assert_eq!(config.max_frame_bytes, 1024 * 1024);
        assert_eq!(config.max_connections, 4);
        assert_eq!(config.accept_timeout_ms, 1000);
        assert_eq!(config.read_timeout_ms, 5000);
        assert_eq!(config.write_timeout_ms, 5000);

        // Verify the endpoint path is created correctly
        let endpoint = create_test_endpoint(&temp_dir);
        assert_eq!(config.endpoint_path, endpoint);
    }

    #[test]
    fn test_create_default_ipc_config_values() {
        let config = create_default_ipc_config();

        assert_eq!(config.max_frame_bytes, 1024 * 1024);
        assert_eq!(config.max_connections, 4);
        assert_eq!(config.accept_timeout_ms, 5000);
        assert_eq!(config.read_timeout_ms, 30000);
        assert_eq!(config.write_timeout_ms, 10000);
        assert_eq!(
            config.transport,
            sentinel_lib::ipc::TransportType::Interprocess
        );
        assert_eq!(config.crc32_variant, sentinel_lib::ipc::Crc32Variant::Ieee);

        // Verify the endpoint path is set correctly
        let expected_endpoint = default_procmond_endpoint();
        assert_eq!(config.endpoint_path, expected_endpoint);
    }

    #[tokio::test]
    async fn test_health_check_disconnected() {
        let (config, _temp_dir) = create_test_config();
        let manager = IpcClientManager::new(config).unwrap();

        // When disconnected, health check should return false
        assert!(!manager.is_healthy().await);
    }
}
