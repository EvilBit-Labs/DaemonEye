//! Data types for collector process management.
//!
//! This module defines the configuration, state, status, and error types used by
//! [`CollectorProcessManager`](super::CollectorProcessManager).

use crate::rpc::CollectorRpcClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::process::Child;

/// Resource limits for collector processes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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
#[non_exhaustive]
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
            health_check_interval: Duration::from_mins(1),
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
#[non_exhaustive]
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
#[non_exhaustive]
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
