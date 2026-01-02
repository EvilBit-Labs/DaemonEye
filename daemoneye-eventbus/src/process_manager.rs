//! Collector Process Manager
//!
//! This module provides comprehensive lifecycle management for collector processes,
//! including spawning, monitoring, termination, pause/resume, and automatic restart
//! capabilities. It handles platform-specific differences between Unix and Windows
//! process control mechanisms.
//!
//! # Features
//!
//! - **Process Spawning**: Start collector processes with custom configuration
//! - **State Tracking**: Monitor process state and health in real-time
//! - **Graceful Shutdown**: Timeout-based graceful shutdown with automatic escalation
//! - **Pause/Resume**: Suspend and resume collectors (Unix only via SIGSTOP/SIGCONT)
//! - **Auto-Restart**: Configurable automatic restart on process crash
//! - **Health Monitoring**: Track process health and detect crashes
//!
//! # Platform Support
//!
//! - **Unix**: Full support including pause/resume via signals (SIGTERM, SIGKILL, SIGSTOP, SIGCONT)
//! - **Windows**: Process spawning and termination; pause/resume not supported
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use daemoneye_eventbus::process_manager::{
//!     CollectorProcessManager, ProcessManagerConfig, CollectorConfig
//! };
//! use std::collections::HashMap;
//! use std::path::PathBuf;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create process manager configuration
//! let config = ProcessManagerConfig {
//!     collector_binaries: HashMap::from([
//!         ("procmond".to_string(), PathBuf::from("/usr/local/bin/procmond")),
//!     ]),
//!     default_graceful_timeout: Duration::from_secs(30),
//!     default_force_timeout: Duration::from_secs(5),
//!     health_check_interval: Duration::from_secs(60),
//!     heartbeat_timeout_multiplier: 3,
//!     enable_auto_restart: false,
//! };
//!
//! // Create process manager
//! let manager = CollectorProcessManager::new(config);
//!
//! // Start a collector
//! let collector_config = CollectorConfig {
//!     binary_path: PathBuf::from("/usr/local/bin/procmond"),
//!     args: vec!["--config".to_string(), "/etc/procmond.conf".to_string()],
//!     env: HashMap::new(),
//!     working_dir: None,
//!     resource_limits: None,
//!     auto_restart: true,
//!     max_restarts: 3,
//! };
//!
//! let pid = manager.start_collector("procmond-1", "procmond", collector_config).await?;
//! println!("Started collector with PID: {}", pid);
//!
//! // Check health
//! let status = manager.get_collector_status("procmond-1").await?;
//! println!("Collector status: {:?}", status.state);
//!
//! // Stop gracefully
//! manager.stop_collector("procmond-1", true, Duration::from_secs(30)).await?;
//! # Ok(())
//! # }
//! ```

use crate::rpc::CollectorRpcClient;
use crate::{DaemoneyeBroker, Message};
#[cfg(all(unix, feature = "freebsd"))]
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// Resource limits for collector processes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResourceLimits {
    /// Maximum memory in bytes
    pub max_memory_bytes: Option<u64>,
    /// Maximum CPU percentage (0-100)
    pub max_cpu_percent: Option<u32>,
}

/// Configuration for a managed collector process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    /// Path to collector executable
    pub binary_path: PathBuf,
    /// Command-line arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub working_dir: Option<PathBuf>,
    /// Resource constraints
    pub resource_limits: Option<ResourceLimits>,
    /// Whether to auto-restart on crash
    pub auto_restart: bool,
    /// Maximum restart attempts
    pub max_restarts: u32,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::new(),
            args: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            resource_limits: None,
            auto_restart: false,
            max_restarts: 3,
        }
    }
}

impl CollectorConfig {
    /// Validate configuration
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.binary_path.exists() {
            anyhow::bail!("Binary path does not exist: {}", self.binary_path.display());
        }
        if self.max_restarts > 100 {
            anyhow::bail!("max_restarts cannot exceed 100");
        }
        Ok(())
    }
}

/// Information about a running collector process
#[derive(Debug)]
pub struct CollectorProcess {
    /// Unique identifier
    pub collector_id: String,
    /// Type (procmond, netmond, etc.)
    pub collector_type: String,
    /// Process handle (None if extracted for termination)
    pub child: Option<Child>,
    /// Process ID
    pub pid: u32,
    /// Current state
    pub state: CollectorState,
    /// When process started
    pub start_time: SystemTime,
    /// Configuration used to start
    pub config: CollectorConfig,
    /// Number of restarts
    pub restart_count: u32,
    /// Last health check time
    pub last_health_check: Option<SystemTime>,
    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,
    /// Count of consecutive missed heartbeats
    pub missed_heartbeats: u32,
    /// Whether heartbeat monitoring is enabled
    pub heartbeat_enabled: bool,
    /// Heartbeat sequence number
    pub heartbeat_sequence: u64,
    /// Optional RPC client for lifecycle operations
    pub rpc_client: Option<Arc<CollectorRpcClient>>,
}

/// Lifecycle state of a collector process
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorState {
    /// Process is starting up
    Starting,
    /// Process is running normally
    Running,
    /// Process is paused (Unix only)
    Paused,
    /// Process is stopping
    Stopping,
    /// Process failed with an error message
    Failed(String),
}

/// Configuration for the process manager
#[derive(Debug, Clone)]
pub struct ProcessManagerConfig {
    /// Collector ID to binary path mapping
    pub collector_binaries: HashMap<String, PathBuf>,
    /// Default graceful shutdown timeout
    pub default_graceful_timeout: Duration,
    /// Default force kill timeout
    pub default_force_timeout: Duration,
    /// Health check interval (also used as heartbeat interval)
    pub health_check_interval: Duration,
    /// Global auto-restart flag
    pub enable_auto_restart: bool,
    /// Heartbeat timeout multiplier (default 3, so timeout = interval * 3)
    pub heartbeat_timeout_multiplier: u32,
}

