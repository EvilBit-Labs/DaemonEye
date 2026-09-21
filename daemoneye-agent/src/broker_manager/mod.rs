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
use daemoneye_lib::detection::DetectionEngine;
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
    /// Spawn-token and schema-catalog gate, sharing the process manager's token store (R8).
    ///
    /// `None` only when the token directory could not be created, and in that case the process
    /// manager is left without a store too, so the two can never disagree about whether
    /// registration is authenticated.
    collector_admission: Option<Arc<crate::collector_admission::CollectorAdmission>>,
    /// The one detection engine: catalog, rule health, compiled plans and the pushed-task ledger.
    ///
    /// Built here, beside the spawn-token store and for the same reason — it has exactly two
    /// users, the admission gate that feeds it registrations and the agent loop that runs its
    /// clock, and a second instance would leave the planner reading a catalog nothing ever fills.
    detection_engine: Arc<Mutex<DetectionEngine>>,
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

        // One store, shared by the side that mints tokens and the side that verifies them. Built
        // here rather than at either use site so there is exactly one of it; `start` asserts the
        // sharing held, because a mis-wired authorizer fails silently — every collector simply
        // stops registering — which is the defect class described in
        // `docs/solutions/security-issues/binary-hashing-authorization-and-toctou-fixes.md`.
        let spawn_tokens = spawn_token_store(&config.socket_path);
        let process_manager =
            CollectorProcessManager::with_spawn_tokens(pm_config, None, spawn_tokens.clone());
        let detection_engine = Arc::new(Mutex::new(DetectionEngine::new()));
        let collector_admission = spawn_tokens.map(|store| {
            Arc::new(crate::collector_admission::CollectorAdmission::new(
                store,
                Arc::clone(&detection_engine),
            ))
        });

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
            collector_admission,
            detection_engine,
            rpc_clients: Arc::new(RwLock::new(std::collections::HashMap::new())),
            agent_state: Arc::new(RwLock::new(AgentState::Loading)),
            collectors_config: Arc::new(RwLock::new(CollectorsConfig::default())),
            readiness_tracker: Arc::new(RwLock::new(CollectorReadinessTracker::empty())),
        }
    }
}

impl BrokerManager {
    /// The detection engine the admission gate feeds and the agent's renewal loop drives.
    #[must_use]
    pub const fn detection_engine(&self) -> &Arc<Mutex<DetectionEngine>> {
        &self.detection_engine
    }

    /// The gate admitted registrations pass through, when registration is authenticated at all.
    #[must_use]
    pub const fn collector_admission(
        &self,
    ) -> Option<&Arc<crate::collector_admission::CollectorAdmission>> {
        self.collector_admission.as_ref()
    }

    /// The spawn-token store this manager mints into and verifies against (R9).
    ///
    /// `None` when the token directory could not be opened, in which case registration is not
    /// authenticated at all.
    // Read path used by integration tests and by future operator tooling.
    #[allow(dead_code)]
    #[must_use]
    pub fn spawn_token_store(
        &self,
    ) -> Option<&Arc<daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore>> {
        self.process_manager.spawn_token_store()
    }
}

/// Open the spawn-token store beside the broker socket.
///
/// A failure here is logged and yields `None`: the agent still starts, but with **no** collector
/// authenticated rather than with collectors authenticated against a store only one half of the
/// system can see.
fn spawn_token_store(
    socket_path: &str,
) -> Option<Arc<daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore>> {
    use daemoneye_eventbus::process_manager::spawn_token::SpawnTokenStore;

    let directory = std::path::Path::new(socket_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    match SpawnTokenStore::new(directory) {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            tracing::error!(
                error = %error,
                "Failed to open the spawn-token store; collector registration will not authenticate"
            );
            None
        }
    }
}
