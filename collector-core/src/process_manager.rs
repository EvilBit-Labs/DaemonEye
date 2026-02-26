//! Process manager configuration types for collector processes.
//!
//! This module defines the basic configuration types used by collector process managers.
//! The full process management implementation lives in `daemoneye-eventbus`.

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

/// Resource limits for collector processes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