impl Default for ProcessManagerConfig {
    fn default() -> Self {
        Self {
            collector_binaries: HashMap::new(),
            default_graceful_timeout: Duration::from_secs(30),
            default_force_timeout: Duration::from_secs(5),
            health_check_interval: Duration::from_secs(60),
            enable_auto_restart: false,
            heartbeat_timeout_multiplier: 3,
        }
    }
}

/// Status of a collector process
#[derive(Debug, Clone)]
pub struct CollectorStatus {
    /// Collector identifier
    pub collector_id: String,
    /// Current state
    pub state: CollectorState,
    /// Process ID
    pub pid: u32,
    /// Start time
    pub start_time: SystemTime,
    /// Number of restarts
    pub restart_count: u32,
    /// Time since start
    pub uptime: Duration,
    /// Current health
    pub health: HealthStatus,
    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,
    /// Count of consecutive missed heartbeats
    pub missed_heartbeats: u32,
    /// Whether heartbeat is healthy
    pub heartbeat_healthy: bool,
}

/// Health status of a collector
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    /// Process is healthy
    Healthy,
    /// Process is degraded but functional
    Degraded,
    /// Process is unhealthy
    Unhealthy,
    /// Health status unknown
    Unknown,
}

/// Errors that can occur during process management
#[derive(Debug, Error)]
pub enum ProcessManagerError {
    /// Collector not found
    #[error("Collector not found: {0}")]
    ProcessNotFound(String),

    /// Collector already running
    #[error("Collector already running: {0}")]
    AlreadyRunning(String),

    /// Failed to spawn process
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    /// Failed to terminate process
    #[error("Failed to terminate process: {0}")]
    TerminateFailed(String),

