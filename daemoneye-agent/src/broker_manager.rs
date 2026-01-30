//! Embedded `EventBus` broker management for daemoneye-agent
//!
//! This module provides the `BrokerManager` which embeds a `DaemoneyeBroker` instance
//! within the daemoneye-agent process architecture. The broker operates independently
//! of the IPC server for CLI communication and provides topic-based pub/sub messaging
//! for collector-core component coordination.

use crate::collector_registry::{CollectorRegistry, RegistryError};
use crate::health::{self, HealthState};
use anyhow::{Context, Result};
use daemoneye_eventbus::ConfigManager;
use daemoneye_eventbus::rpc::{
    CollectorLifecycleRequest, CollectorOperation, CollectorRpcClient, ComponentHealth,
    ConfigProvider, ConfigUpdateResult, DeregistrationRequest, HealthCheckData, HealthProvider,
    HealthStatus, RegistrationError, RegistrationProvider, RegistrationRequest,
    RegistrationResponse, RpcPayload, RpcRequest, RpcStatus, ShutdownRequest, ShutdownType,
};
use daemoneye_eventbus::{
    DaemoneyeBroker, DaemoneyeEventBus, EventBus, EventBusStatistics,
    process_manager::CollectorProcessManager,
};
use daemoneye_lib::config::BrokerConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Health status of the embedded broker
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrokerHealth {
    /// Broker is healthy and operational
    Healthy,
    /// Broker is starting up
    Starting,
    /// Broker is shutting down
    ShuttingDown,
    /// Broker has encountered an error
    Unhealthy(String),
    /// Broker is stopped
    Stopped,
}

impl HealthState for BrokerHealth {
    fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    fn is_starting(&self) -> bool {
        matches!(self, Self::Starting)
    }

    fn unhealthy_message(&self) -> Option<&str> {
        match *self {
            Self::Unhealthy(ref msg) => Some(msg),
            Self::Healthy | Self::Starting | Self::ShuttingDown | Self::Stopped => None,
        }
    }

    fn is_stopped_or_shutting_down(&self) -> bool {
        matches!(self, Self::ShuttingDown | Self::Stopped)
    }

    fn service_name() -> &'static str {
        "Broker"
    }
}

/// Embedded broker manager that coordinates the `DaemoneyeBroker` lifecycle
/// within the daemoneye-agent process architecture.
pub struct BrokerManager {
    /// Configuration for the broker
    config: BrokerConfig,
    /// The embedded broker instance
    broker: Arc<RwLock<Option<Arc<DaemoneyeBroker>>>>,
    /// `EventBus` client for agent-side operations
    event_bus: Arc<Mutex<Option<DaemoneyeEventBus>>>,
    /// Current health status
    health_status: Arc<RwLock<BrokerHealth>>,
    /// Shutdown signal sender
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    /// Process manager for collector lifecycle
    process_manager: Arc<CollectorProcessManager>,
    /// Configuration manager for collectors
    config_manager: Arc<ConfigManager>,
    /// Registry tracking registered collectors
    collector_registry: Arc<RwLock<Option<Arc<CollectorRegistry>>>>,
    /// RPC clients for collector lifecycle management
    rpc_clients: Arc<RwLock<std::collections::HashMap<String, Arc<CollectorRpcClient>>>>,
}

