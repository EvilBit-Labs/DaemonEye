//! Embedded `EventBus` broker management for daemoneye-agent
//!
//! This module provides the `BrokerManager` which embeds a `DaemoneyeBroker` instance
//! within the daemoneye-agent process architecture. The broker operates independently
//! of the IPC server for CLI communication and provides topic-based pub/sub messaging
//! for collector-core component coordination.
//!
//! # Agent Loading State Machine
//!
//! The agent implements a state machine to coordinate startup:
//!
//! ```text
//! Loading → Ready → SteadyState
//! ```
//!
//! - **Loading**: Agent starting, broker initializing, spawning collectors
//! - **Ready**: All collectors registered and reported "ready", privileges dropped
//! - **`SteadyState`**: Normal operation, collectors monitoring

use crate::collector_config::CollectorsConfig;
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
use std::collections::HashSet;
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

/// Agent loading state machine.
///
/// The agent progresses through these states during startup:
/// - Loading → Ready → `SteadyState`
///
/// This ensures coordinated startup where all collectors are ready before
/// the agent drops privileges and enters normal operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)] // Variants constructed by state machine methods, used in Task #15
pub enum AgentState {
    /// Agent is starting up, broker initializing, spawning collectors.
    /// The agent waits for all expected collectors to register and report "ready".
    Loading,

    /// All collectors have registered and reported "ready".
    /// The agent has dropped privileges (if configured).
    /// Waiting to broadcast "begin monitoring" to transition to steady state.
    Ready,

    /// Normal operation. Collectors are actively monitoring.
    /// The agent broadcasts "begin monitoring" when entering this state.
    SteadyState,

    /// Agent failed to start (collectors didn't report ready within timeout).
    StartupFailed { reason: String },

    /// Agent is shutting down.
    ShuttingDown,
}

impl AgentState {
    /// Returns true if the agent is in a state where it's accepting collector registrations.
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn accepts_registrations(&self) -> bool {
        matches!(self, Self::Loading | Self::Ready | Self::SteadyState)
    }

    /// Returns true if the agent is in a running state (not failed or shutting down).
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Loading | Self::Ready | Self::SteadyState)
    }

    /// Returns true if the agent startup has failed.
    #[allow(dead_code)] // Used in Task #15 when main.rs integrates state machine
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::StartupFailed { .. })
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Loading => write!(f, "Loading"),
            Self::Ready => write!(f, "Ready"),
            Self::SteadyState => write!(f, "SteadyState"),
            Self::StartupFailed { ref reason } => write!(f, "StartupFailed: {reason}"),
            Self::ShuttingDown => write!(f, "ShuttingDown"),
        }
    }
}

/// Tracks which collectors have reported ready status.
#[derive(Debug)]
#[allow(dead_code)] // Methods used by BrokerManager state machine, integrated in Task #15
struct CollectorReadinessTracker {
    /// Set of collector IDs that are expected to report ready.
    expected_collectors: HashSet<String>,
    /// Set of collector IDs that have reported ready.
    ready_collectors: HashSet<String>,
}

#[allow(dead_code)] // Methods used by BrokerManager state machine, integrated in Task #15
impl CollectorReadinessTracker {
    /// Create a new readiness tracker with expected collector IDs.
    #[allow(dead_code)] // May be used in future for direct initialization
    fn new(expected_collectors: HashSet<String>) -> Self {
        Self {
            expected_collectors,
            ready_collectors: HashSet::new(),
        }
    }

    /// Create an empty tracker (no collectors expected).
    fn empty() -> Self {
        Self {
            expected_collectors: HashSet::new(),
            ready_collectors: HashSet::new(),
        }
    }

    /// Mark a collector as ready.
    fn mark_ready(&mut self, collector_id: &str) {
        self.ready_collectors.insert(collector_id.to_owned());
    }