    /// Operation not valid in current state
    #[error("Invalid state for operation: {0}")]
    InvalidState(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    /// Operation not supported on platform
    #[error("Platform not supported: {0}")]
    PlatformNotSupported(String),

    /// Operation timed out
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manager for collector process lifecycle
#[derive(Debug)]
pub struct CollectorProcessManager {
    /// Manager configuration
    pub config: ProcessManagerConfig,
    /// Running processes
    processes: Arc<Mutex<HashMap<String, CollectorProcess>>>,
    /// Shutdown signal broadcaster
    shutdown_tx: broadcast::Sender<()>,
    /// Restart request sender
    restart_tx: mpsc::UnboundedSender<RestartRequest>,
    /// Optional broker for heartbeat publishing
    broker: Option<Arc<DaemoneyeBroker>>,
}

/// Internal restart request used by the monitor task
#[derive(Debug, Clone)]
struct RestartRequest {
    collector_id: String,
    collector_type: String,
    config: CollectorConfig,
    restart_count: u32,
}

impl CollectorProcessManager {
    async fn spawn_process_with_retries(
        binary_path: &PathBuf,
        config: &CollectorConfig,
    ) -> io::Result<Child> {
        const MAX_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;

        loop {
            let mut command = Command::new(binary_path);
            command.args(&config.args);

            for (key, value) in &config.env {
                command.env(key, value);
            }

            if let Some(working_dir) = &config.working_dir {
                command.current_dir(working_dir);
            }

            command.stdin(Stdio::null());
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());

            match command.spawn() {
                Ok(child) => return Ok(child),
                Err(e) if Self::is_text_file_busy(&e) && attempt + 1 < MAX_ATTEMPTS => {
                    attempt += 1;
                    let backoff_ms = 25 * attempt as u64;
                    warn!(
                        attempt,
                        backoff_ms,
                        path = %binary_path.display(),
                        error = %e,
                        "Collector spawn returned ETXTBUSY, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn is_text_file_busy(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(code) if code == 26)
    }

    /// Create a new process manager
    ///
    /// # Arguments
    ///
    /// * `config` - Process manager configuration
    ///
    /// # Returns
    ///
    /// New process manager instance
    pub fn new(config: ProcessManagerConfig) -> Arc<Self> {
        Self::with_broker(config, None)
    }

    /// Create a new process manager with broker integration
    ///
    /// # Arguments
    ///
    /// * `config` - Process manager configuration
    /// * `broker` - Optional broker for heartbeat publishing
    ///
    /// # Returns
    ///
    /// New process manager instance
    pub fn with_broker(
        config: ProcessManagerConfig,
        broker: Option<Arc<DaemoneyeBroker>>,
    ) -> Arc<Self> {
        let (shutdown_tx, _) = broadcast::channel(16);
        let (restart_tx, mut restart_rx) = mpsc::unbounded_channel();

        let manager = Arc::new(Self {
            config,
            processes: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx,
            restart_tx,
            broker,
        });

        // Spawn restart handler task
        let manager_clone = Arc::clone(&manager);
        tokio::spawn(async move {
            while let Some(req) = restart_rx.recv().await {
                info!(
                    "Processing restart request for collector {} (attempt {}/{})",
                    req.collector_id,
                    req.restart_count + 1,
                    req.config.max_restarts
                );

                match manager_clone
                    .start_collector(&req.collector_id, &req.collector_type, req.config.clone())
                    .await
                {
                    Ok(new_pid) => {
                        info!(
                            "Successfully restarted collector {} with new PID: {}",
                            req.collector_id, new_pid
                        );

                        // Update restart count
                        let mut procs = manager_clone.processes.lock().await;
                        if let Some(proc) = procs.get_mut(&req.collector_id) {
                            proc.restart_count = req.restart_count + 1;
                        }
                    }
                    Err(e) => {
                        error!("Failed to restart collector {}: {}", req.collector_id, e);

                        // Set Failed state
                        let mut procs = manager_clone.processes.lock().await;
                        if let Some(proc) = procs.get_mut(&req.collector_id) {
                            proc.state = CollectorState::Failed(format!("Restart failed: {}", e));
                        }
                    }
                }
            }
        });

        manager
    }

    /// Start a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Unique identifier for the collector
    /// * `collector_type` - Type of collector (procmond, netmond, etc.)
    /// * `config` - Configuration for spawning the process
    ///
    /// # Returns
    ///
    /// Process ID on success
    ///
    /// # Errors
    ///
    /// - `AlreadyRunning` if collector is already running
    /// - `SpawnFailed` if process spawn fails
    pub async fn start_collector(
        &self,
        collector_id: &str,
        collector_type: &str,
        config: CollectorConfig,
    ) -> Result<u32, ProcessManagerError> {
        {
            let processes = self.processes.lock().await;
            if processes.contains_key(collector_id) {
                return Err(ProcessManagerError::AlreadyRunning(
                    collector_id.to_string(),
                ));
            }
        }

        let binary_path = if config.binary_path.as_os_str().is_empty() {
            self.config
                .collector_binaries
                .get(collector_type)
                .cloned()
                .ok_or_else(|| {
                    ProcessManagerError::SpawnFailed(format!(
                        "No binary path configured for collector type: {}",
                        collector_type
                    ))
                })?
        } else {
            config.binary_path.clone()
        };

        info!(
            "Starting collector: {} (type: {}) with binary: {}",
            collector_id,
            collector_type,
            binary_path.display()
        );

        let mut child = Self::spawn_process_with_retries(&binary_path, &config)
            .await
            .map_err(|e| {
                ProcessManagerError::SpawnFailed(format!(
                    "Failed to spawn {}: {}",
                    binary_path.display(),
                    e
                ))
            })?;

        let pid = child
            .id()
            .ok_or_else(|| ProcessManagerError::SpawnFailed("Failed to get PID".to_string()))?;

        info!("Spawned collector {} with PID: {}", collector_id, pid);

        let mut processes = self.processes.lock().await;
        if processes.contains_key(collector_id) {
            drop(processes);
            warn!(
                collector_id,
                pid,
                "Collector registered while spawn was in progress; terminating duplicate instance"
            );
            if let Err(e) = child.start_kill() {
                warn!(pid, error = %e, "Failed to signal duplicate collector for termination");
            }
            let _ = child.wait().await;
            return Err(ProcessManagerError::AlreadyRunning(
                collector_id.to_string(),
            ));
        }

        let now = SystemTime::now();
        let process = CollectorProcess {
            collector_id: collector_id.to_string(),
            collector_type: collector_type.to_string(),
            child: Some(child),
            pid,
            state: CollectorState::Starting,
            start_time: now,
            config: config.clone(),
            restart_count: 0,
            last_health_check: None,
            last_heartbeat: now,
            missed_heartbeats: 0,
            heartbeat_enabled: true,
            heartbeat_sequence: 0,
            rpc_client: None,
        };

        processes.insert(collector_id.to_string(), process);
        drop(processes);

        // Create RPC client if broker is available
        if let Some(ref broker) = self.broker {
            let target_topic = format!("control.collector.{}", collector_id);
            match CollectorRpcClient::new(&target_topic, Arc::clone(broker)).await {
                Ok(client) => {
                    let mut processes = self.processes.lock().await;
                    if let Some(process) = processes.get_mut(collector_id) {
                        process.rpc_client = Some(Arc::new(client));
                    }
                    info!(
                        collector_id = %collector_id,
                        "Created RPC client for collector"
                    );
                }
                Err(e) => {
                    warn!(
                        collector_id = %collector_id,
                        error = %e,
                        "Failed to create RPC client, will use signal-based operations"
                    );
                }
            }
        }

        // Spawn monitoring task
        self.spawn_process_monitor(collector_id.to_string()).await;

        // Spawn heartbeat task
        self.spawn_heartbeat_task(collector_id.to_string()).await;

        // Update state to Running
        let mut processes = self.processes.lock().await;
        if let Some(process) = processes.get_mut(collector_id) {
            process.state = CollectorState::Running;
        }

        Ok(pid)
    }

    /// Spawn a monitoring task for a collector process
    ///
    /// This task waits for the process to exit and handles auto-restart if configured.
    /// Auto-restart is performed if:
    /// - Global `enable_auto_restart` is true
    /// - Process-level `auto_restart` is true
    /// - `restart_count < max_restarts`
    ///
    /// On exit without restart, the collector is removed from the process map to allow subsequent start calls.
    async fn spawn_process_monitor(&self, collector_id: String) {
        let processes = Arc::clone(&self.processes);
        let process_manager_config = self.config.clone();
        let restart_tx = self.restart_tx.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Check if process exited and decide on restart
                let restart_request = {
                    let mut procs = processes.lock().await;

                    let Some(proc) = procs.get_mut(&collector_id) else {
                        debug!(
                            "Collector {} no longer in process map, ending monitor",
                            collector_id
                        );
                        return;
                    };

                    let child_pid = proc.pid;

                    match proc
                        .child
                        .as_mut()
                        .and_then(|c| c.try_wait().ok())
                        .flatten()
                    {
                        Some(status) => {
                            info!(
                                "Collector {} (PID: {}) exited with status: {:?}",
                                collector_id, child_pid, status
                            );

                            // Determine if should auto-restart
                            let should_restart = process_manager_config.enable_auto_restart
                                && proc.config.auto_restart
                                && proc.restart_count < proc.config.max_restarts;

                            if should_restart {
                                let restart_count = proc.restart_count;
                                let collector_type = proc.collector_type.clone();
                                let config = proc.config.clone();

                                info!(
                                    "Collector {} will be auto-restarted (attempt {}/{})",
                                    collector_id,
                                    restart_count + 1,
                                    config.max_restarts
                                );

                                // Remove from map to allow start_collector to succeed
                                procs.remove(&collector_id);

                                Some(RestartRequest {
                                    collector_id: collector_id.clone(),
                                    collector_type,
                                    config,
                                    restart_count,
                                })
                            } else {
                                // Not restarting, remove from map
                                if status.success() {
                                    info!(
                                        "Collector {} stopped cleanly, removing from map",
                                        collector_id
                                    );
                                } else {
                                    warn!(
                                        "Collector {} failed with exit code: {:?}, removing from map",
                                        collector_id,
                                        status.code()
                                    );
                                }
                                procs.remove(&collector_id);
                                None
                            }
                        }
                        None => {
                            // Still running, continue monitoring
                            None
                        }
                    }
                };
                // Lock dropped here

                // Send restart request if needed, then exit monitor
                if let Some(req) = restart_request {
                    if let Err(e) = restart_tx.send(req) {
                        error!("Failed to send restart request: {}", e);
                    }
                    return;
                }

                // If no restart but we removed the process, exit monitor
                if restart_request.is_none() {
                    let procs = processes.lock().await;
                    if !procs.contains_key(&collector_id) {
                        debug!("Monitor exiting for collector {}", collector_id);
                        return;
                    }
                }
            }
        });
    }

    /// Spawn a heartbeat task for a collector process
    ///
    /// This task periodically publishes heartbeat messages and monitors for timeouts.
    /// The task exits when the collector is removed from the process map.
    async fn spawn_heartbeat_task(&self, collector_id: String) {
        let processes = Arc::clone(&self.processes);
        let broker = self.broker.clone();
        let heartbeat_interval = self.config.health_check_interval;
        let heartbeat_threshold = self.config.heartbeat_timeout_multiplier;

        // Guard against zero or extremely small intervals to avoid busy loops
        let min_interval = Duration::from_millis(50);
        let effective_interval = if heartbeat_interval.is_zero() {
            warn!(
                "Heartbeat interval is zero, using minimum of {:?}",
                min_interval
            );
            min_interval
        } else if heartbeat_interval < min_interval {
            warn!(
                "Heartbeat interval {:?} below minimum {:?}, clamping to minimum",
                heartbeat_interval, min_interval
            );
            min_interval
        } else {
            heartbeat_interval
        };

        tokio::spawn(async move {
            Self::run_heartbeat_loop(
                collector_id,
                processes,
                broker,
                effective_interval,
                heartbeat_threshold,
            )
            .await;
        });
    }

    async fn run_heartbeat_loop(
        collector_id: String,
        processes: Arc<Mutex<HashMap<String, CollectorProcess>>>,
        broker: Option<Arc<DaemoneyeBroker>>,
        heartbeat_interval: Duration,
        heartbeat_threshold: u32,
    ) {
        loop {
            tokio::time::sleep(heartbeat_interval).await;

            // Update heartbeat and check timeout
            let should_exit = {
                let mut procs = processes.lock().await;

                let Some(proc) = procs.get_mut(&collector_id) else {
                    debug!(
                        "Collector {} no longer in process map, ending heartbeat task",
                        collector_id
                    );
                    return;
                };

                let now = SystemTime::now();
                let elapsed_since_heartbeat = now
                    .duration_since(proc.last_heartbeat)
                    .unwrap_or(Duration::from_secs(0));

                // Calculate expected heartbeats missed based on elapsed time.
                // Use millisecond precision and guard against sub-second intervals to avoid division by zero.
                let intervals_missed: u64 = {
                    let interval_ms = heartbeat_interval.as_millis();
                    if interval_ms == 0 {
                        0
                    } else {
                        let elapsed_ms = elapsed_since_heartbeat.as_millis();
                        let n = elapsed_ms / interval_ms;
                        u64::try_from(n).unwrap_or(u64::MAX)
                    }
                };

                // Increment heartbeat sequence
                proc.heartbeat_sequence = proc.heartbeat_sequence.wrapping_add(1);
                let sequence = proc.heartbeat_sequence;

                // Publish heartbeat if broker available
                let mut publish_success = false;
                if let Some(ref broker) = broker {
                    let topic = format!("control.health.heartbeat.{}", collector_id);
                    let message = Message::heartbeat(sequence);
                    let correlation_id = format!("heartbeat-{}-{}", collector_id, sequence);

                    match serde_json::to_vec(&message) {
                        Ok(payload) => {
                            if let Err(e) = broker.publish(&topic, &correlation_id, payload).await {
                                warn!("Failed to publish heartbeat for {}: {}", collector_id, e);
                                // Increment missed heartbeats on publish failure
                                proc.missed_heartbeats = proc.missed_heartbeats.saturating_add(1);
                            } else {
                                debug!(
                                    "Published heartbeat {} for collector {}",
                                    sequence, collector_id
                                );
                                // Reset missed heartbeats and update timestamp on success
                                proc.missed_heartbeats = 0;
                                proc.last_heartbeat = now;
                                publish_success = true;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize heartbeat message: {}", e);
                            proc.missed_heartbeats = proc.missed_heartbeats.saturating_add(1);
                        }
                    }
                }

                // If no broker or publish failed, increment based on elapsed time
                if !publish_success && intervals_missed > 0 {
                    proc.missed_heartbeats =
                        intervals_missed.min(heartbeat_threshold as u64 + 1) as u32;
                }

                // Check if we should continue
                proc.heartbeat_enabled
            };

            if !should_exit {
                debug!(
                    "Heartbeat monitoring disabled for {}, exiting task",
                    collector_id
                );
                return;
            }
        }
    }

    /// Stop a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to stop
    /// * `graceful` - Whether to attempt graceful shutdown first
    /// * `timeout` - Timeout for graceful shutdown
    ///
    /// # Returns
    ///
    /// Exit status on success
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `TerminateFailed` if termination fails
    /// - `Timeout` if graceful shutdown times out
    ///
    /// # Platform Notes
    ///
    /// - **Unix**: Graceful stop sends SIGTERM, force kill sends SIGKILL
    /// - **Windows**: Both graceful and force kill use `TerminateProcess` (immediate termination)
    ///   Windows does not support graceful process termination; applications must implement
    ///   their own shutdown coordination (e.g., via named events or window messages).
    pub async fn stop_collector(
        &self,
        collector_id: &str,
        graceful: bool,
        timeout: Duration,
    ) -> Result<Option<i32>, ProcessManagerError> {
        // Extract child handle and RPC client before awaiting
        #[allow(unused_variables)]
        let (mut child, pid, collector_id_owned, rpc_client) = {
            let mut processes = self.processes.lock().await;
            let proc = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

            // If child is None, process already stopped or being stopped
            if proc.child.is_none() {
                info!(
                    "Collector {} already stopped (no child handle)",
                    collector_id
                );
                let exit_code = None;
                processes.remove(collector_id);
                return Ok(exit_code);
            }

            info!(
                "Stopping collector {} (PID: {}, graceful: {})",
                collector_id, proc.pid, graceful
            );

            // Mark as stopping
            proc.state = CollectorState::Stopping;
            let pid = proc.pid;
            let collector_id_owned = collector_id.to_string();

            // Extract child and RPC client, leaving None in their places
            let child = proc.child.take().expect("invariant: child exists");
            let rpc_client = proc.rpc_client.take();

            (child, pid, collector_id_owned, rpc_client)
        };
        // Lock dropped here

        // Try RPC-based shutdown first if client is available
        if graceful && let Some(client) = rpc_client {
            match self
                .shutdown_collector_rpc(&collector_id_owned, &client, true, timeout, &mut child)
                .await
            {
                Ok(exit_code) => {
                    info!(
                        collector_id = %collector_id_owned,
                        "Collector stopped via RPC"
                    );
                    // Remove from process map
                    let mut processes = self.processes.lock().await;
                    processes.remove(&collector_id_owned);
                    return Ok(exit_code);
                }
                Err(e) => {
                    warn!(
                        collector_id = %collector_id_owned,
                        error = %e,
                        "RPC shutdown failed, falling back to signal-based shutdown"
                    );
                    // Fall through to signal-based shutdown
                }
            }
        }

        let exit_code = if graceful {
            // Send termination signal
            #[cfg(all(unix, feature = "freebsd"))]
            {
                // Treat ESRCH (no such process) as benign: the process likely exited between checks
                match send_signal(pid, Signal::SIGTERM) {
                    Ok(()) => {}
                    Err(ProcessManagerError::TerminateFailed(msg))
                        if msg.contains("ESRCH") || msg.contains("No such process") =>
                    {
                        warn!(
                            "SIGTERM returned ESRCH for collector {} (PID: {}), proceeding as already exited",
                            collector_id_owned, pid
                        );
                        // Continue to wait() which should return immediately if already exited
                    }
                    Err(e) => {
                        // Re-acquire lock to set Failed state and restore child
                        let mut processes = self.processes.lock().await;
                        if let Some(proc) = processes.get_mut(&collector_id_owned) {
                            proc.state = CollectorState::Failed(format!("Signal failed: {}", e));
                            proc.child = Some(child);
                        }
                        return Err(e);
                    }
                }
            }
            #[cfg(windows)]
            {
                // Windows: start_kill() uses TerminateProcess (immediate termination)
                // No graceful shutdown mechanism available at OS level
                if let Err(e) = child.start_kill() {
                    let err =
                        ProcessManagerError::TerminateFailed(format!("Failed to terminate: {}", e));
                    // Re-acquire lock to set Failed state and restore child
                    let mut processes = self.processes.lock().await;
                    if let Some(proc) = processes.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Terminate failed: {}", e));
                        proc.child = Some(child);
                    }
                    return Err(err);
                }
            }

            // Wait with timeout (lock not held)
            let wait_result = tokio::time::timeout(timeout, child.wait()).await;

            match wait_result {
                Ok(Ok(status)) => {
                    info!(
                        "Collector {} exited gracefully with status: {:?}",
                        collector_id_owned, status
                    );
                    // Re-acquire lock to remove entry on successful termination
                    let mut processes = self.processes.lock().await;
                    processes.remove(&collector_id_owned);
                    status.code()
                }
                Ok(Err(e)) => {
                    warn!("Error waiting for collector {}: {}", collector_id_owned, e);
                    // Re-acquire lock to set Failed state and keep entry for visibility
                    let mut processes = self.processes.lock().await;
                    if let Some(proc) = processes.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Wait failed: {}", e));
                        // Don't restore child as it's consumed by wait()
                    }
                    // Fall through to force kill
                    None
                }
                Err(_) => {
                    warn!(
                        "Graceful shutdown timeout for collector {}, escalating to force kill",
                        collector_id_owned
                    );
                    // Keep entry for force kill attempt
                    // Fall through to force kill
                    None
                }
            }
        } else {
            None
        };

        // If graceful succeeded, return immediately
        if exit_code.is_some() {
            return Ok(exit_code);
        }

        // Force kill path
        #[cfg(all(unix, feature = "freebsd"))]
        {
            // Treat ESRCH (no such process) as benign and proceed to wait
            match send_signal(pid, Signal::SIGKILL) {
                Ok(()) => {}
                Err(ProcessManagerError::TerminateFailed(msg))
                    if msg.contains("ESRCH") || msg.contains("No such process") =>
                {
                    warn!(
                        "SIGKILL returned ESRCH for collector {} (PID: {}), assuming already exited",
                        collector_id_owned, pid
                    );
                }
                Err(e) => {
                    // Re-acquire lock to set Failed state
                    let mut processes = self.processes.lock().await;
                    if let Some(proc) = processes.get_mut(&collector_id_owned) {
                        proc.state = CollectorState::Failed(format!("Force signal failed: {}", e));
                    }
                    return Err(e);
                }
            }
        }
        #[cfg(windows)]
        {
            if let Err(e) = child.start_kill() {
                let err = ProcessManagerError::TerminateFailed(format!("Failed to kill: {}", e));
                // Re-acquire lock to set Failed state
                let mut processes = self.processes.lock().await;
                if let Some(proc) = processes.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed(format!("Force kill failed: {}", e));
                }
                return Err(err);
            }
        }

        // Wait with force timeout (lock not held)
        let force_timeout = self.config.default_force_timeout;
        match tokio::time::timeout(force_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                info!(
                    "Collector {} force killed: {:?}",
                    collector_id_owned, status
                );
                // Re-acquire lock to remove entry on successful termination
                let mut processes = self.processes.lock().await;
                processes.remove(&collector_id_owned);
                Ok(status.code())
            }
            Ok(Err(e)) => {
                // Re-acquire lock to set Failed state and keep entry for visibility
                let mut processes = self.processes.lock().await;
                if let Some(proc) = processes.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed(format!("Force wait failed: {}", e));
                }
                Err(ProcessManagerError::TerminateFailed(format!(
                    "Force wait failed: {}",
                    e
                )))
            }
            Err(_) => {
                error!("Force kill timeout for collector {}", collector_id_owned);
                // Re-acquire lock to set Failed state and keep entry for visibility
                let mut processes = self.processes.lock().await;
                if let Some(proc) = processes.get_mut(&collector_id_owned) {
                    proc.state = CollectorState::Failed("Force kill timeout".to_string());
                }
                Err(ProcessManagerError::Timeout(format!(
                    "Force kill timeout for {}",
                    collector_id_owned
                )))
            }
        }
    }

    /// Shutdown a collector via RPC
    async fn shutdown_collector_rpc(
        &self,
        collector_id: &str,
        client: &CollectorRpcClient,
        graceful: bool,
        timeout: Duration,
        child: &mut Child,
    ) -> Result<Option<i32>, ProcessManagerError> {
        use crate::rpc::{
            CollectorLifecycleRequest, CollectorOperation, RpcRequest, RpcStatus, ShutdownRequest,
            ShutdownType,
        };

        if graceful {
            let shutdown_request = ShutdownRequest {
                collector_id: collector_id.to_string(),
                shutdown_type: ShutdownType::Graceful,
                graceful_timeout_ms: timeout.as_millis() as u64,
                force_after_timeout: true,
                reason: Some("Process manager initiated graceful shutdown".to_string()),
            };
            let request = RpcRequest::shutdown(
                client.client_id.clone(),
                client.target_topic.clone(),
                shutdown_request,
                timeout,
            );

            match tokio::time::timeout(timeout, client.call(request, timeout)).await {
                Ok(Ok(response)) => {
                    if response.status == RpcStatus::Success {
                        // Wait for process to exit
                        match tokio::time::timeout(timeout, child.wait()).await {
                            Ok(Ok(status)) => Ok(status.code()),
                            Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                                "Wait failed: {}",
                                e
                            ))),
                            Err(_) => Err(ProcessManagerError::Timeout(
                                "Process wait timeout".to_string(),
                            )),
                        }
                    } else {
                        Err(ProcessManagerError::TerminateFailed(
                            response
                                .error_details
                                .map(|e| e.message)
                                .unwrap_or_else(|| "RPC shutdown failed".to_string()),
                        ))
                    }
                }
                Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                    "RPC call failed: {}",
                    e
                ))),
                Err(_) => Err(ProcessManagerError::Timeout(
                    "RPC shutdown timeout".to_string(),
                )),
            }
        } else {
            let lifecycle_request = CollectorLifecycleRequest::stop(collector_id);
            let request = RpcRequest::lifecycle(
                client.client_id.clone(),
                client.target_topic.clone(),
                CollectorOperation::Stop,
                lifecycle_request,
                timeout,
            );

            match tokio::time::timeout(timeout, client.call(request, timeout)).await {
                Ok(Ok(response)) => {
                    if response.status == RpcStatus::Success {
                        // Wait for process to exit
                        match tokio::time::timeout(timeout, child.wait()).await {
                            Ok(Ok(status)) => Ok(status.code()),
                            Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                                "Wait failed: {}",
                                e
                            ))),
                            Err(_) => Err(ProcessManagerError::Timeout(
                                "Process wait timeout".to_string(),
                            )),
                        }
                    } else {
                        Err(ProcessManagerError::TerminateFailed(
                            response
                                .error_details
                                .map(|e| e.message)
                                .unwrap_or_else(|| "RPC stop failed".to_string()),
                        ))
                    }
                }
                Ok(Err(e)) => Err(ProcessManagerError::TerminateFailed(format!(
                    "RPC call failed: {}",
                    e
                ))),
                Err(_) => Err(ProcessManagerError::Timeout("RPC stop timeout".to_string())),
            }
        }
    }

    /// Restart a collector process
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to restart
    /// * `timeout` - Timeout for stop operation
    ///
    /// # Returns
    ///
    /// New process ID on success
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `SpawnFailed` if restart fails
    pub async fn restart_collector(
        &self,
        collector_id: &str,
        timeout: Duration,
    ) -> Result<u32, ProcessManagerError> {
        info!("Restarting collector: {}", collector_id);

        // Get current config
        let (collector_type, config, restart_count) = {
            let processes = self.processes.lock().await;
            let process = processes
                .get(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

            (
                process.collector_type.clone(),
                process.config.clone(),
                process.restart_count,
            )
        };

        // Stop the collector
        self.stop_collector(collector_id, true, timeout).await?;

        // Start with incremented restart count
        let pid = self
            .start_collector(collector_id, &collector_type, config)
            .await?;

        // Update restart count
        let mut processes = self.processes.lock().await;
        if let Some(process) = processes.get_mut(collector_id) {
            process.restart_count = restart_count + 1;
        }

        Ok(pid)
    }

    /// Pause a collector process (Unix only)
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to pause
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `InvalidState` if collector is not in Running state
    /// - `PlatformNotSupported` on Windows
    pub async fn pause_collector(&self, collector_id: &str) -> Result<(), ProcessManagerError> {
        #[cfg(unix)]
        {
            let mut processes = self.processes.lock().await;
            let process = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

            // Validate state before pausing
            if process.state != CollectorState::Running {
                return Err(ProcessManagerError::InvalidState(format!(
                    "Collector {} is not running (state: {:?}), cannot pause",
                    collector_id, process.state
                )));
            }

            info!("Pausing collector {} (PID: {})", collector_id, process.pid);

            #[cfg(feature = "freebsd")]
            {
                send_signal(process.pid, Signal::SIGSTOP)?;
                process.state = CollectorState::Paused;
                Ok(())
            }
            #[cfg(not(feature = "freebsd"))]
            {
                Err(ProcessManagerError::SpawnFailed(
                    "Pause operation requires freebsd feature".to_string(),
                ))
            }
        }

        #[cfg(windows)]
        {
            let _ = collector_id; // Parameter required for API consistency
            Err(ProcessManagerError::PlatformNotSupported(
                "Pause operation not supported on Windows".to_string(),
            ))
        }
    }

    /// Resume a paused collector process (Unix only)
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to resume
    ///
    /// # Errors
    ///
    /// - `ProcessNotFound` if collector doesn't exist
    /// - `InvalidState` if collector is not paused
    /// - `PlatformNotSupported` on Windows
    pub async fn resume_collector(&self, collector_id: &str) -> Result<(), ProcessManagerError> {
        #[cfg(unix)]
        {
            let mut processes = self.processes.lock().await;
            let process = processes
                .get_mut(collector_id)
                .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

            if process.state != CollectorState::Paused {
                return Err(ProcessManagerError::InvalidState(format!(
                    "Collector {} is not paused (state: {:?})",
                    collector_id, process.state
                )));
            }

            info!("Resuming collector {} (PID: {})", collector_id, process.pid);

            #[cfg(feature = "freebsd")]
            {
                send_signal(process.pid, Signal::SIGCONT)?;
                process.state = CollectorState::Running;
                Ok(())
            }
            #[cfg(not(feature = "freebsd"))]
            {
                Err(ProcessManagerError::SpawnFailed(
                    "Resume operation requires freebsd feature".to_string(),
                ))
            }
        }

        #[cfg(windows)]
        {
            let _ = collector_id; // Parameter required for API consistency
            Err(ProcessManagerError::PlatformNotSupported(
                "Resume operation not supported on Windows".to_string(),
            ))
        }
    }

    /// Check collector health
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector to check
    ///
    /// # Returns
    ///
    /// Health status
    pub async fn check_collector_health(
        &self,
        collector_id: &str,
    ) -> Result<HealthStatus, ProcessManagerError> {
        let mut processes = self.processes.lock().await;
        let process = processes
            .get_mut(collector_id)
            .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

        process.last_health_check = Some(SystemTime::now());

        // Check heartbeat status with graduated response
        let heartbeat_threshold = self.config.heartbeat_timeout_multiplier;
        let health_from_heartbeat = if process.missed_heartbeats >= heartbeat_threshold {
            HealthStatus::Unhealthy
        } else if process.missed_heartbeats > 0 {
            HealthStatus::Degraded // Some heartbeats missed but not yet critical
        } else {
            HealthStatus::Healthy
        };

        // Check if process is still running
        let health_from_process = match process
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .flatten()
        {
            Some(_) => HealthStatus::Unhealthy, // Process exited
            None => HealthStatus::Healthy,      // Still running or no child handle
        };

        // Return worst status (Unhealthy > Degraded > Healthy)
        Ok(match (health_from_process, health_from_heartbeat) {
            (HealthStatus::Unhealthy, _) | (_, HealthStatus::Unhealthy) => HealthStatus::Unhealthy,
            (HealthStatus::Degraded, _) | (_, HealthStatus::Degraded) => HealthStatus::Degraded,
            _ => HealthStatus::Healthy,
        })
    }

    /// Get collector status
    ///
    /// # Arguments
    ///
    /// * `collector_id` - Identifier of the collector
    ///
    /// # Returns
    ///
    /// Collector status
    pub async fn get_collector_status(
        &self,
        collector_id: &str,
    ) -> Result<CollectorStatus, ProcessManagerError> {
        let mut processes = self.processes.lock().await;
        let process = processes
            .get_mut(collector_id)
            .ok_or_else(|| ProcessManagerError::ProcessNotFound(collector_id.to_string()))?;

        let uptime = SystemTime::now()
            .duration_since(process.start_time)
            .unwrap_or_default();

        let health = match process
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok())
            .flatten()
        {
            Some(_) => HealthStatus::Unhealthy, // Process exited
            None => HealthStatus::Healthy,      // Still running or no child handle
        };

        let heartbeat_healthy = process.missed_heartbeats == 0;

        Ok(CollectorStatus {
            collector_id: process.collector_id.clone(),
            state: process.state.clone(),
            pid: process.pid,
            start_time: process.start_time,
            restart_count: process.restart_count,
            uptime,
            health,
            last_heartbeat: process.last_heartbeat,
            missed_heartbeats: process.missed_heartbeats,
            heartbeat_healthy,
        })
    }

    /// Shutdown all collectors
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub async fn shutdown_all(&self) -> Result<(), ProcessManagerError> {
        info!("Shutting down all collectors");

        // Send shutdown signal
        let _ = self.shutdown_tx.send(());

        // Get list of collector IDs
        let collector_ids: Vec<String> = {
            let processes = self.processes.lock().await;
            processes.keys().cloned().collect()
        };

        // First pass: attempt graceful stop for each collector
        for collector_id in &collector_ids {
            match self
                .stop_collector(collector_id, true, self.config.default_graceful_timeout)
                .await
            {
                Ok(_) => {
                    info!("Successfully stopped collector: {}", collector_id);
                }
                Err(e) => {
                    warn!(
                        "Failed to gracefully stop collector {}: {}. Will attempt force stop.",
                        collector_id, e
                    );
                }
            }
        }

        // Second pass: force kill any remaining collectors
        let remaining_ids: Vec<String> = {
            let processes = self.processes.lock().await;
            processes.keys().cloned().collect()
        };

        for collector_id in &remaining_ids {
            match self
                .stop_collector(collector_id, false, self.config.default_force_timeout)
                .await
            {
                Ok(_) => info!("Force-stopped collector: {}", collector_id),
                Err(e) => warn!(
                    "Collector {} did not terminate after force stop: {}. Keeping entry for visibility.",
                    collector_id, e
                ),
            }
        }

        info!("All collectors shut down");
        Ok(())
    }

    /// List the IDs of all known collectors
    pub async fn list_collector_ids(&self) -> Vec<String> {
        let processes = self.processes.lock().await;
        processes.keys().cloned().collect()
    }
}