impl BrokerManager {
    /// Create a new broker manager with the given configuration
    pub fn new(config: BrokerConfig) -> Self {
        // Convert config to process manager config
        let pm_config = daemoneye_eventbus::process_manager::ProcessManagerConfig {
            collector_binaries: config.collector_binaries.clone(),
            default_graceful_timeout: Duration::from_secs(
                config.process_manager.graceful_shutdown_timeout_seconds,
            ),
            default_force_timeout: Duration::from_secs(
                config.process_manager.force_shutdown_timeout_seconds,
            ),
            health_check_interval: Duration::from_secs(
                config.process_manager.health_check_interval_seconds,
            ),
            enable_auto_restart: config.process_manager.enable_auto_restart,
            heartbeat_timeout_multiplier: 3, // Default: 3 missed heartbeats = timeout
        };

        let process_manager = CollectorProcessManager::new(pm_config);

        // Initialize configuration manager with configured directory
        let config_manager = Arc::new(ConfigManager::new(config.config_directory.clone()));

        Self {
            config,
            broker: Arc::new(RwLock::new(None)),
            event_bus: Arc::new(Mutex::new(None)),
            health_status: Arc::new(RwLock::new(BrokerHealth::Stopped)),
            shutdown_tx: Arc::new(Mutex::new(None)),
            process_manager,
            config_manager,
            collector_registry: Arc::new(RwLock::new(None)),
            rpc_clients: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Initialize and start the embedded broker
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Embedded broker is disabled in configuration");
            return Ok(());
        }

        // Update health status to starting
        *self.health_status.write().await = BrokerHealth::Starting;

        info!(
            socket_path = %self.config.socket_path,
            max_connections = self.config.max_connections,
            "Starting embedded DaemonEye EventBus broker"
        );

        // Ensure config directory exists
        if !self.config.config_directory.exists() {
            tokio::fs::create_dir_all(&self.config.config_directory)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create config directory: {}",
                        self.config.config_directory.display()
                    )
                })?;
            info!(
                config_dir = %self.config.config_directory.display(),
                "Created config directory"
            );
        }

        self.config
            .ensure_socket_directory()
            .context("Failed to prepare broker socket directory")?;

        // Create the broker instance
        let broker = DaemoneyeBroker::new(&self.config.socket_path)
            .await
            .context("Failed to create DaemoneyeBroker")?;

        // Create EventBus client from the broker (this will start the broker internally)
        let event_bus = DaemoneyeEventBus::from_broker(broker)
            .await
            .context("Failed to create DaemoneyeEventBus from broker")?;

        // Get the broker reference from the event bus
        let broker_arc = Arc::clone(event_bus.broker());

        // Store the broker and event bus
        *self.broker.write().await = Some(Arc::clone(&broker_arc));
        *self.event_bus.lock().await = Some(event_bus);

        // Initialize collector registry
        *self.collector_registry.write().await = Some(Arc::new(CollectorRegistry::default()));

        // Update health status to healthy
        *self.health_status.write().await = BrokerHealth::Healthy;

        info!("Embedded DaemonEye EventBus broker started successfully");
        Ok(())
    }

    /// Gracefully shutdown the embedded broker
    pub async fn shutdown(&self) -> Result<()> {
        info!("Initiating graceful shutdown of embedded broker");

        // Update health status to shutting down
        *self.health_status.write().await = BrokerHealth::ShuttingDown;

        // Send graceful shutdown RPC to all collectors first
        info!("Sending graceful shutdown RPC to all collectors");
        let collector_ids: Vec<String> = {
            let clients = self.rpc_clients.read().await;
            clients.keys().cloned().collect()
        };

        for collector_id in &collector_ids {
            if let Err(e) = self.stop_collector_rpc(collector_id, true).await {
                warn!(
                    collector_id = %collector_id,
                    error = %e,
                    "Failed to send graceful shutdown RPC, will fall back to signal-based shutdown"
                );
            }
        }

        // Wait a bit for RPC responses
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Shutdown all managed collector processes (fallback to signals)
        info!("Shutting down managed collector processes");
        if let Err(e) = self.process_manager.shutdown_all().await {
            error!(error = %e, "Failed to shutdown all collector processes");
            // Continue with broker shutdown even if collector shutdown fails
        } else {
            info!("All collector processes shut down successfully");
        }

        // Clean up RPC clients
        {
            let mut clients = self.rpc_clients.write().await;
            for (collector_id, client) in clients.drain() {
                if let Err(e) = client.shutdown().await {
                    warn!(
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to shutdown RPC client"
                    );
                }
            }
        }

        // Send shutdown signal if available
        {
            let mut shutdown_tx_guard = self.shutdown_tx.lock().await;
            if let Some(tx) = shutdown_tx_guard.take()
                && tx.send(()).is_err()
            {
                warn!("Failed to send shutdown signal - receiver may have been dropped");
            }
        }

        // Shutdown the event bus first
        {
            let mut event_bus_guard = self.event_bus.lock().await;
            if let Some(mut event_bus) = event_bus_guard.take()
                && let Err(e) = event_bus.shutdown().await
            {
                error!(error = %e, "Failed to shutdown EventBus client");
            }
        }

        // Shutdown the broker with timeout
        let shutdown_timeout = Duration::from_secs(self.config.shutdown_timeout_seconds);
        let shutdown_result = tokio::time::timeout(shutdown_timeout, async {
            let broker_guard = self.broker.read().await;
            if let Some(broker) = broker_guard.as_ref() {
                broker.shutdown().await
            } else {
                Ok(())
            }
        })
        .await;

        match shutdown_result {
            Ok(Ok(())) => {
                info!("Embedded broker shutdown completed successfully");
            }
            Ok(Err(e)) => {
                error!(error = %e, "Error during broker shutdown");
                return Err(e.into());
            }
            Err(_) => {
                warn!(
                    timeout_seconds = self.config.shutdown_timeout_seconds,
                    "Broker shutdown timed out, forcing termination"
                );
            }
        }

        // Clear the broker reference
        *self.broker.write().await = None;

        // Clear collector registry
        self.collector_registry.write().await.take();

        // Update health status to stopped
        *self.health_status.write().await = BrokerHealth::Stopped;

        info!("Embedded broker shutdown complete");
        Ok(())
    }

    /// Get the current health status of the broker
    pub async fn health_status(&self) -> BrokerHealth {
        let health = self.health_status.read().await;
        health.clone()
    }

    /// Get broker statistics if available
    pub async fn statistics(&self) -> Option<EventBusStatistics> {
        let broker_guard = self.broker.read().await;
        if let Some(broker) = broker_guard.as_ref() {
            Some(broker.statistics().await)
        } else {
            None
        }
    }

    async fn registry(&self) -> std::result::Result<Arc<CollectorRegistry>, RegistrationError> {
        let guard = self.collector_registry.read().await;
        guard.as_ref().cloned().ok_or_else(|| {
            RegistrationError::Internal("collector registry not initialized".to_owned())
        })
    }

    fn map_registry_error(error: RegistryError) -> RegistrationError {
        match error {
            RegistryError::AlreadyRegistered(id) => RegistrationError::AlreadyRegistered(id),
            RegistryError::NotFound(id) => RegistrationError::NotFound(id),
            RegistryError::Validation(msg) => RegistrationError::Validation(msg),
        }
    }

    /// Get a reference to the `EventBus` client for agent operations
    #[allow(dead_code)]
    pub fn event_bus(&self) -> Arc<Mutex<Option<DaemoneyeEventBus>>> {
        Arc::clone(&self.event_bus)
    }

    /// Check if the broker is currently running
    #[allow(dead_code)]
    pub async fn is_running(&self) -> bool {
        let health = self.health_status.read().await;
        matches!(*health, BrokerHealth::Healthy | BrokerHealth::Starting)
    }

    /// Get the socket path for the broker
    pub fn socket_path(&self) -> &str {
        &self.config.socket_path
    }

    /// Get a reference to the process manager
    #[allow(dead_code)] // Public accessor for future use
    pub const fn process_manager(&self) -> &Arc<CollectorProcessManager> {
        &self.process_manager
    }

    /// Get a reference to the configuration manager
    #[allow(dead_code)]
    pub fn config_manager(&self) -> Arc<ConfigManager> {
        Arc::clone(&self.config_manager)
    }

    /// Perform a health check on the broker
    pub async fn health_check(&self) -> BrokerHealth {
        let current_health = self.health_status().await;

        match current_health {
            BrokerHealth::Healthy => {
                // Verify broker is actually responsive
                if let Some(stats) = self.statistics().await {
                    debug!(
                        messages_published = stats.messages_published,
                        active_subscribers = stats.active_subscribers,
                        uptime_seconds = stats.uptime_seconds,
                        "Broker health check passed"
                    );
                    // Aggregate collector health across all managed collectors
                    let collector_ids = self.process_manager.list_collector_ids().await;
                    let mut any_unhealthy = false;
                    let mut any_degraded = false;
                    for id in collector_ids {
                        match self.process_manager.check_collector_health(&id).await {
                            Ok(daemoneye_eventbus::process_manager::HealthStatus::Unhealthy) => {
                                any_unhealthy = true;
                                break;
                            }
                            Ok(daemoneye_eventbus::process_manager::HealthStatus::Degraded) => {
                                any_degraded = true;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!(collector_id = %id, error = %e, "Failed to check collector health");
                                any_degraded = true;
                            }
                        }
                    }

                    if any_unhealthy {
                        let unhealthy_status = BrokerHealth::Unhealthy(
                            "One or more collectors are unhealthy".to_owned(),
                        );
                        let mut health = self.health_status.write().await;
                        *health = unhealthy_status.clone();
                        unhealthy_status
                    } else if any_degraded {
                        // Represent degraded collector state as Unhealthy with reason
                        let degraded_status = BrokerHealth::Unhealthy(
                            "One or more collectors are degraded".to_owned(),
                        );
                        let mut health = self.health_status.write().await;
                        *health = degraded_status.clone();
                        degraded_status
                    } else {
                        BrokerHealth::Healthy
                    }
                } else {
                    warn!("Broker health check failed - unable to get statistics");
                    let unhealthy_status =
                        BrokerHealth::Unhealthy("Unable to get statistics".to_owned());
                    let mut health = self.health_status.write().await;
                    *health = unhealthy_status.clone();
                    unhealthy_status
                }
            }
            BrokerHealth::Starting
            | BrokerHealth::ShuttingDown
            | BrokerHealth::Unhealthy(_)
            | BrokerHealth::Stopped => current_health,
        }
    }

    /// Wait for the broker to become healthy with a timeout
    pub async fn wait_for_healthy(&self, timeout: Duration) -> Result<()> {
        let health_status = Arc::clone(&self.health_status);
        health::wait_for_healthy(timeout, || async { health_status.read().await.clone() }).await
    }

    /// Create an RPC client for a collector
    pub async fn create_rpc_client(&self, collector_id: &str) -> Result<Arc<CollectorRpcClient>> {
        let broker = {
            let broker_guard = self.broker.read().await;
            Arc::clone(
                broker_guard
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Broker not available"))?,
            )
        };

        let target_topic = format!("control.collector.{collector_id}");
        let client = Arc::new(
            CollectorRpcClient::new(&target_topic, broker)
                .await
                .context("Failed to create RPC client")?,
        );

        // Store the client
        self.rpc_clients
            .write()
            .await
            .insert(collector_id.to_owned(), Arc::clone(&client));

        info!(
            collector_id = %collector_id,
            target_topic = %target_topic,
            "Created RPC client for collector"
        );

        Ok(client)
    }

    /// Start a collector via RPC
    #[allow(dead_code)]
    pub async fn start_collector_rpc(&self, collector_id: &str) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;
        let lifecycle_request = CollectorLifecycleRequest::start(collector_id, None);
        let request = RpcRequest::lifecycle(
            client.client_id.clone(),
            client.target_topic.clone(),
            CollectorOperation::Start,
            lifecycle_request,
            Duration::from_secs(30),
        );

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Start RPC failed: {:?}", response.error_details);
        }

        info!(collector_id = %collector_id, "Collector started via RPC");
        Ok(())
    }

    /// Stop a collector via RPC
    pub async fn stop_collector_rpc(&self, collector_id: &str, graceful: bool) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;

        if graceful {
            let shutdown_request = ShutdownRequest {
                collector_id: collector_id.to_owned(),
                shutdown_type: ShutdownType::Graceful,
                graceful_timeout_ms: 5000,
                force_after_timeout: true,
                reason: Some("Agent-initiated graceful shutdown".to_owned()),
            };
            let request = RpcRequest::shutdown(
                client.client_id.clone(),
                client.target_topic.clone(),
                shutdown_request,
                Duration::from_secs(5),
            );

            let response = client.call(request, Duration::from_secs(5)).await?;
            if response.status != RpcStatus::Success {
                anyhow::bail!("Graceful shutdown RPC failed: {:?}", response.error_details);
            }
        } else {
            let lifecycle_request = CollectorLifecycleRequest::stop(collector_id);
            let request = RpcRequest::lifecycle(
                client.client_id.clone(),
                client.target_topic.clone(),
                CollectorOperation::Stop,
                lifecycle_request,
                Duration::from_secs(5),
            );

            let response = client.call(request, Duration::from_secs(5)).await?;
            if response.status != RpcStatus::Success {
                anyhow::bail!("Stop RPC failed: {:?}", response.error_details);
            }
        }

        info!(collector_id = %collector_id, graceful = graceful, "Collector stopped via RPC");
        Ok(())
    }

    /// Restart a collector via RPC
    #[allow(dead_code)]
    pub async fn restart_collector_rpc(&self, collector_id: &str) -> Result<()> {
        let client = self.get_rpc_client(collector_id).await?;
        let lifecycle_request = CollectorLifecycleRequest::restart(collector_id, None);
        let request = RpcRequest::lifecycle(
            client.client_id.clone(),
            client.target_topic.clone(),
            CollectorOperation::Restart,
            lifecycle_request,
            Duration::from_secs(30),
        );

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Restart RPC failed: {:?}", response.error_details);
        }

        info!(collector_id = %collector_id, "Collector restarted via RPC");
        Ok(())
    }

    /// Perform health check via RPC
    pub async fn health_check_rpc(&self, collector_id: &str) -> Result<HealthCheckData> {
        let client = self.get_rpc_client(collector_id).await?;
        let request = RpcRequest::health_check(
            client.client_id.clone(),
            client.target_topic.clone(),
            Duration::from_secs(10),
        );

        let response = client.call(request, Duration::from_secs(10)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Health check RPC failed: {:?}", response.error_details);
        }

        match response.payload {
            Some(RpcPayload::HealthCheck(health_data)) => Ok(health_data),
            _ => anyhow::bail!("Invalid health check response payload"),
        }
    }

    /// Execute a task via RPC
    pub async fn execute_task_rpc(
        &self,
        collector_id: &str,
        task: daemoneye_lib::proto::DetectionTask,
    ) -> Result<daemoneye_lib::proto::DetectionResult> {
        let client = self.get_rpc_client(collector_id).await?;

        let task_json =
            serde_json::to_value(&task).context("Failed to serialize detection task")?;

        #[allow(clippy::arithmetic_side_effects)] // Safe: SystemTime + Duration is well-defined
        let deadline = std::time::SystemTime::now() + Duration::from_secs(30);
        let request = RpcRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_id: client.client_id.clone(),
            target: client.target_topic.clone(),
            operation: CollectorOperation::ExecuteTask,
            payload: RpcPayload::Task(task_json),
            timestamp: std::time::SystemTime::now(),
            deadline,
            correlation_metadata: daemoneye_eventbus::rpc::RpcCorrelationMetadata::new(
                uuid::Uuid::new_v4().to_string(),
            ),
        };

        let response = client.call(request, Duration::from_secs(30)).await?;
        if response.status != RpcStatus::Success {
            anyhow::bail!("Execute task RPC failed: {:?}", response.error_details);
        }

        match response.payload {
            Some(RpcPayload::TaskResult(value)) => {
                let result: daemoneye_lib::proto::DetectionResult =
                    serde_json::from_value(value)
                        .context("Failed to deserialize detection result")?;
                Ok(result)
            }
            _ => anyhow::bail!("Invalid task execution response payload"),
        }
    }

    /// Get or create an RPC client for a collector
    pub async fn get_rpc_client(&self, collector_id: &str) -> Result<Arc<CollectorRpcClient>> {
        // Check if client already exists
        if let Some(client) = self.rpc_clients.read().await.get(collector_id) {
            return Ok(Arc::clone(client));
        }

        // Create new client
        self.create_rpc_client(collector_id).await
    }

    /// List all registered collector IDs that have RPC clients
    pub async fn list_registered_collector_ids(&self) -> Vec<String> {
        let clients = self.rpc_clients.read().await;
        clients.keys().cloned().collect()
    }
}

