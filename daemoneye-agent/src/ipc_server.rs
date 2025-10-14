//! IPC server management for daemoneye-agent CLI communication
//!
//! This module provides the `IpcServerManager` which manages an IPC server
//! for communication with daemoneye-cli using protobuf + CRC32 framing.
//! This operates alongside the embedded EventBus broker for collector-core
//! component communication, implementing the dual-protocol architecture.

use anyhow::{Context, Result};
use daemoneye_lib::ipc::{InterprocessServer, IpcConfig};
use daemoneye_lib::proto::{DetectionResult, DetectionTask};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Health status of the IPC server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcServerHealth {
    /// Server is healthy and operational
    Healthy,
    /// Server is starting up
    Starting,
    /// Server is shutting down
    ShuttingDown,
    /// Server has encountered an error
    Unhealthy(String),
    /// Server is stopped
    Stopped,
}

/// IPC server manager that coordinates the InterprocessServer lifecycle
/// within the daemoneye-agent process architecture for CLI communication.
pub struct IpcServerManager {
    /// Configuration for the IPC server
    config: IpcConfig,
    /// The IPC server instance
    server: Arc<RwLock<Option<InterprocessServer>>>,
    /// Current health status
    health_status: Arc<RwLock<IpcServerHealth>>,
    /// Shutdown signal sender
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl IpcServerManager {
    /// Create a new IPC server manager with the given configuration
    pub fn new(config: IpcConfig) -> Self {
        Self {
            config,
            server: Arc::new(RwLock::new(None)),
            health_status: Arc::new(RwLock::new(IpcServerHealth::Stopped)),
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Initialize and start the IPC server
    pub async fn start(&self) -> Result<()> {
        // Update health status to starting
        {
            let mut health = self.health_status.write().await;
            *health = IpcServerHealth::Starting;
        }

        info!(
            endpoint_path = %self.config.endpoint_path,
            max_connections = self.config.max_connections,
            "Starting IPC server for CLI communication"
        );

        // Create the server instance
        let mut server = InterprocessServer::new(self.config.clone());

        // Set up message handler for CLI requests
        server.set_handler(|task: DetectionTask| async move {
            // Handle CLI requests - for now, return a simple response
            // This will be expanded to handle actual CLI operations
            debug!("Received CLI task: {}", task.task_id);

            // Create a basic response for CLI requests
            let result = DetectionResult {
                task_id: task.task_id.clone(),
                success: true,
                error_message: None,
                processes: vec![], // CLI requests typically don't return process data
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            };

            Ok(result)
        });

        // Start the server
        server.start().await.context("Failed to start IPC server")?;

        // Store the server instance
        {
            let mut server_guard = self.server.write().await;
            *server_guard = Some(server);
        }

        // Update health status to healthy
        {
            let mut health = self.health_status.write().await;
            *health = IpcServerHealth::Healthy;
        }

        info!("IPC server started successfully for CLI communication");
        Ok(())
    }

    /// Gracefully shutdown the IPC server
    pub async fn shutdown(&self) -> Result<()> {
        info!("Initiating graceful shutdown of IPC server");

        // Update health status to shutting down
        {
            let mut health = self.health_status.write().await;
            *health = IpcServerHealth::ShuttingDown;
        }

        // Send shutdown signal if available
        {
            let mut shutdown_tx_guard = self.shutdown_tx.lock().await;
            if let Some(tx) = shutdown_tx_guard.take() {
                if tx.send(()).is_err() {
                    warn!("Failed to send shutdown signal - receiver may have been dropped");
                }
            }
        }

        // Shutdown the server with timeout
        let shutdown_timeout = Duration::from_secs(30); // 30 second timeout for CLI server
        let shutdown_result = tokio::time::timeout(shutdown_timeout, async {
            let mut server_guard = self.server.write().await;
            if let Some(mut server) = server_guard.take() {
                server.graceful_shutdown().await
            } else {
                Ok(())
            }
        })
        .await;

        match shutdown_result {
            Ok(Ok(())) => {
                info!("IPC server shutdown completed successfully");
            }
            Ok(Err(e)) => {
                error!(error = %e, "Error during IPC server shutdown");
                return Err(e.into());
            }
            Err(_) => {
                warn!(
                    timeout_seconds = 30,
                    "IPC server shutdown timed out, forcing termination"
                );
                // Force cleanup
                let mut server_guard = self.server.write().await;
                if let Some(mut server) = server_guard.take() {
                    server.stop();
                }
            }
        }

        // Update health status to stopped
        {
            let mut health = self.health_status.write().await;
            *health = IpcServerHealth::Stopped;
        }

        info!("IPC server shutdown complete");
        Ok(())
    }

    /// Get the current health status of the IPC server
    pub async fn health_status(&self) -> IpcServerHealth {
        let health = self.health_status.read().await;
        health.clone()
    }

    /// Check if the IPC server is currently running
    #[allow(dead_code)]
    pub async fn is_running(&self) -> bool {
        let health = self.health_status().await;
        matches!(health, IpcServerHealth::Healthy | IpcServerHealth::Starting)
    }

    /// Get the endpoint path for the IPC server
    pub fn endpoint_path(&self) -> &str {
        &self.config.endpoint_path
    }

    /// Perform a health check on the IPC server
    pub async fn health_check(&self) -> IpcServerHealth {
        let current_health = self.health_status().await;

        match current_health {
            IpcServerHealth::Healthy => {
                // Verify server is actually responsive by checking if it exists
                let server_guard = self.server.read().await;
                if server_guard.is_some() {
                    debug!("IPC server health check passed");
                    IpcServerHealth::Healthy
                } else {
                    warn!("IPC server health check failed - server instance not found");
                    let mut health = self.health_status.write().await;
                    *health = IpcServerHealth::Unhealthy("Server instance not found".to_string());
                    IpcServerHealth::Unhealthy("Server instance not found".to_string())
                }
            }
            other => other,
        }
    }

    /// Wait for the IPC server to become healthy with a timeout
    pub async fn wait_for_healthy(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let health = self.health_status().await;
            match health {
                IpcServerHealth::Healthy => {
                    debug!("IPC server is healthy");
                    return Ok(());
                }
                IpcServerHealth::Starting => {
                    debug!("IPC server is still starting, waiting...");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                IpcServerHealth::Unhealthy(ref error) => {
                    return Err(anyhow::anyhow!("IPC server is unhealthy: {}", error));
                }
                IpcServerHealth::ShuttingDown | IpcServerHealth::Stopped => {
                    return Err(anyhow::anyhow!("IPC server is not running"));
                }
            }
        }

        Err(anyhow::anyhow!(
            "Timeout waiting for IPC server to become healthy after {:?}",
            timeout
        ))
    }
}

/// Create a default IPC configuration for daemoneye-agent CLI server
///
/// This configuration is optimized for CLI communication with:
/// - Smaller message sizes suitable for CLI operations
/// - Reasonable timeouts for interactive use
/// - Conservative connection limits for CLI usage
pub fn create_cli_ipc_config() -> IpcConfig {
    IpcConfig {
        transport: daemoneye_lib::ipc::TransportType::Interprocess,
        endpoint_path: default_cli_endpoint_path(),
        max_frame_bytes: 64 * 1024, // 64KB - suitable for CLI operations
        accept_timeout_ms: 5000,    // 5 seconds - reasonable for CLI
        read_timeout_ms: 30000,     // 30 seconds - allow time for operations
        write_timeout_ms: 10000,    // 10 seconds - reasonable for responses
        max_connections: 8,         // Limited connections for CLI usage
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Abort, // Production configuration
    }
}

/// Get the default CLI endpoint path based on the platform
fn default_cli_endpoint_path() -> String {
    #[cfg(unix)]
    {
        "/var/run/daemoneye/agent-cli.sock".to_owned()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\daemoneye\agent-cli".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ipc_server_manager_creation() {
        let config = create_cli_ipc_config();
        let manager = IpcServerManager::new(config);

        let health = manager.health_status().await;
        assert_eq!(health, IpcServerHealth::Stopped);
        assert!(!manager.is_running().await);
    }

    #[tokio::test]
    async fn test_ipc_server_manager_endpoint_path() {
        let config = create_cli_ipc_config();
        let manager = IpcServerManager::new(config);

        #[cfg(unix)]
        assert_eq!(manager.endpoint_path(), "/var/run/daemoneye/agent-cli.sock");
        #[cfg(windows)]
        assert_eq!(manager.endpoint_path(), r"\\.\pipe\daemoneye\agent-cli");
    }

    #[tokio::test]
    async fn test_ipc_server_manager_health_check_when_stopped() {
        let config = create_cli_ipc_config();
        let manager = IpcServerManager::new(config);

        let health = manager.health_check().await;
        assert_eq!(health, IpcServerHealth::Stopped);
    }

    #[tokio::test]
    async fn test_ipc_server_manager_wait_for_healthy_timeout() {
        let config = create_cli_ipc_config();
        let manager = IpcServerManager::new(config);

        let result = manager.wait_for_healthy(Duration::from_millis(100)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_ipc_config_creation() {
        let config = create_cli_ipc_config();
        assert_eq!(config.max_frame_bytes, 64 * 1024); // 64KB for CLI operations
        assert_eq!(config.accept_timeout_ms, 5000); // 5 seconds for CLI
        assert_eq!(config.read_timeout_ms, 30000);
        assert_eq!(config.write_timeout_ms, 10000);
        assert_eq!(config.max_connections, 8); // Limited for CLI usage
    }

    #[test]
    fn test_default_cli_endpoint_path() {
        let path = default_cli_endpoint_path();
        #[cfg(unix)]
        assert!(path.contains("agent-cli.sock"));
        #[cfg(windows)]
        assert!(path.contains(r"\\.\pipe\"));
    }
}
