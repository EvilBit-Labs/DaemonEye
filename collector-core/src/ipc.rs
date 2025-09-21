//! IPC integration for collector-core framework.
//!
//! This module integrates the existing IPC infrastructure from sentinel-lib
//! with the collector-core runtime, enabling communication between collector-core
//! components and sentinelagent.

use crate::{config::CollectorConfig, source::SourceCaps};
use anyhow::{Context, Result};
use daemoneye_lib::{
    ipc::{InterprocessServer, IpcConfig, PanicStrategy, TransportType},
    proto::{CollectionCapabilities, DetectionResult, DetectionTask, TaskType},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// IPC server integration for collector-core runtime.
///
/// This struct manages the IPC server that handles communication between
/// collector-core components and sentinelagent, preserving the existing
/// protobuf protocol and CRC32 framing while integrating with the
/// collector-core event handling system.
pub struct CollectorIpcServer {
    config: IpcConfig,
    server: Option<InterprocessServer>,
    capabilities: Arc<RwLock<SourceCaps>>,
    shutdown_signal: Arc<AtomicBool>,
}

impl CollectorIpcServer {
    /// Creates a new IPC server for collector-core integration.
    ///
    /// # Arguments
    ///
    /// * `collector_config` - Collector configuration for IPC settings
    /// * `capabilities` - Shared capabilities from registered event sources
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use collector_core::{CollectorConfig, ipc::CollectorIpcServer, SourceCaps};
    /// use std::sync::Arc;
    /// use tokio::sync::RwLock;
    ///
    /// let config = CollectorConfig::default();
    /// let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));
    /// let ipc_server = CollectorIpcServer::new(config, capabilities);
    /// ```
    pub fn new(collector_config: CollectorConfig, capabilities: Arc<RwLock<SourceCaps>>) -> Self {
        let ipc_config = IpcConfig {
            transport: TransportType::Interprocess,
            endpoint_path: default_endpoint_path(),
            max_frame_bytes: 1024 * 1024, // 1MB
            accept_timeout_ms: 5000,      // 5 seconds
            read_timeout_ms: 30000,       // 30 seconds
            write_timeout_ms: 10000,      // 10 seconds
            max_connections: collector_config.max_event_sources,
            panic_strategy: if cfg!(test) {
                PanicStrategy::Unwind
            } else {
                PanicStrategy::Abort
            }, // Use unwind for tests, abort for production
        };

        Self {
            config: ipc_config,
            server: None,
            capabilities,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts the IPC server with task processing integration.
    ///
    /// This method initializes the IPC server and sets up message handling
    /// that integrates with the collector-core event processing system.
    ///
    /// # Arguments
    ///
    /// * `task_handler` - Function to handle incoming detection tasks
    ///
    /// # Errors
    ///
    /// Returns an error if the IPC server cannot be started or configured.
    pub async fn start<F>(&mut self, task_handler: F) -> Result<()>
    where
        F: Fn(
                DetectionTask,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<DetectionResult>> + Send>,
            > + Send
            + Sync
            + 'static,
    {
        info!("Starting collector-core IPC server");

        // Create IPC server
        let mut server = InterprocessServer::new(self.config.clone());

        // Set up message handler that integrates with collector-core
        let capabilities = Arc::clone(&self.capabilities);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);

        let task_handler = Arc::new(task_handler);
        server.set_handler(move |task: DetectionTask| {
            let handler = Arc::clone(&task_handler);
            let caps = Arc::clone(&capabilities);
            let shutdown = Arc::clone(&shutdown_signal);

            async move {
                // Check if we're shutting down
                if shutdown.load(Ordering::Relaxed) {
                    return Ok(DetectionResult::failure(
                        &task.task_id,
                        "Server is shutting down",
                    ));
                }

                // Validate task against capabilities
                if let Err(validation_error) = validate_task_capabilities(&task, &caps).await {
                    warn!(
                        task_id = %task.task_id,
                        error = %validation_error,
                        "Task validation failed"
                    );
                    return Ok(DetectionResult::failure(&task.task_id, validation_error));
                }

                // Process the task
                debug!(task_id = %task.task_id, task_type = task.task_type, "Processing detection task");

                match handler(task).await {
                    Ok(result) => {
                        debug!(task_id = %result.task_id, success = result.success, "Task completed");
                        Ok(result)
                    }
                    Err(e) => {
                        error!(error = %e, "Task processing failed");
                        Ok(DetectionResult::failure("unknown", e.to_string()))
                    }
                }
            }
        });

        // Start the server
        server.start().await.context("Failed to start IPC server")?;

        self.server = Some(server);
        info!("Collector-core IPC server started successfully");

        Ok(())
    }

    /// Returns the current capabilities as a CollectionCapabilities message.
    ///
    /// This method converts the collector-core SourceCaps bitflags into
    /// a protobuf CollectionCapabilities message for capability negotiation
    /// with sentinelagent.
    pub async fn get_capabilities(&self) -> CollectionCapabilities {
        let caps = self.capabilities.read().await;
        source_caps_to_proto_capabilities(*caps)
    }

    /// Gracefully shuts down the IPC server.
    ///
    /// This method signals shutdown to all active connections and waits
    /// for them to complete before stopping the server.
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down collector-core IPC server");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Stop the server if it exists
        if let Some(mut server) = self.server.take() {
            server
                .graceful_shutdown()
                .await
                .context("Failed to shutdown IPC server gracefully")?;
        }

        info!("Collector-core IPC server shut down successfully");
        Ok(())
    }

    /// Updates the capabilities based on registered event sources.
    ///
    /// This method should be called when event sources are registered
    /// or unregistered to keep the capability information current.
    pub async fn update_capabilities(&self, new_capabilities: SourceCaps) {
        let mut caps = self.capabilities.write().await;
        *caps = new_capabilities;
        debug!(capabilities = ?new_capabilities, "Updated IPC server capabilities");
    }
}

/// Validates that a detection task is supported by current capabilities.
async fn validate_task_capabilities(
    task: &DetectionTask,
    capabilities: &Arc<RwLock<SourceCaps>>,
) -> Result<(), String> {
    let caps = capabilities.read().await;

    match TaskType::try_from(task.task_type) {
        Ok(TaskType::EnumerateProcesses)
        | Ok(TaskType::CheckProcessHash)
        | Ok(TaskType::MonitorProcessTree)
        | Ok(TaskType::VerifyExecutable) => {
            if !caps.contains(SourceCaps::PROCESS) {
                return Err("Process monitoring not supported".to_string());
            }
        }
        Ok(TaskType::MonitorNetworkConnections) => {
            if !caps.contains(SourceCaps::NETWORK) {
                return Err("Network monitoring not supported".to_string());
            }
        }
        Ok(TaskType::TrackFileOperations) => {
            if !caps.contains(SourceCaps::FILESYSTEM) {
                return Err("Filesystem monitoring not supported".to_string());
            }
        }
        Ok(TaskType::CollectPerformanceMetrics) => {
            if !caps.contains(SourceCaps::PERFORMANCE) {
                return Err("Performance monitoring not supported".to_string());
            }
        }
        Ok(_) => {
            return Err(format!("Unsupported task type: {}", task.task_type));
        }
        Err(_) => {
            return Err(format!("Unknown task type: {}", task.task_type));
        }
    }

    Ok(())
}

/// Converts SourceCaps bitflags to protobuf CollectionCapabilities.
fn source_caps_to_proto_capabilities(caps: SourceCaps) -> CollectionCapabilities {
    use daemoneye_lib::proto::{AdvancedCapabilities, MonitoringDomain};

    let mut supported_domains = Vec::new();

    if caps.contains(SourceCaps::PROCESS) {
        supported_domains.push(MonitoringDomain::Process as i32);
    }
    if caps.contains(SourceCaps::NETWORK) {
        supported_domains.push(MonitoringDomain::Network as i32);
    }
    if caps.contains(SourceCaps::FILESYSTEM) {
        supported_domains.push(MonitoringDomain::Filesystem as i32);
    }
    if caps.contains(SourceCaps::PERFORMANCE) {
        supported_domains.push(MonitoringDomain::Performance as i32);
    }

    CollectionCapabilities {
        supported_domains,
        advanced: Some(AdvancedCapabilities {
            kernel_level: caps.contains(SourceCaps::KERNEL_LEVEL),
            realtime: caps.contains(SourceCaps::REALTIME),
            system_wide: caps.contains(SourceCaps::SYSTEM_WIDE),
        }),
    }
}

/// Get the default endpoint path based on the platform.
fn default_endpoint_path() -> String {
    #[cfg(unix)]
    {
        if cfg!(test) {
            // Use a temporary path for tests
            format!("/tmp/sentineld-test-{}.sock", std::process::id())
        } else {
            "/var/run/sentineld/collector-core.sock".to_owned()
        }
    }
    #[cfg(windows)]
    {
        if cfg!(test) {
            // Use a temporary pipe name for tests
            format!(r"\\.\pipe\sentineld-test-{}", std::process::id())
        } else {
            r"\\.\pipe\sentineld\collector-core".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceCaps;

    #[test]
    fn test_source_caps_to_proto_capabilities() {
        use daemoneye_lib::proto::MonitoringDomain;

        let caps = SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE;
        let proto_caps = source_caps_to_proto_capabilities(caps);

        assert!(
            proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Process as i32))
        );
        assert!(
            !proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Network as i32))
        );
        assert!(
            !proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Filesystem as i32))
        );
        assert!(
            !proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Performance as i32))
        );

        let advanced = proto_caps.advanced.as_ref().unwrap();
        assert!(!advanced.kernel_level);
        assert!(advanced.realtime);
        assert!(advanced.system_wide);
    }

    #[test]
    fn test_default_endpoint_path() {
        let path = default_endpoint_path();
        #[cfg(unix)]
        {
            if cfg!(test) {
                assert!(path.contains("sentineld-test-"));
                assert!(path.ends_with(".sock"));
            } else {
                assert!(path.contains("collector-core.sock"));
            }
        }
        #[cfg(windows)]
        {
            if cfg!(test) {
                assert!(path.contains(r"\\.\pipe\sentineld-test-"));
            } else {
                assert!(path.contains(r"\\.\pipe\"));
            }
        }
    }

    #[tokio::test]
    async fn test_collector_ipc_server_creation() {
        let config = CollectorConfig::default();
        let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));
        let _server = CollectorIpcServer::new(config, capabilities);
        // Server creation should succeed
    }

    #[tokio::test]
    async fn test_capability_updates() {
        let config = CollectorConfig::default();
        let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));
        let server = CollectorIpcServer::new(config, Arc::clone(&capabilities));

        // Test initial capabilities
        use daemoneye_lib::proto::MonitoringDomain;

        let proto_caps = server.get_capabilities().await;
        assert!(
            proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Process as i32))
        );
        assert!(
            !proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Network as i32))
        );

        // Update capabilities
        server
            .update_capabilities(SourceCaps::PROCESS | SourceCaps::NETWORK)
            .await;

        // Test updated capabilities
        let proto_caps = server.get_capabilities().await;
        assert!(
            proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Process as i32))
        );
        assert!(
            proto_caps
                .supported_domains
                .contains(&(MonitoringDomain::Network as i32))
        );
    }

    #[tokio::test]
    async fn test_task_validation() {
        let capabilities = Arc::new(RwLock::new(SourceCaps::PROCESS));

        // Test valid process task
        let process_task = DetectionTask {
            task_id: "test-1".to_string(),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        let result = validate_task_capabilities(&process_task, &capabilities).await;
        assert!(result.is_ok());

        // Test invalid network task (not supported)
        let network_task = DetectionTask {
            task_id: "test-2".to_string(),
            task_type: TaskType::MonitorNetworkConnections as i32,
            process_filter: None,
            hash_check: None,
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        let result = validate_task_capabilities(&network_task, &capabilities).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Network monitoring not supported")
        );
    }
}
