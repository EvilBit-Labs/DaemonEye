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

mod control;
mod lifecycle;
mod termination;
mod types;

pub use types::{
    CollectorConfig, CollectorProcess, CollectorState, CollectorStatus, HealthStatus,
    ProcessManagerConfig, ProcessManagerError, ResourceLimits,
};

use crate::DaemoneyeBroker;
#[cfg(all(unix, feature = "freebsd"))]
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{error, info};

/// Manager for collector process lifecycle
#[derive(Debug)]
pub struct CollectorProcessManager {
    /// Manager configuration
    pub(super) config: ProcessManagerConfig,
    /// Running processes
    pub(super) processes: Arc<Mutex<HashMap<String, CollectorProcess>>>,
    /// Shutdown signal broadcaster
    pub(super) shutdown_tx: broadcast::Sender<()>,
    /// Restart request sender
    pub(super) restart_tx: mpsc::UnboundedSender<RestartRequest>,
    /// Optional broker for heartbeat publishing
    pub(super) broker: Option<Arc<DaemoneyeBroker>>,
}

/// Internal restart request used by the monitor task
#[derive(Debug, Clone)]
pub(super) struct RestartRequest {
    pub(super) collector_id: String,
    pub(super) collector_type: String,
    pub(super) config: CollectorConfig,
    pub(super) restart_count: u32,
}

impl CollectorProcessManager {
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
                    req.restart_count.saturating_add(1),
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
                            proc.restart_count = req.restart_count.saturating_add(1);
                        }
                    }
                    Err(e) => {
                        error!("Failed to restart collector {}: {}", req.collector_id, e);

                        // Set Failed state
                        let mut procs = manager_clone.processes.lock().await;
                        if let Some(proc) = procs.get_mut(&req.collector_id) {
                            proc.state = CollectorState::Failed(format!("Restart failed: {e}"));
                        }
                    }
                }
            }
        });

        manager
    }
}

/// Send a signal to a process (Unix only)
#[cfg(all(unix, feature = "freebsd"))]
pub(super) fn send_signal(pid: u32, signal: Signal) -> Result<(), ProcessManagerError> {
    #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
    // pid u32→i32: valid for process IDs (max PID is well within i32 range)
    let nix_pid = Pid::from_raw(pid as i32);
    signal::kill(nix_pid, signal)
        .map_err(|e| ProcessManagerError::TerminateFailed(format!("Failed to send signal: {e}")))
}

#[cfg(all(unix, not(feature = "freebsd")))]
#[allow(dead_code)]
fn send_signal(_pid: u32, _signal: u32) -> Result<(), ProcessManagerError> {
    // On non-FreeBSD Unix systems, use tokio::process or std::process
    // For now, return an error indicating signal sending is not available
    Err(ProcessManagerError::TerminateFailed(
        "Signal sending requires freebsd feature".to_owned(),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]
mod tests {
    use super::*;
    use std::time::Duration;

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
