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
//! - **Ready**: All collectors registered and reported "ready"; caller should drop privileges
//! - **`SteadyState`**: Normal operation, collectors monitoring (broadcasts "begin monitoring")

mod health;
mod lifecycle;
mod rpc;
mod state;
mod state_machine;

#[cfg(test)]
mod tests;

pub use health::BrokerHealth;
pub use state::AgentState;

use crate::collector_config::CollectorsConfig;
use crate::collector_registry::CollectorRegistry;
use daemoneye_eventbus::ConfigManager;
use daemoneye_eventbus::rpc::CollectorRpcClient;
use daemoneye_eventbus::{
    DaemoneyeBroker, DaemoneyeEventBus, process_manager::CollectorProcessManager,
};
use daemoneye_lib::config::BrokerConfig;
use state::CollectorReadinessTracker;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

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
}
