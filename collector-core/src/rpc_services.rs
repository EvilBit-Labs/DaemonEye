//! RPC service implementations for collector lifecycle management.
//!
//! This module provides concrete implementations of the RPC service traits
//! defined in busrt_types.rs, enabling collector lifecycle management through
//! busrt message broker RPC calls.

use crate::{
    busrt_types::{
        BusrtError, CapabilitiesRequest, CapabilitiesResponse, CollectorInfo,
        CollectorLifecycleService, CollectorStatus, ConfigurationService, GetConfigRequest,
        GetConfigResponse, HealthCheckRequest, HealthCheckResponse, HealthCheckService,
        HeartbeatRequest, HeartbeatResponse, ListCollectorsRequest, ListCollectorsResponse,
        MetricsRequest, MetricsResponse, PauseCollectorRequest, PauseCollectorResponse,
        ResourceLimits, ResourceUsage, RestartCollectorRequest, RestartCollectorResponse,
        ResumeCollectorRequest, ResumeCollectorResponse, StartCollectorRequest,
        StartCollectorResponse, StatusRequest, StatusResponse, StopCollectorRequest,
        StopCollectorResponse, UpdateConfigRequest, UpdateConfigResponse, ValidateConfigRequest,
        ValidateConfigResponse,
    },
    config_manager::{ConfigManager, ConfigManagerSettings},
    health_monitor::{HealthMonitor, HealthMonitorConfig},
    shutdown_coordinator::{ShutdownConfig, ShutdownCoordinator, ShutdownRequest},
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

/// Collector lifecycle manager that implements RPC services
pub struct CollectorLifecycleManager {
    /// Registry of managed collectors
    collectors: Arc<RwLock<HashMap<String, ManagedCollector>>>,
    /// Configuration management system
    config_manager: Arc<Mutex<ConfigManager>>,
    /// Health monitoring system
    health_monitor: Arc<Mutex<HealthMonitor>>,
    /// Shutdown coordination system
    shutdown_coordinator: Arc<Mutex<ShutdownCoordinator>>,
}

/// Internal representation of a managed collector
#[derive(Debug, Clone)]
struct ManagedCollector {
    collector_id: String,
    collector_type: String,
    status: CollectorStatus,
    start_time: SystemTime,
    last_activity: SystemTime,
    capabilities: Vec<String>,
    resource_usage: ResourceUsage,
    error_message: Option<String>,
    pause_reason: Option<String>,
}

// Remove the local CollectorHealthState as we now use the one from health_monitor

impl CollectorLifecycleManager {
    /// Create a new collector lifecycle manager
    pub fn new() -> Self {
        let health_config = HealthMonitorConfig::default();
        let health_monitor = HealthMonitor::new(health_config);

        let config_settings = ConfigManagerSettings::default();
        let config_manager = ConfigManager::new(config_settings);

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        Self {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        }
    }

    /// Create a new collector lifecycle manager with custom health config
    pub fn new_with_health_config(health_config: HealthMonitorConfig) -> Self {
        let health_monitor = HealthMonitor::new(health_config);

        let config_settings = ConfigManagerSettings::default();
        let config_manager = ConfigManager::new(config_settings);

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        Self {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        }
    }

    /// Create a new collector lifecycle manager with custom config settings (for testing)
    pub fn new_with_config_settings(
        health_config: HealthMonitorConfig,
        config_settings: ConfigManagerSettings,
    ) -> Self {
        let health_monitor = HealthMonitor::new(health_config);
        let config_manager = ConfigManager::new(config_settings);

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        Self {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        }
    }

    /// Start the lifecycle manager (starts health monitoring and config manager)
    pub async fn start(&self) -> Result<(), BusrtError> {
        // Start config manager first to register validation rules
        let mut config_manager = self.config_manager.lock().await;
        config_manager.start().await.map_err(|e| {
            BusrtError::BrokerUnavailable(format!("Failed to start config manager: {}", e))
        })?;
        drop(config_manager);

        // Start health monitor
        let mut health_monitor = self.health_monitor.lock().await;
        health_monitor.start().await.map_err(|e| {
            BusrtError::BrokerUnavailable(format!("Failed to start health monitor: {}", e))
        })?;
        Ok(())
    }

    /// Stop the lifecycle manager (stops health monitoring)
    pub async fn stop(&self) -> Result<(), BusrtError> {
        let mut health_monitor = self.health_monitor.lock().await;
        health_monitor.stop().await.map_err(|e| {
            BusrtError::BrokerUnavailable(format!("Failed to stop health monitor: {}", e))
        })?;
        Ok(())
    }

    /// Shutdown all collectors (for testing)
    pub async fn shutdown_all_collectors(
        &self,
        reason: String,
    ) -> Result<Vec<crate::shutdown_coordinator::ShutdownResponse>, BusrtError> {
        let shutdown_coordinator = self.shutdown_coordinator.lock().await;
        shutdown_coordinator
            .shutdown_all_collectors(reason)
            .await
            .map_err(|e| {
                BusrtError::BrokerUnavailable(format!("Shutdown coordination failed: {}", e))
            })
    }

    /// Register a new collector in the manager
    pub async fn register_collector(
        &self,
        collector_id: String,
        collector_type: String,
        capabilities: Vec<String>,
    ) -> Result<(), BusrtError> {
        let collector = ManagedCollector {
            collector_id: collector_id.clone(),
            collector_type,
            status: CollectorStatus::Starting,
            start_time: SystemTime::now(),
            last_activity: SystemTime::now(),
            capabilities,
            resource_usage: ResourceUsage {
                cpu_percent: 0.0,
                memory_bytes: 0,
                disk_io_bytes: 0,
                network_io_bytes: 0,
                open_files: 0,
            },
            error_message: None,
            pause_reason: None,
        };

        {
            let mut collectors = self.collectors.write().unwrap();
            collectors.insert(collector_id.clone(), collector);
        }

        // Register with health monitor
        {
            let health_monitor = self.health_monitor.lock().await;
            health_monitor
                .register_collector(collector_id.clone())
                .await
                .map_err(|e| {
                    BusrtError::BrokerUnavailable(format!(
                        "Failed to register collector with health monitor: {}",
                        e
                    ))
                })?;
        }

        info!(collector_id = %collector_id, "Collector registered");
        Ok(())
    }

    /// Update collector status
    pub async fn update_collector_status(
        &self,
        collector_id: &str,
        status: CollectorStatus,
        error_message: Option<String>,
    ) -> Result<(), BusrtError> {
        let mut collectors = self.collectors.write().unwrap();
        if let Some(collector) = collectors.get_mut(collector_id) {
            collector.status = status;
            collector.last_activity = SystemTime::now();
            collector.error_message = error_message;
            debug!(collector_id = %collector_id, status = ?collector.status, "Collector status updated");
            Ok(())
        } else {
            Err(BusrtError::TopicNotFound(format!(
                "Collector not found: {}",
                collector_id
            )))
        }
    }

    /// Update collector resource usage
    pub async fn update_resource_usage(
        &self,
        collector_id: &str,
        resource_usage: ResourceUsage,
    ) -> Result<(), BusrtError> {
        let mut collectors = self.collectors.write().unwrap();
        if let Some(collector) = collectors.get_mut(collector_id) {
            collector.resource_usage = resource_usage;
            collector.last_activity = SystemTime::now();
            Ok(())
        } else {
            Err(BusrtError::TopicNotFound(format!(
                "Collector not found: {}",
                collector_id
            )))
        }
    }

    /// Get collector by ID
    fn get_collector(&self, collector_id: &str) -> Result<ManagedCollector, BusrtError> {
        let collectors = self.collectors.read().unwrap();
        collectors.get(collector_id).cloned().ok_or_else(|| {
            BusrtError::TopicNotFound(format!("Collector not found: {}", collector_id))
        })
    }

    /// Calculate uptime for a collector
    fn calculate_uptime(&self, start_time: SystemTime) -> u64 {
        SystemTime::now()
            .duration_since(start_time)
            .unwrap_or_default()
            .as_secs()
    }

    /// Convert SystemTime to timestamp
    fn system_time_to_timestamp(&self, time: SystemTime) -> i64 {
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }
}

impl Default for CollectorLifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CollectorLifecycleService for CollectorLifecycleManager {
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn start_collector(
        &self,
        request: StartCollectorRequest,
    ) -> Result<StartCollectorResponse, BusrtError> {
        info!(
            collector_id = %request.collector_id,
            collector_type = %request.collector_type,
            "Starting collector"
        );

        // Register the collector if not already registered
        self.register_collector(
            request.collector_id.clone(),
            request.collector_type.clone(),
            request.config.capabilities.clone(),
        )
        .await?;

        // Store configuration using config manager
        let config_result = {
            let config_manager = self.config_manager.lock().await;
            let update_request = UpdateConfigRequest {
                collector_id: request.collector_id.clone(),
                new_config: request.config.clone(),
                validate_only: false,
            };
            config_manager.update_config(update_request).await
        };

        // Only update status to running after successful config persistence
        match config_result {
            Ok(_) => {
                self.update_collector_status(&request.collector_id, CollectorStatus::Running, None)
                    .await?;

                let collector = self.get_collector(&request.collector_id)?;

                Ok(StartCollectorResponse {
                    success: true,
                    collector_id: request.collector_id,
                    error_message: None,
                    status: collector.status,
                })
            }
            Err(e) => {
                // Config persistence failed, set error status
                let error_msg = format!("Failed to persist configuration: {}", e);
                self.update_collector_status(
                    &request.collector_id,
                    CollectorStatus::Error,
                    Some(error_msg.clone()),
                )
                .await?;

                Ok(StartCollectorResponse {
                    success: false,
                    collector_id: request.collector_id,
                    error_message: Some(error_msg),
                    status: CollectorStatus::Error,
                })
            }
        }
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn stop_collector(
        &self,
        request: StopCollectorRequest,
    ) -> Result<StopCollectorResponse, BusrtError> {
        info!(
            collector_id = %request.collector_id,
            force = request.force,
            "Stopping collector"
        );

        // Update status to stopping
        self.update_collector_status(&request.collector_id, CollectorStatus::Stopping, None)
            .await?;

        // Use shutdown coordinator for graceful shutdown
        let shutdown_request = ShutdownRequest {
            collector_id: request.collector_id.clone(),
            reason: "Stop collector request".to_string(),
            force: request.force,
            timeout: Some(Duration::from_secs(request.timeout_seconds as u64)),
        };

        let shutdown_response = {
            let shutdown_coordinator = self.shutdown_coordinator.lock().await;
            shutdown_coordinator
                .shutdown_collector(shutdown_request)
                .await
                .map_err(|e| {
                    BusrtError::BrokerUnavailable(format!("Shutdown coordination failed: {}", e))
                })?
        };

        // Update final status based on shutdown result
        let final_status = if shutdown_response.success {
            CollectorStatus::Stopped
        } else {
            CollectorStatus::Error
        };

        self.update_collector_status(
            &request.collector_id,
            final_status,
            shutdown_response.error_message.clone(),
        )
        .await?;

        Ok(StopCollectorResponse {
            success: shutdown_response.success,
            error_message: shutdown_response.error_message,
            shutdown_duration_ms: shutdown_response.duration.as_millis() as u32,
        })
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn restart_collector(
        &self,
        request: RestartCollectorRequest,
    ) -> Result<RestartCollectorResponse, BusrtError> {
        info!(collector_id = %request.collector_id, "Restarting collector");

        // Update status to restarting
        self.update_collector_status(&request.collector_id, CollectorStatus::Restarting, None)
            .await?;

        // Stop the collector first
        let stop_request = StopCollectorRequest {
            collector_id: request.collector_id.clone(),
            force: false,
            timeout_seconds: request.timeout_seconds,
        };
        self.stop_collector(stop_request).await?;

        // Update configuration if provided
        if let Some(new_config) = &request.new_config {
            let config_manager = self.config_manager.lock().await;
            let update_request = UpdateConfigRequest {
                collector_id: request.collector_id.clone(),
                new_config: new_config.clone(),
                validate_only: false,
            };
            config_manager.update_config(update_request).await?;
        }

        // Start the collector again
        let collector = self.get_collector(&request.collector_id)?;
        let config = if let Some(new_config) = request.new_config {
            new_config
        } else {
            // Get existing config
            let config_manager = self.config_manager.lock().await;
            let get_request = GetConfigRequest {
                collector_id: request.collector_id.clone(),
            };
            let get_response = config_manager.get_config(get_request).await?;
            get_response.config
        };

        let start_request = StartCollectorRequest {
            collector_type: collector.collector_type,
            collector_id: request.collector_id.clone(),
            config,
            environment: HashMap::new(),
        };
        self.start_collector(start_request).await?;

        Ok(RestartCollectorResponse {
            success: true,
            error_message: None,
            new_collector_id: request.collector_id,
        })
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn get_collector_status(
        &self,
        request: StatusRequest,
    ) -> Result<StatusResponse, BusrtError> {
        let collector = self.get_collector(&request.collector_id)?;
        let uptime = self.calculate_uptime(collector.start_time);
        let last_activity = self.system_time_to_timestamp(collector.last_activity);

        Ok(StatusResponse {
            collector_id: request.collector_id,
            status: collector.status,
            uptime_seconds: uptime,
            last_activity,
            error_message: collector.error_message,
            resource_usage: collector.resource_usage,
        })
    }

    #[instrument(skip(self))]
    async fn list_collectors(
        &self,
        request: ListCollectorsRequest,
    ) -> Result<ListCollectorsResponse, BusrtError> {
        let collectors = self.collectors.read().unwrap();
        let mut collector_infos = Vec::new();

        for collector in collectors.values() {
            // Apply filters
            if let Some(filter_status) = &request.filter_status {
                if collector.status != *filter_status {
                    continue;
                }
            }

            if let Some(filter_type) = &request.filter_type {
                if collector.collector_type != *filter_type {
                    continue;
                }
            }

            let uptime = self.calculate_uptime(collector.start_time);
            let last_activity = self.system_time_to_timestamp(collector.last_activity);

            collector_infos.push(CollectorInfo {
                collector_id: collector.collector_id.clone(),
                collector_type: collector.collector_type.clone(),
                status: collector.status.clone(),
                uptime_seconds: uptime,
                last_activity,
                capabilities: collector.capabilities.clone(),
                resource_usage: collector.resource_usage.clone(),
            });
        }

        Ok(ListCollectorsResponse {
            total_count: collector_infos.len(),
            collectors: collector_infos,
        })
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn get_collector_capabilities(
        &self,
        request: CapabilitiesRequest,
    ) -> Result<CapabilitiesResponse, BusrtError> {
        let collector = self.get_collector(&request.collector_id)?;

        let supported_operations = vec![
            "start".to_string(),
            "stop".to_string(),
            "restart".to_string(),
            "pause".to_string(),
            "resume".to_string(),
            "health_check".to_string(),
            "update_config".to_string(),
        ];

        let resource_limits = ResourceLimits {
            max_cpu_percent: Some(80.0),
            max_memory_bytes: Some(1024 * 1024 * 1024), // 1GB
            max_disk_io_bytes_per_sec: Some(100 * 1024 * 1024), // 100MB/s
            max_network_io_bytes_per_sec: Some(50 * 1024 * 1024), // 50MB/s
            max_open_files: Some(1000),
        };

        let mut metadata = HashMap::new();
        metadata.insert("version".to_string(), "1.0.0".to_string());
        metadata.insert("platform".to_string(), std::env::consts::OS.to_string());

        Ok(CapabilitiesResponse {
            collector_id: request.collector_id,
            capabilities: collector.capabilities,
            supported_operations,
            resource_limits,
            metadata,
        })
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn pause_collector(
        &self,
        request: PauseCollectorRequest,
    ) -> Result<PauseCollectorResponse, BusrtError> {
        info!(
            collector_id = %request.collector_id,
            reason = %request.reason,
            "Pausing collector"
        );

        let mut collectors = self.collectors.write().unwrap();
        if let Some(collector) = collectors.get_mut(&request.collector_id) {
            if collector.status == CollectorStatus::Running {
                collector.status = CollectorStatus::Paused;
                collector.pause_reason = Some(request.reason);
                collector.last_activity = SystemTime::now();

                let paused_at = self.system_time_to_timestamp(SystemTime::now());

                Ok(PauseCollectorResponse {
                    success: true,
                    error_message: None,
                    paused_at,
                })
            } else {
                Ok(PauseCollectorResponse {
                    success: false,
                    error_message: Some(format!(
                        "Cannot pause collector in status: {:?}",
                        collector.status
                    )),
                    paused_at: 0,
                })
            }
        } else {
            Err(BusrtError::TopicNotFound(format!(
                "Collector not found: {}",
                request.collector_id
            )))
        }
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn resume_collector(
        &self,
        request: ResumeCollectorRequest,
    ) -> Result<ResumeCollectorResponse, BusrtError> {
        info!(collector_id = %request.collector_id, "Resuming collector");

        let mut collectors = self.collectors.write().unwrap();
        if let Some(collector) = collectors.get_mut(&request.collector_id) {
            if collector.status == CollectorStatus::Paused {
                collector.status = CollectorStatus::Running;
                collector.pause_reason = None;
                collector.last_activity = SystemTime::now();

                let resumed_at = self.system_time_to_timestamp(SystemTime::now());

                Ok(ResumeCollectorResponse {
                    success: true,
                    error_message: None,
                    resumed_at,
                })
            } else {
                Ok(ResumeCollectorResponse {
                    success: false,
                    error_message: Some(format!(
                        "Cannot resume collector in status: {:?}",
                        collector.status
                    )),
                    resumed_at: 0,
                })
            }
        } else {
            Err(BusrtError::TopicNotFound(format!(
                "Collector not found: {}",
                request.collector_id
            )))
        }
    }
}

#[async_trait]
impl HealthCheckService for CollectorLifecycleManager {
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn health_check(
        &self,
        request: HealthCheckRequest,
    ) -> Result<HealthCheckResponse, BusrtError> {
        let health_monitor = self.health_monitor.lock().await;
        health_monitor
            .perform_health_check(request)
            .await
            .map_err(|e| BusrtError::BrokerUnavailable(format!("Health check failed: {}", e)))
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn heartbeat(&self, request: HeartbeatRequest) -> Result<HeartbeatResponse, BusrtError> {
        let health_monitor = self.health_monitor.lock().await;
        health_monitor
            .process_heartbeat(request)
            .await
            .map_err(|e| {
                BusrtError::BrokerUnavailable(format!("Heartbeat processing failed: {}", e))
            })
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn get_metrics(&self, request: MetricsRequest) -> Result<MetricsResponse, BusrtError> {
        let health_monitor = self.health_monitor.lock().await;
        health_monitor
            .get_metrics(request)
            .await
            .map_err(|e| BusrtError::BrokerUnavailable(format!("Metrics retrieval failed: {}", e)))
    }
}

#[async_trait]
impl ConfigurationService for CollectorLifecycleManager {
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn update_config(
        &self,
        request: UpdateConfigRequest,
    ) -> Result<UpdateConfigResponse, BusrtError> {
        let collector_id = request.collector_id.clone();
        let new_capabilities = request.new_config.capabilities.clone();

        let config_manager = self.config_manager.lock().await;
        let response = config_manager.update_config(request).await?;

        // Update collector capabilities if configuration was successfully updated
        if response.success {
            let mut collectors = self.collectors.write().unwrap();
            if let Some(collector) = collectors.get_mut(&collector_id) {
                collector.capabilities = new_capabilities;
                collector.last_activity = SystemTime::now();
            }
        }

        Ok(response)
    }

    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    async fn get_config(&self, request: GetConfigRequest) -> Result<GetConfigResponse, BusrtError> {
        let config_manager = self.config_manager.lock().await;
        config_manager.get_config(request).await
    }

    #[instrument(skip(self))]
    async fn validate_config(
        &self,
        request: ValidateConfigRequest,
    ) -> Result<ValidateConfigResponse, BusrtError> {
        let config_manager = self.config_manager.lock().await;
        config_manager.validate_config(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::busrt_types::CollectorConfig;
    use crate::busrt_types::HealthStatus;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_collector_lifecycle() {
        let temp_dir = TempDir::new().unwrap();
        let config_settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..ConfigManagerSettings::default()
        };
        let config_manager = ConfigManager::new(config_settings);

        let health_config = HealthMonitorConfig::default();
        let health_monitor = HealthMonitor::new(health_config);

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        let manager = CollectorLifecycleManager {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        };

        // Test start collector
        let start_request = StartCollectorRequest {
            collector_type: "process".to_string(),
            collector_id: "test-collector".to_string(),
            config: CollectorConfig {
                collector_type: "process".to_string(),
                scan_interval_ms: 1000,
                batch_size: 100,
                timeout_seconds: 30,
                capabilities: vec!["process".to_string()],
                settings: HashMap::new(),
            },
            environment: HashMap::new(),
        };

        let start_response = manager.start_collector(start_request).await.unwrap();
        assert!(start_response.success);
        assert_eq!(start_response.collector_id, "test-collector");

        // Test get status
        let status_request = StatusRequest {
            collector_id: "test-collector".to_string(),
        };
        let status_response = manager.get_collector_status(status_request).await.unwrap();
        assert_eq!(status_response.collector_id, "test-collector");
        assert_eq!(status_response.status, CollectorStatus::Running);

        // Test pause collector
        let pause_request = PauseCollectorRequest {
            collector_id: "test-collector".to_string(),
            reason: "Testing pause functionality".to_string(),
        };
        let pause_response = manager.pause_collector(pause_request).await.unwrap();
        assert!(pause_response.success);

        // Test resume collector
        let resume_request = ResumeCollectorRequest {
            collector_id: "test-collector".to_string(),
        };
        let resume_response = manager.resume_collector(resume_request).await.unwrap();
        assert!(resume_response.success);

        // Test stop collector
        let stop_request = StopCollectorRequest {
            collector_id: "test-collector".to_string(),
            force: false,
            timeout_seconds: 30,
        };
        let stop_response = manager.stop_collector(stop_request).await.unwrap();
        assert!(stop_response.success);
    }

    #[tokio::test]
    async fn test_health_check() {
        let temp_dir = TempDir::new().unwrap();
        let config_settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..ConfigManagerSettings::default()
        };
        let config_manager = ConfigManager::new(config_settings);

        let health_config = HealthMonitorConfig::default();
        let mut health_monitor = HealthMonitor::new(health_config);
        health_monitor.start().await.unwrap();

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        let manager = CollectorLifecycleManager {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        };

        // Register a collector first
        manager
            .register_collector(
                "health-test".to_string(),
                "process".to_string(),
                vec!["process".to_string()],
            )
            .await
            .unwrap();

        // Test heartbeat
        let heartbeat_request = HeartbeatRequest {
            collector_id: "health-test".to_string(),
        };
        let heartbeat_response = manager.heartbeat(heartbeat_request).await.unwrap();
        assert!(heartbeat_response.timestamp > 0);

        // Test health check
        let health_request = HealthCheckRequest {
            collector_id: "health-test".to_string(),
            include_metrics: true,
        };
        let health_response = manager.health_check(health_request).await.unwrap();
        assert_eq!(health_response.status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_configuration_management() {
        let temp_dir = TempDir::new().unwrap();
        let config_settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..ConfigManagerSettings::default()
        };
        let config_manager = ConfigManager::new(config_settings);

        let health_config = HealthMonitorConfig::default();
        let health_monitor = HealthMonitor::new(health_config);

        let shutdown_config = ShutdownConfig::default();
        let shutdown_coordinator = ShutdownCoordinator::new(shutdown_config);

        let manager = CollectorLifecycleManager {
            collectors: Arc::new(RwLock::new(HashMap::new())),
            config_manager: Arc::new(Mutex::new(config_manager)),
            health_monitor: Arc::new(Mutex::new(health_monitor)),
            shutdown_coordinator: Arc::new(Mutex::new(shutdown_coordinator)),
        };

        let config = CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 1000,
            batch_size: 100,
            timeout_seconds: 30,
            capabilities: vec!["process".to_string()],
            settings: HashMap::new(),
        };

        // Test validate config
        let validate_request = ValidateConfigRequest {
            config: config.clone(),
        };
        let validate_response = manager.validate_config(validate_request).await.unwrap();
        assert!(validate_response.valid);

        // Register collector and store config
        manager
            .register_collector(
                "config-test".to_string(),
                "process".to_string(),
                vec!["process".to_string()],
            )
            .await
            .unwrap();

        let update_request = UpdateConfigRequest {
            collector_id: "config-test".to_string(),
            new_config: config.clone(),
            validate_only: false,
        };
        let update_response = manager.update_config(update_request).await.unwrap();
        assert!(update_response.success);

        // Test get config
        let get_request = GetConfigRequest {
            collector_id: "config-test".to_string(),
        };
        let get_response = manager.get_config(get_request).await.unwrap();
        assert_eq!(get_response.config.collector_type, "process");
    }
}