    /// Check if all expected collectors are ready.
    fn all_ready(&self) -> bool {
        if self.expected_collectors.is_empty() {
            // No collectors expected, consider ready
            return true;
        }
        self.expected_collectors
            .iter()
            .all(|id| self.ready_collectors.contains(id))
    }

    /// Get the list of collectors that haven't reported ready yet.
    fn pending_collectors(&self) -> Vec<&str> {
        self.expected_collectors
            .iter()
            .filter(|id| !self.ready_collectors.contains(*id))
            .map(String::as_str)
            .collect()
    }

    /// Get the number of expected collectors.
    fn expected_count(&self) -> usize {
        self.expected_collectors.len()
    }

    /// Get the number of ready collectors.
    fn ready_count(&self) -> usize {
        self.ready_collectors.len()
    }

    /// Set expected collectors from configuration.
    fn set_expected(&mut self, expected: HashSet<String>) {
        self.expected_collectors = expected;
    }

    /// Reset ready status for all collectors.
    #[allow(dead_code)] // May be used in future for restart scenarios
    fn reset(&mut self) {
        self.ready_collectors.clear();
    }
}

/// Embedded broker manager that coordinates the `DaemoneyeBroker` lifecycle
/// within the daemoneye-agent process architecture.
///
/// The broker manager also implements the agent loading state machine that
/// coordinates startup between the agent and its collectors.
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
    /// Current agent state (loading state machine)
    #[allow(dead_code)] // Used by state machine methods, integrated in Task #15
    agent_state: Arc<RwLock<AgentState>>,
    /// Collectors configuration (loaded from file)
    #[allow(dead_code)] // Used by state machine methods, integrated in Task #15
    collectors_config: Arc<RwLock<CollectorsConfig>>,
    /// Tracks which collectors have reported ready
    #[allow(dead_code)] // Used by state machine methods, integrated in Task #15
    readiness_tracker: Arc<RwLock<CollectorReadinessTracker>>,
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
            agent_state: Arc::new(RwLock::new(AgentState::Loading)),
            collectors_config: Arc::new(RwLock::new(CollectorsConfig::default())),
            readiness_tracker: Arc::new(RwLock::new(CollectorReadinessTracker::empty())),
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

    // -------------------------------------------------------
    // Agent Loading State Machine
    // These methods will be used when main.rs integrates the state machine (Task #15)
    // -------------------------------------------------------

    /// Get the current agent state.
    #[allow(dead_code)]
    pub async fn agent_state(&self) -> AgentState {
        self.agent_state.read().await.clone()
    }

    /// Set the collectors configuration and update the readiness tracker.
    ///
    /// This should be called during agent startup after loading the configuration file.
    /// Only enabled collectors are added to the expected collectors set.
    #[allow(dead_code)]
    pub async fn set_collectors_config(&self, config: CollectorsConfig) {
        // Extract enabled collector IDs
        let expected: HashSet<String> = config
            .collectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.clone())
            .collect();

        let expected_count = expected.len();

        // Update readiness tracker with expected collectors
        self.readiness_tracker.write().await.set_expected(expected);

        // Store the configuration
        *self.collectors_config.write().await = config;

        info!(
            expected_collectors = expected_count,
            "Configured collector readiness tracking"
        );
    }

    /// Get the current collectors configuration.
    #[allow(dead_code)]
    pub async fn collectors_config(&self) -> CollectorsConfig {
        self.collectors_config.read().await.clone()
    }

    /// Mark a collector as ready and check if all collectors are ready.
    ///
    /// Returns `true` if all expected collectors are now ready.
    /// This method is typically called when a collector sends a "ready" status
    /// during its registration handshake.
    #[allow(dead_code)]
    pub async fn mark_collector_ready(&self, collector_id: &str) -> bool {
        let mut tracker = self.readiness_tracker.write().await;
        tracker.mark_ready(collector_id);
        let all_ready = tracker.all_ready();

        info!(
            collector_id = %collector_id,
            ready_count = tracker.ready_count(),
            expected_count = tracker.expected_count(),
            all_ready = all_ready,
            "Collector marked as ready"
        );

        all_ready
    }

    /// Get the list of collectors that haven't reported ready yet.
    #[allow(dead_code)]
    pub async fn pending_collectors(&self) -> Vec<String> {
        let tracker = self.readiness_tracker.read().await;
        tracker
            .pending_collectors()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Check if all expected collectors have reported ready.
    #[allow(dead_code)]
    pub async fn all_collectors_ready(&self) -> bool {
        let tracker = self.readiness_tracker.read().await;
        tracker.all_ready()
    }

    /// Transition from Loading to Ready state.
    ///
    /// This transition occurs when all expected collectors have registered and
    /// reported "ready" status. Returns `Ok(())` if the transition is valid,
    /// or `Err` if the current state doesn't allow this transition.
    #[allow(dead_code)]
    pub async fn transition_to_ready(&self) -> Result<()> {
        let mut state = self.agent_state.write().await;

        match *state {
            AgentState::Loading => {
                // Verify all collectors are ready before transitioning
                let tracker = self.readiness_tracker.read().await;
                if !tracker.all_ready() {
                    let pending: Vec<_> = tracker.pending_collectors().into_iter().collect();
                    anyhow::bail!(
                        "Cannot transition to Ready: collectors still pending: {pending:?}"
                    );
                }
                drop(tracker);

                info!(
                    previous_state = %*state,
                    "Agent transitioning to Ready state"
                );
                *state = AgentState::Ready;
                Ok(())
            }
            AgentState::Ready | AgentState::SteadyState => {
                // Already in Ready or beyond - no-op
                debug!(current_state = %*state, "Agent already in Ready or SteadyState");
                Ok(())
            }
            AgentState::StartupFailed { ref reason } => {
                anyhow::bail!("Cannot transition to Ready: startup failed: {reason}");
            }
            AgentState::ShuttingDown => {
                anyhow::bail!("Cannot transition to Ready: agent is shutting down");
            }
        }
    }

    /// Transition from Ready to `SteadyState` and broadcast "begin monitoring".
    ///
    /// This transition occurs after privilege dropping (if configured) and
    /// signals all collectors to begin their monitoring operations.
    #[allow(dead_code)]
    pub async fn transition_to_steady_state(&self) -> Result<()> {
        let mut state = self.agent_state.write().await;

        match *state {
            AgentState::Ready => {
                info!(
                    previous_state = %*state,
                    "Agent transitioning to SteadyState"
                );
                *state = AgentState::SteadyState;

                // Broadcast "begin monitoring" to all collectors
                drop(state); // Release lock before async operations
                self.broadcast_begin_monitoring().await?;

                Ok(())
            }
            AgentState::SteadyState => {
                // Already in SteadyState - no-op
                debug!("Agent already in SteadyState");
                Ok(())
            }
            AgentState::Loading => {
                anyhow::bail!(
                    "Cannot transition to SteadyState: must be in Ready state first (currently Loading)"
                );
            }
            AgentState::StartupFailed { ref reason } => {
                anyhow::bail!("Cannot transition to SteadyState: startup failed: {reason}");
            }
            AgentState::ShuttingDown => {
                anyhow::bail!("Cannot transition to SteadyState: agent is shutting down");
            }
        }
    }

    /// Mark the agent startup as failed.
    ///
    /// This is called when the startup timeout expires before all collectors
    /// have reported ready.
    #[allow(dead_code)]
    pub async fn mark_startup_failed(&self, reason: String) {
        let mut state = self.agent_state.write().await;

        // Only transition to failed if still in Loading state
        if matches!(*state, AgentState::Loading) {
            let pending = {
                let tracker = self.readiness_tracker.read().await;
                tracker
                    .pending_collectors()
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            };

            error!(
                reason = %reason,
                pending_collectors = ?pending,
                "Agent startup failed"
            );

            *state = AgentState::StartupFailed {
                reason: reason.clone(),
            };
        }
    }

    /// Transition to `ShuttingDown` state.
    ///
    /// This should be called before initiating the shutdown sequence.
    #[allow(dead_code)]
    pub async fn transition_to_shutting_down(&self) {
        let mut state = self.agent_state.write().await;
        let previous = state.clone();

        *state = AgentState::ShuttingDown;
        drop(state);
        info!(previous_state = %previous, "Agent transitioning to ShuttingDown state");
    }

    /// Wait for all expected collectors to be ready within the given timeout.
    ///
    /// This method polls the readiness tracker at intervals until all collectors
    /// are ready or the timeout expires. If the timeout expires, the startup is
    /// marked as failed.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for all collectors to be ready
    /// * `poll_interval` - How often to check readiness status
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - All collectors are ready
    /// * `Ok(false)` - Timeout expired before all collectors were ready (startup marked as failed)
    #[allow(dead_code)]
    pub async fn wait_for_collectors_ready(
        &self,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<bool> {
        let start = std::time::Instant::now();

        info!(
            timeout_secs = timeout.as_secs(),
            "Waiting for all collectors to be ready"
        );

        loop {
            // Check if all collectors are ready
            if self.all_collectors_ready().await {
                let elapsed = start.elapsed();
                info!(elapsed_secs = elapsed.as_secs(), "All collectors are ready");
                return Ok(true);
            }

            // Check if we've timed out
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                let pending = self.pending_collectors().await;
                warn!(
                    timeout_secs = timeout.as_secs(),
                    pending_collectors = ?pending,
                    "Startup timeout expired waiting for collectors"
                );

                // Mark startup as failed
                self.mark_startup_failed(format!(
                    "Timeout after {} seconds waiting for collectors: {:?}",
                    timeout.as_secs(),
                    pending
                ))
                .await;

                return Ok(false);
            }

            // Log progress periodically
            let pending = self.pending_collectors().await;
            debug!(
                elapsed_secs = elapsed.as_secs(),
                pending_count = pending.len(),
                pending_collectors = ?pending,
                "Still waiting for collectors to be ready"
            );

            // Wait before polling again
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Drop privileges after all collectors have registered and reported ready.
    ///
    /// This method is a stub that will be implemented with platform-specific
    /// privilege dropping logic in the future. Currently it just logs that
    /// privilege dropping would occur.
    ///
    /// # Platform-specific behavior (future)
    ///
    /// - **Unix**: Drop to a non-root user (e.g., `daemoneye`)
    /// - **Windows**: Reduce process token privileges
    /// - **macOS**: Drop supplementary groups and effective UID
    ///
    /// # Safety
    ///
    /// This should only be called after all collectors have been spawned and
    /// have completed their privileged initialization (e.g., binding to
    /// privileged ports, accessing protected resources).
    #[allow(dead_code)]
    #[allow(clippy::unused_async)] // Future implementation will use async for platform-specific privilege dropping
    pub async fn drop_privileges(&self) -> Result<()> {
        info!("Privilege dropping requested (stub - not yet implemented)");

        // Future implementation will:
        // 1. Check if running as root/elevated
        // 2. Drop supplementary groups
        // 3. Set effective UID/GID to unprivileged user
        // 4. Verify privileges were dropped successfully

        // For now, just log and succeed
        debug!("Privilege dropping is not implemented - running with current privileges");

        Ok(())
    }

    /// Get the default startup timeout from the collectors configuration.
    ///
    /// Returns the maximum startup timeout among all enabled collectors,
    /// or a default of 60 seconds if no collectors are configured.
    #[allow(dead_code)]
    pub async fn get_startup_timeout(&self) -> Duration {
        let max_timeout = self
            .collectors_config
            .read()
            .await
            .collectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.startup_timeout_secs)
            .max()
            .unwrap_or(60);

        Duration::from_secs(max_timeout)
    }

    /// Broadcast "begin monitoring" message to all collectors.
    ///
    /// This is sent on the `control.collector.lifecycle` topic to signal
    /// all collectors that they should begin their monitoring operations.
    #[allow(dead_code)]
    #[allow(clippy::significant_drop_tightening)] // Guard must be held while broker is used
    pub async fn broadcast_begin_monitoring(&self) -> Result<()> {
        let broker_guard = self.broker.read().await;
        let Some(broker) = broker_guard.as_ref() else {
            warn!("Cannot broadcast 'begin monitoring': broker not available");
            return Ok(());
        };

        let topic = "control.collector.lifecycle";

        // Create the lifecycle message
        let message = serde_json::json!({
            "type": "BeginMonitoring",
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            "source": "daemoneye-agent",
        });

        let payload =
            serde_json::to_vec(&message).context("Failed to serialize BeginMonitoring message")?;

        broker
            .publish(topic, "begin-monitoring", payload)
            .await
            .context("Failed to publish BeginMonitoring message")?;

        info!(
            topic = %topic,
            "Broadcast 'begin monitoring' to all collectors"
        );

        Ok(())
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
        if response.accepted {
            if let Err(e) = self.create_rpc_client(&request.collector_id).await {
                warn!(
                    collector_id = %request.collector_id,
                    error = %e,
                    "Failed to create RPC client after registration"
                );
            }

            // Mark collector as ready when it registers successfully
            // Registration indicates the collector has completed its initialization
            let all_ready = self.mark_collector_ready(&request.collector_id).await;

            // Check if we should transition to Ready state
            if all_ready {
                let current_state = self.agent_state().await;
                if matches!(current_state, AgentState::Loading) {
                    info!("All expected collectors are ready, agent can transition to Ready state");
                    // Note: The actual transition is triggered by main.rs or startup coordination
                    // to ensure proper privilege dropping and other startup steps
                }
            }
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
    use crate::collector_config::CollectorEntry;
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

    // -------------------------------------------------------
    // Agent State Machine Tests
    // -------------------------------------------------------

    #[tokio::test]
    async fn test_agent_state_initial_loading() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let state = manager.agent_state().await;
        assert_eq!(state, AgentState::Loading);
    }

    #[tokio::test]
    async fn test_agent_state_display() {
        assert_eq!(format!("{}", AgentState::Loading), "Loading");
        assert_eq!(format!("{}", AgentState::Ready), "Ready");
        assert_eq!(format!("{}", AgentState::SteadyState), "SteadyState");
        assert_eq!(
            format!(
                "{}",
                AgentState::StartupFailed {
                    reason: "timeout".to_owned()
                }
            ),
            "StartupFailed: timeout"
        );
        assert_eq!(format!("{}", AgentState::ShuttingDown), "ShuttingDown");
    }

    #[tokio::test]
    async fn test_agent_state_helper_methods() {
        assert!(AgentState::Loading.accepts_registrations());
        assert!(AgentState::Ready.accepts_registrations());
        assert!(AgentState::SteadyState.accepts_registrations());
        assert!(
            !AgentState::StartupFailed {
                reason: "test".to_owned()
            }
            .accepts_registrations()
        );
        assert!(!AgentState::ShuttingDown.accepts_registrations());

        assert!(AgentState::Loading.is_running());
        assert!(AgentState::Ready.is_running());
        assert!(AgentState::SteadyState.is_running());
        assert!(
            !AgentState::StartupFailed {
                reason: "test".to_owned()
            }
            .is_running()
        );
        assert!(!AgentState::ShuttingDown.is_running());

        assert!(!AgentState::Loading.is_failed());
        assert!(!AgentState::Ready.is_failed());
        assert!(
            AgentState::StartupFailed {
                reason: "test".to_owned()
            }
            .is_failed()
        );
    }

    #[tokio::test]
    async fn test_set_collectors_config() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        let collectors_config = CollectorsConfig {
            collectors: vec![
                CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1"),
                CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2")
                    .with_enabled(false),
            ],
        };

        manager
            .set_collectors_config(collectors_config.clone())
            .await;

        let loaded_config = manager.collectors_config().await;
        assert_eq!(loaded_config.collectors.len(), 2);

        // Only enabled collectors should be in the readiness tracker
        let pending = manager.pending_collectors().await;
        assert_eq!(pending.len(), 1);
        assert!(pending.contains(&"collector-1".to_owned()));
    }

    #[tokio::test]
    async fn test_mark_collector_ready() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Configure with two enabled collectors
        let collectors_config = CollectorsConfig {
            collectors: vec![
                CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1"),
                CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2"),
            ],
        };

        manager.set_collectors_config(collectors_config).await;

        // Initially not all ready
        assert!(!manager.all_collectors_ready().await);

        // Mark first collector ready
        let all_ready = manager.mark_collector_ready("collector-1").await;
        assert!(!all_ready);

        // Mark second collector ready
        let all_ready = manager.mark_collector_ready("collector-2").await;
        assert!(all_ready);

        assert!(manager.all_collectors_ready().await);
        assert!(manager.pending_collectors().await.is_empty());
    }

    #[tokio::test]
    async fn test_all_collectors_ready_with_no_collectors() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Empty config means no collectors expected - should be ready by default
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;
        assert!(manager.all_collectors_ready().await);
    }

    #[tokio::test]
    async fn test_transition_to_ready_succeeds() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Configure with no collectors (automatically ready)
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;

        // Should succeed since all collectors are ready
        let result = manager.transition_to_ready().await;
        assert!(result.is_ok());

        let state = manager.agent_state().await;
        assert_eq!(state, AgentState::Ready);
    }

    #[tokio::test]
    async fn test_transition_to_ready_fails_with_pending_collectors() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Configure with a collector that hasn't reported ready
        let collectors_config = CollectorsConfig {
            collectors: vec![CollectorEntry::new(
                "pending-collector",
                "type-a",
                "/usr/bin/collector",
            )],
        };
        manager.set_collectors_config(collectors_config).await;

        // Should fail since collector hasn't reported ready
        let result = manager.transition_to_ready().await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("pending-collector"));

        // State should still be Loading
        let state = manager.agent_state().await;
        assert_eq!(state, AgentState::Loading);
    }

    #[tokio::test]
    async fn test_transition_to_ready_is_idempotent() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;

        // First transition
        manager.transition_to_ready().await.unwrap();
        assert_eq!(manager.agent_state().await, AgentState::Ready);

        // Second transition should succeed (no-op)
        let result = manager.transition_to_ready().await;
        assert!(result.is_ok());
        assert_eq!(manager.agent_state().await, AgentState::Ready);
    }

    #[tokio::test]
    async fn test_transition_to_steady_state_fails_from_loading() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Try to skip directly to SteadyState from Loading
        let result = manager.transition_to_steady_state().await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("must be in Ready state first"));
    }

    #[tokio::test]
    async fn test_mark_startup_failed() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Configure with a collector that won't report ready
        let collectors_config = CollectorsConfig {
            collectors: vec![CollectorEntry::new(
                "slow-collector",
                "type-a",
                "/usr/bin/collector",
            )],
        };
        manager.set_collectors_config(collectors_config).await;

        // Mark startup as failed
        manager
            .mark_startup_failed("Timeout waiting for collectors".to_owned())
            .await;

        let state = manager.agent_state().await;
        match state {
            AgentState::StartupFailed { reason } => {
                assert!(reason.contains("Timeout"));
            }
            _ => panic!("Expected StartupFailed state, got {state:?}"),
        }
    }

    #[tokio::test]
    async fn test_mark_startup_failed_only_from_loading() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;

        // Transition to Ready first
        manager.transition_to_ready().await.unwrap();
        assert_eq!(manager.agent_state().await, AgentState::Ready);

        // Try to mark as failed - should be ignored since not in Loading state
        manager
            .mark_startup_failed("This should be ignored".to_owned())
            .await;

        // State should still be Ready
        assert_eq!(manager.agent_state().await, AgentState::Ready);
    }

    #[tokio::test]
    async fn test_transition_to_shutting_down() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;

        // Transition through states
        manager.transition_to_ready().await.unwrap();
        manager.transition_to_shutting_down().await;

        assert_eq!(manager.agent_state().await, AgentState::ShuttingDown);
    }

    #[tokio::test]
    async fn test_full_state_machine_lifecycle() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Step 1: Start in Loading
        assert_eq!(manager.agent_state().await, AgentState::Loading);

        // Step 2: Configure collectors
        let collectors_config = CollectorsConfig {
            collectors: vec![CollectorEntry::new(
                "test-collector",
                "type-a",
                "/usr/bin/collector",
            )],
        };
        manager.set_collectors_config(collectors_config).await;
        assert!(!manager.all_collectors_ready().await);

        // Step 3: Mark collector as ready
        manager.mark_collector_ready("test-collector").await;
        assert!(manager.all_collectors_ready().await);

        // Step 4: Transition to Ready
        manager.transition_to_ready().await.unwrap();
        assert_eq!(manager.agent_state().await, AgentState::Ready);

        // Step 5: Shutdown
        manager.transition_to_shutting_down().await;
        assert_eq!(manager.agent_state().await, AgentState::ShuttingDown);
    }

    #[tokio::test]
    async fn test_registration_marks_collector_ready() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Initialize registry
        {
            let mut guard = manager.collector_registry.write().await;
            *guard = Some(Arc::new(CollectorRegistry::default()));
        }

        // Configure expected collectors
        let collectors_config = CollectorsConfig {
            collectors: vec![
                CollectorEntry::new("collector-a", "type-a", "/usr/bin/collector-a"),
                CollectorEntry::new("collector-b", "type-b", "/usr/bin/collector-b"),
            ],
        };
        manager.set_collectors_config(collectors_config).await;

        // Initially not all ready
        assert!(!manager.all_collectors_ready().await);
        assert_eq!(manager.pending_collectors().await.len(), 2);

        // Register first collector - should mark it as ready
        let request_a = RegistrationRequest {
            collector_id: "collector-a".to_owned(),
            collector_type: "type-a".to_owned(),
            hostname: "localhost".to_owned(),
            version: Some("1.0.0".to_owned()),
            pid: Some(1234),
            capabilities: vec![],
            attributes: std::collections::HashMap::new(),
            heartbeat_interval_ms: None,
        };

        let response = manager
            .register_collector(request_a)
            .await
            .expect("registration succeeds");
        assert!(response.accepted);

        // First collector should be marked ready
        assert_eq!(manager.pending_collectors().await.len(), 1);
        assert!(!manager.all_collectors_ready().await);

        // Register second collector - should mark it as ready and complete readiness
        let request_b = RegistrationRequest {
            collector_id: "collector-b".to_owned(),
            collector_type: "type-b".to_owned(),
            hostname: "localhost".to_owned(),
            version: Some("1.0.0".to_owned()),
            pid: Some(5678),
            capabilities: vec![],
            attributes: std::collections::HashMap::new(),
            heartbeat_interval_ms: None,
        };

        let response = manager
            .register_collector(request_b)
            .await
            .expect("registration succeeds");
        assert!(response.accepted);

        // Both collectors should now be ready
        assert!(manager.pending_collectors().await.is_empty());
        assert!(manager.all_collectors_ready().await);

        // Should be able to transition to Ready state now
        manager
            .transition_to_ready()
            .await
            .expect("transition succeeds");
        assert_eq!(manager.agent_state().await, AgentState::Ready);
    }

    #[tokio::test]
    async fn test_wait_for_collectors_ready_success() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Initialize registry
        {
            let mut guard = manager.collector_registry.write().await;
            *guard = Some(Arc::new(CollectorRegistry::default()));
        }

        // Configure with a single collector
        let collectors_config = CollectorsConfig {
            collectors: vec![CollectorEntry::new(
                "test-collector",
                "type-a",
                "/usr/bin/collector",
            )],
        };
        manager.set_collectors_config(collectors_config).await;

        // Spawn a task that will register the collector after a short delay
        let manager_clone = Arc::new(manager);
        let manager_for_task = Arc::clone(&manager_clone);

        let registration_task = tokio::spawn(async move {
            // Wait a bit then register
            tokio::time::sleep(Duration::from_millis(50)).await;

            let request = RegistrationRequest {
                collector_id: "test-collector".to_owned(),
                collector_type: "type-a".to_owned(),
                hostname: "localhost".to_owned(),
                version: None,
                pid: None,
                capabilities: vec![],
                attributes: std::collections::HashMap::new(),
                heartbeat_interval_ms: None,
            };

            manager_for_task
                .register_collector(request)
                .await
                .expect("registration succeeds");
        });

        // Wait for collectors with a generous timeout
        let result = manager_clone
            .wait_for_collectors_ready(Duration::from_secs(5), Duration::from_millis(10))
            .await
            .expect("wait succeeds");

        assert!(result, "All collectors should be ready");
        assert!(manager_clone.all_collectors_ready().await);

        // Clean up
        registration_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_wait_for_collectors_ready_timeout() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // Configure with a collector that will never register
        let collectors_config = CollectorsConfig {
            collectors: vec![CollectorEntry::new(
                "never-registers",
                "type-a",
                "/usr/bin/collector",
            )],
        };
        manager.set_collectors_config(collectors_config).await;

        // Wait with a short timeout
        let result = manager
            .wait_for_collectors_ready(Duration::from_millis(100), Duration::from_millis(20))
            .await
            .expect("wait succeeds but returns false");

        assert!(!result, "Should timeout and return false");

        // Agent state should be StartupFailed
        let state = manager.agent_state().await;
        match state {
            AgentState::StartupFailed { reason } => {
                assert!(reason.contains("never-registers"));
                assert!(reason.contains("Timeout"));
            }
            _ => panic!("Expected StartupFailed state, got {state:?}"),
        }
    }

    #[tokio::test]
    async fn test_wait_for_collectors_ready_no_collectors() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // No collectors configured - should succeed immediately
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;

        let result = manager
            .wait_for_collectors_ready(Duration::from_secs(1), Duration::from_millis(10))
            .await
            .expect("wait succeeds");

        assert!(result, "Should succeed immediately with no collectors");
    }

    #[tokio::test]
    async fn test_get_startup_timeout() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // No collectors - default timeout
        manager
            .set_collectors_config(CollectorsConfig::default())
            .await;
        assert_eq!(manager.get_startup_timeout().await, Duration::from_secs(60));

        // With collectors, use max timeout
        let collectors_config = CollectorsConfig {
            collectors: vec![
                CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1")
                    .with_startup_timeout(30),
                CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2")
                    .with_startup_timeout(90),
                CollectorEntry::new("collector-3", "type-c", "/usr/bin/collector3")
                    .with_startup_timeout(45)
                    .with_enabled(false), // Disabled - should be ignored
            ],
        };
        manager.set_collectors_config(collectors_config).await;

        // Should return 90 (max of enabled collectors)
        assert_eq!(manager.get_startup_timeout().await, Duration::from_secs(90));
    }

    #[tokio::test]
    async fn test_drop_privileges_stub() {
        let config = BrokerConfig::default();
        let manager = BrokerManager::new(config);

        // The stub should succeed without doing anything
        let result = manager.drop_privileges().await;
        assert!(result.is_ok());
    }
}