// -------------------------------
// RPC Provider trait implementations
// -------------------------------

#[async_trait::async_trait]
impl HealthProvider for BrokerManager {
    async fn get_collector_health(
        &self,
        collector_id: &str,
    ) -> std::result::Result<HealthCheckData, daemoneye_eventbus::ProcessManagerError> {
        let status = self
            .process_manager
            .get_collector_status(collector_id)
            .await?;
        let health = self
            .process_manager
            .check_collector_health(collector_id)
            .await?;

        // Build component details
        let mut components = std::collections::HashMap::new();
        components.insert(
            "process".to_owned(),
            ComponentHealth {
                name: "process".to_owned(),
                status: match health {
                    daemoneye_eventbus::process_manager::HealthStatus::Healthy => {
                        HealthStatus::Healthy
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Degraded => {
                        HealthStatus::Degraded
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Unhealthy => {
                        HealthStatus::Unhealthy
                    }
                    daemoneye_eventbus::process_manager::HealthStatus::Unknown => {
                        HealthStatus::Unknown
                    }
                },
                message: Some(format!("PID: {}, State: {:?}", status.pid, status.state)),
                last_check: std::time::SystemTime::now(),
                check_interval_seconds: self.process_manager.config.health_check_interval.as_secs(),
            },
        );

        let hb_status = if status.missed_heartbeats >= 3 {
            HealthStatus::Unhealthy
        } else if status.missed_heartbeats > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let heartbeat_age = std::time::SystemTime::now()
            .duration_since(status.last_heartbeat)
            .unwrap_or_default()
            .as_secs();

        components.insert(
            "heartbeat".to_owned(),
            ComponentHealth {
                name: "heartbeat".to_owned(),
                status: hb_status,
                message: Some(format!(
                    "Last heartbeat: {}s ago, Missed: {}",
                    heartbeat_age, status.missed_heartbeats
                )),
                last_check: status.last_heartbeat,
                check_interval_seconds: self.process_manager.config.health_check_interval.as_secs(),
            },
        );

        // Event sources health is not yet implemented
        components.insert(
            "event_sources".to_owned(),
            ComponentHealth {
                name: "event_sources".to_owned(),
                status: HealthStatus::Unknown,
                message: Some("Event sources health monitoring not yet implemented".to_owned()),
                last_check: std::time::SystemTime::now(),
                check_interval_seconds: 60,
            },
        );

        // Compute overall health using worst-of aggregation
        let overall = aggregate_worst_of(components.values().map(|c| c.status));

        #[allow(clippy::as_conversions)]
        // Safe: uptime_seconds and heartbeat_age are small u64 values
        let uptime_seconds_f64 = status.uptime.as_secs() as f64;
        #[allow(clippy::as_conversions)]
        let heartbeat_age_f64 = heartbeat_age as f64;

        let mut metrics = std::collections::HashMap::new();
        metrics.insert("pid".to_owned(), f64::from(status.pid));
        metrics.insert("restart_count".to_owned(), f64::from(status.restart_count));
        metrics.insert("uptime_seconds".to_owned(), uptime_seconds_f64);
        metrics.insert(
            "missed_heartbeats".to_owned(),
            f64::from(status.missed_heartbeats),
        );
        metrics.insert("last_heartbeat_age_seconds".to_owned(), heartbeat_age_f64);
        metrics.insert("error_count".to_owned(), 0.0);

        Ok(HealthCheckData {
            collector_id: collector_id.to_owned(),
            status: overall,
            components,
            metrics,
            last_heartbeat: status.last_heartbeat,
            uptime_seconds: status.uptime.as_secs(),
            error_count: 0,
        })
    }
}

#[async_trait::async_trait]
impl ConfigProvider for BrokerManager {
    async fn get_config(
        &self,
        collector_id: &str,
    ) -> std::result::Result<
        daemoneye_eventbus::CollectorConfig,
        daemoneye_eventbus::ConfigManagerError,
    > {
        self.config_manager.get_config(collector_id).await
    }

    async fn update_config(
        &self,
        collector_id: &str,
        changes: std::collections::HashMap<String, serde_json::Value>,
        validate_only: bool,
        rollback_on_failure: bool,
    ) -> std::result::Result<ConfigUpdateResult, daemoneye_eventbus::ConfigManagerError> {
        let current = self.config_manager.get_config(collector_id).await?;
        let snapshot = self
            .config_manager
            .update_config(
                collector_id,
                changes.clone(),
                validate_only,
                rollback_on_failure,
            )
            .await?;

        if validate_only {
            return Ok(ConfigUpdateResult {
                version: snapshot.version,
                changed_fields: Vec::new(),
                restart_performed: false,
                timestamp: std::time::SystemTime::now(),
            });
        }

        let restart_needed =
            daemoneye_eventbus::ConfigManager::requires_restart(&current, &snapshot.config);
        let changed_fields =
            daemoneye_eventbus::ConfigManager::get_changed_fields(&current, &snapshot.config);

        if restart_needed {
            let restart_timeout = Duration::from_secs(30);
            self.process_manager
                .restart_collector(collector_id, restart_timeout)
                .await
                .map_err(|e| {
                    daemoneye_eventbus::ConfigManagerError::PersistenceFailed(format!(
                        "Failed to restart collector after config update: {e}"
                    ))
                })?;
        } else {
            // Publish hot-reload notification if broker is present
            let broker_guard = self.broker.read().await;
            if let Some(broker) = broker_guard.as_ref() {
                let topic = format!("control.collector.config.{collector_id}");
                let notification = daemoneye_eventbus::rpc::ConfigChangeNotification {
                    collector_id: collector_id.to_owned(),
                    changed_fields: changed_fields.clone(),
                    version: snapshot.version,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                match serde_json::to_vec(&notification) {
                    Ok(payload) => {
                        if let Err(e) = broker
                            .publish(&topic, &format!("config-change-{collector_id}"), payload)
                            .await
                        {
                            tracing::warn!(
                                collector_id = %collector_id,
                                topic = %topic,
                                error = %e,
                                "Failed to publish config change notification - collector may not hot-reload"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            collector_id = %collector_id,
                            error = %e,
                            "Failed to serialize config change notification"
                        );
                    }
                }
            }
        }

        Ok(ConfigUpdateResult {
            version: snapshot.version,
            changed_fields,
            restart_performed: restart_needed,
            timestamp: std::time::SystemTime::now(),
        })
    }

    async fn validate_config(
        &self,
        _collector_id: &str,
        config: &daemoneye_eventbus::CollectorConfig,
    ) -> std::result::Result<(), daemoneye_eventbus::ConfigManagerError> {
        self.config_manager.validate_config(config).await
    }
}

// -------------------------------
// RPC Provider trait implementations
// -------------------------------

#[async_trait::async_trait]
impl RegistrationProvider for BrokerManager {
    async fn register_collector(
        &self,
        request: RegistrationRequest,
    ) -> std::result::Result<RegistrationResponse, RegistrationError> {
        let registry = self.registry().await?;
        let response = registry
            .register(request.clone())
            .await
            .map_err(Self::map_registry_error)?;

        // Create RPC client after successful registration
        if response.accepted
            && let Err(e) = self.create_rpc_client(&request.collector_id).await
        {
            warn!(
                collector_id = %request.collector_id,
                error = %e,
                "Failed to create RPC client after registration"
            );
        }

        Ok(response)
    }

    async fn deregister_collector(
        &self,
        request: DeregistrationRequest,
    ) -> std::result::Result<(), RegistrationError> {
        let registry = self.registry().await?;
        registry
            .deregister(request.clone())
            .await
            .map_err(Self::map_registry_error)?;

        // Remove RPC client and shut it down
        let removed_client = self.rpc_clients.write().await.remove(&request.collector_id);
        if let Some(client) = removed_client
            && let Err(e) = client.shutdown().await
        {
            warn!(
                collector_id = %request.collector_id,
                error = %e,
                "Failed to shutdown RPC client during deregistration"
            );
        }

        Ok(())
    }

    async fn update_heartbeat(
        &self,
        collector_id: &str,
    ) -> std::result::Result<(), RegistrationError> {
        let registry = self.registry().await?;
        registry
            .update_heartbeat(collector_id)
            .await
            .map_err(Self::map_registry_error)
    }
}

/// Compute worst-of aggregation across component health statuses.
/// Returns Unhealthy if any component is Unhealthy, else Degraded if any is Degraded,
/// else Healthy if any is Healthy, else Unknown.
fn aggregate_worst_of<I: Iterator<Item = HealthStatus>>(iter: I) -> HealthStatus {
    let mut has_healthy = false;
    let mut has_degraded = false;
    let mut has_unhealthy = false;
    let mut has_unresponsive = false;

    for s in iter {
        match s {
            HealthStatus::Unhealthy => has_unhealthy = true,
            HealthStatus::Unresponsive => has_unresponsive = true,
            HealthStatus::Degraded => has_degraded = true,
            HealthStatus::Healthy => has_healthy = true,
            HealthStatus::Unknown => {}
        }
    }

    // Worst-of aggregation: Unresponsive > Unhealthy > Degraded > Healthy > Unknown
    if has_unresponsive {
        HealthStatus::Unresponsive
    } else if has_unhealthy {
        HealthStatus::Unhealthy
    } else if has_degraded {
        HealthStatus::Degraded
    } else if has_healthy {
        HealthStatus::Healthy
    } else {
        // No components or all unknown
        HealthStatus::Unknown
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::semicolon_outside_block,
    clippy::semicolon_inside_block,
    clippy::semicolon_if_nothing_returned
)]
mod tests {
    use super::*;
    use daemoneye_lib::config::BrokerConfig;

    fn sample_registration_request() -> RegistrationRequest {
        RegistrationRequest {
            collector_id: "test-collector".to_owned(),
            collector_type: "test-collector".to_owned(),
            hostname: "localhost".to_owned(),
            version: Some("1.0.0".to_owned()),
            pid: Some(1234),
            capabilities: vec!["process".to_owned()],
            attributes: std::collections::HashMap::new(),
            heartbeat_interval_ms: Some(5_000),
        }
    }

    #[tokio::test]
    async fn test_broker_manager_creation() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let health = manager.health_status().await;
        assert_eq!(health, BrokerHealth::Stopped);
        assert!(!manager.is_running().await);
    }

    #[tokio::test]
    async fn test_broker_manager_disabled() {
        let config = BrokerConfig {
            enabled: false,
            ..Default::default()
        };

        let manager = BrokerManager::new(config);
        let result = manager.start().await;

        assert!(result.is_ok());
        let health = manager.health_status().await;
        assert_eq!(health, BrokerHealth::Stopped);
    }

    #[tokio::test]
    async fn test_broker_manager_socket_path() {
        let config = BrokerConfig {
            socket_path: "/tmp/test-broker.sock".to_owned(),
            ..Default::default()
        };

        let manager = BrokerManager::new(config);
        assert_eq!(manager.socket_path(), "/tmp/test-broker.sock");
    }

    #[tokio::test]
    async fn test_broker_manager_statistics_when_stopped() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let stats = manager.statistics().await;
        assert!(stats.is_none());
    }

    #[tokio::test]
    async fn test_registration_provider_delegates_to_registry() {
        let manager = BrokerManager::new(BrokerConfig::default());

        {
            let mut guard = manager.collector_registry.write().await;
            *guard = Some(Arc::new(CollectorRegistry::default()))
        };

        let request = sample_registration_request();
        let response = manager
            .register_collector(request.clone())
            .await
            .expect("registration succeeds");
        assert!(response.accepted);
        assert_eq!(response.collector_id, request.collector_id);

        manager
            .update_heartbeat(&request.collector_id)
            .await
            .expect("heartbeat updated");

        manager
            .deregister_collector(DeregistrationRequest {
                collector_id: request.collector_id,
                reason: Some("test".to_owned()),
                force: false,
            })
            .await
            .expect("deregistration succeeds");
    }

    #[tokio::test]
    async fn test_broker_manager_health_check_when_stopped() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let health = manager.health_check().await;
        assert_eq!(health, BrokerHealth::Stopped);
    }

    #[tokio::test]
    async fn test_broker_manager_wait_for_healthy_timeout() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let result = manager.wait_for_healthy(Duration::from_millis(100)).await;
        assert!(result.is_err());
    }
}