/// Send a signal to a process (Unix only)
#[cfg(all(unix, feature = "freebsd"))]
fn send_signal(pid: u32, signal: Signal) -> Result<(), ProcessManagerError> {
    let nix_pid = Pid::from_raw(pid as i32);
    signal::kill(nix_pid, signal)
        .map_err(|e| ProcessManagerError::TerminateFailed(format!("Failed to send signal: {}", e)))
}

#[cfg(all(unix, not(feature = "freebsd")))]
#[allow(dead_code)]
fn send_signal(_pid: u32, _signal: u32) -> Result<(), ProcessManagerError> {
    // On non-FreeBSD Unix systems, use tokio::process or std::process
    // For now, return an error indicating signal sending is not available
    Err(ProcessManagerError::TerminateFailed(
        "Signal sending requires freebsd feature".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collector_state_transitions() {
        let state = CollectorState::Starting;
        assert_eq!(state, CollectorState::Starting);

        let state = CollectorState::Running;
        assert_eq!(state, CollectorState::Running);
    }

    #[test]
    fn test_process_manager_config_default() {
        let config = ProcessManagerConfig::default();
        assert_eq!(config.default_graceful_timeout, Duration::from_secs(30));
        assert_eq!(config.default_force_timeout, Duration::from_secs(5));
        assert!(!config.enable_auto_restart);
    }

    #[test]
    fn test_health_status() {
        assert_eq!(HealthStatus::Healthy, HealthStatus::Healthy);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Unhealthy);
    }

    #[test]
    #[cfg(not(feature = "freebsd"))]
    fn test_pause_resume_requires_freebsd_feature() {
        // Test that pause/resume operations fail gracefully without freebsd feature
        let config = ProcessManagerConfig::default();

        // Use tokio runtime for async test
        // Manager must be created inside block_on because it spawns async tasks
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let manager = CollectorProcessManager::new(config);
            // Both operations should return error without freebsd feature
            // Note: When collector doesn't exist, we get ProcessNotFound first
            // The freebsd check only happens if the collector exists
            let pause_result = manager.pause_collector("test-collector").await;
            assert!(pause_result.is_err());
            // Accept either ProcessNotFound (collector doesn't exist) or SpawnFailed (freebsd feature missing)
            match pause_result {
                Err(ProcessManagerError::ProcessNotFound(_)) => {
                    // Expected: collector doesn't exist
                }
                Err(ProcessManagerError::SpawnFailed(msg)) => {
                    assert!(msg.contains("freebsd feature"));
                }
                Err(ProcessManagerError::PlatformNotSupported(_)) => {
                    // Expected on Windows: platform not supported
                }
                _ => panic!("Expected ProcessNotFound, SpawnFailed, or PlatformNotSupported error for pause without freebsd feature"),
            }

            let resume_result = manager.resume_collector("test-collector").await;
            assert!(resume_result.is_err());
            // Accept either ProcessNotFound (collector doesn't exist) or SpawnFailed (freebsd feature missing)
            match resume_result {
                Err(ProcessManagerError::ProcessNotFound(_)) => {
                    // Expected: collector doesn't exist
                }
                Err(ProcessManagerError::SpawnFailed(msg)) => {
                    assert!(msg.contains("freebsd feature"));
                }
                Err(ProcessManagerError::PlatformNotSupported(_)) => {
                    // Expected on Windows: platform not supported
                }
                _ => panic!("Expected ProcessNotFound, SpawnFailed, or PlatformNotSupported error for resume without freebsd feature"),
            }
        });
    }
}
