//! Configuration Manager for collector processes
//!
//! Provides validated, persistent configuration management for collector processes.
//! This module lives in the eventbus crate to avoid circular dependencies and to
//! keep lifecycle orchestration concerns alongside RPC/services.

use crate::process_manager::CollectorConfig;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Configuration snapshot for rollback
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    /// Configuration snapshot
    pub config: CollectorConfig,
    /// Timestamp when snapshot was taken
    pub timestamp: SystemTime,
    /// Configuration version number
    pub version: u64,
}

/// Configuration Manager errors
#[derive(Debug, Error)]
pub enum ConfigManagerError {
    /// Configuration not found
    #[error("Configuration not found: {0}")]
    ConfigNotFound(String),

    /// Validation failed
    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    /// Failed to persist configuration
    #[error("Persistence failed: {0}")]
    PersistenceFailed(String),

    /// Failed to rollback
    #[error("Rollback failed: {0}")]
    RollbackFailed(String),

    /// Invalid configuration change
    #[error("Invalid configuration change: {0}")]
    InvalidConfigChange(String),

    /// I/O error
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(#[from] toml::ser::Error),

    /// Deserialization error
    #[error("Deserialization error: {0}")]
    DeserializationError(#[from] toml::de::Error),

    /// JSON error
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Configuration Manager with validation, rollback, and persistence.
#[derive(Debug)]
pub struct ConfigManager {
    configs: Arc<Mutex<HashMap<String, CollectorConfig>>>,
    snapshots: Arc<Mutex<HashMap<String, ConfigSnapshot>>>,
    config_dir: PathBuf,
    version_counter: Arc<Mutex<u64>>,
}

impl ConfigManager {
    /// Create a new configuration manager
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            configs: Arc::new(Mutex::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            config_dir,
            version_counter: Arc::new(Mutex::new(0)),
        }
    }

    fn config_path(&self, collector_id: &str) -> PathBuf {
        self.config_dir.join(format!("{}.toml", collector_id))
    }

    /// Load configuration for a collector from disk (or default if missing)
    pub async fn load_config(
        &self,
        collector_id: &str,
    ) -> Result<CollectorConfig, ConfigManagerError> {
        let config_path = self.config_path(collector_id);
        let cfg = if config_path.exists() {
            let content = fs::read_to_string(&config_path).await?;
            toml::from_str::<CollectorConfig>(&content)?
        } else {
            CollectorConfig::default()
        };

        // Validate
        self.validate_config(&cfg).await?;

        // Cache
        let mut guard = self.configs.lock().await;
        guard.insert(collector_id.to_string(), cfg.clone());
        Ok(cfg)
    }

    /// Get configuration (loads from disk if not cached)
    pub async fn get_config(
        &self,
        collector_id: &str,
    ) -> Result<CollectorConfig, ConfigManagerError> {
        if let Some(cfg) = self.configs.lock().await.get(collector_id).cloned() {
            return Ok(cfg);
        }
        self.load_config(collector_id).await
    }

    /// Validate a configuration with platform-specific checks
    pub async fn validate_config(
        &self,
        config: &CollectorConfig,
    ) -> Result<(), ConfigManagerError> {
        // Binary path must exist
        if !config.binary_path.exists() {
            return Err(ConfigManagerError::ValidationFailed(format!(
                "Binary path does not exist: {}",
                config.binary_path.display()
            )));
        }

        // Executable checks
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&config.binary_path).await.map_err(|e| {
                ConfigManagerError::ValidationFailed(format!(
                    "Failed to check binary permissions: {}",
                    e
                ))
            })?;
            let mode = metadata.permissions().mode();
            if (mode & 0o111) == 0 {
                return Err(ConfigManagerError::ValidationFailed(format!(
                    "Binary is not executable: {}",
                    config.binary_path.display()
                )));
            }
        }
        #[cfg(windows)]
        {
            if let Some(ext) = config.binary_path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if !["exe", "bat", "cmd", "ps1"].contains(&ext_lower.as_str()) {
                    return Err(ConfigManagerError::ValidationFailed(format!(
                        "Binary does not have executable extension (.exe, .bat, .cmd, .ps1): {}",
                        config.binary_path.display()
                    )));
                }
            } else {
                return Err(ConfigManagerError::ValidationFailed(format!(
                    "Binary has no file extension: {}",
                    config.binary_path.display()
                )));
            }
        }

        // Resource limits sanity checks
        if let Some(limits) = &config.resource_limits {
            if let Some(max_memory) = limits.max_memory_bytes {
                const MIN_MEMORY_BYTES: u64 = 1024 * 1024; // 1MB
                const MAX_MEMORY_BYTES: u64 = 1024 * 1024 * 1024 * 1024; // 1TB
                if max_memory == 0 {
                    return Err(ConfigManagerError::ValidationFailed(
                        "max_memory_bytes cannot be zero".to_string(),
                    ));
                }
                if max_memory < MIN_MEMORY_BYTES {
                    return Err(ConfigManagerError::ValidationFailed(format!(
                        "max_memory_bytes ({}) is below minimum ({})",
                        max_memory, MIN_MEMORY_BYTES
                    )));
                }
                if max_memory > MAX_MEMORY_BYTES {
                    return Err(ConfigManagerError::ValidationFailed(format!(
                        "max_memory_bytes ({}) exceeds maximum ({})",
                        max_memory, MAX_MEMORY_BYTES
                    )));
                }
            }

            if let Some(max_cpu) = limits.max_cpu_percent
                && (max_cpu == 0 || max_cpu > 100)
            {
                return Err(ConfigManagerError::ValidationFailed(
                    "max_cpu_percent must be 1..=100".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Update configuration (optionally validate-only) and persist changes
    pub async fn update_config(
        &self,
        collector_id: &str,
        config_changes: HashMap<String, JsonValue>,
        validate_only: bool,
        rollback_on_failure: bool,
    ) -> Result<ConfigSnapshot, ConfigManagerError> {
        let current = self.get_config(collector_id).await?;
        let mut new_config = current.clone();

        // Apply changes
        self.apply_changes(&mut new_config, &config_changes)?;

        // Validate
        if let Err(e) = self.validate_config(&new_config).await {
            if rollback_on_failure {
                let _ = self.rollback_config(collector_id).await; // best effort
            }
            return Err(e);
        }

        if validate_only {
            // For validate-only, return snapshot with current version
            let version = *self.version_counter.lock().await;
            let snapshot = ConfigSnapshot {
                config: new_config.clone(),
                timestamp: SystemTime::now(),
                version,
            };
            return Ok(snapshot);
        }

        // Persist
        self.persist_config(collector_id, &new_config).await?;

        // Increment version and create snapshot with new version
        let new_version = {
            let mut vc = self.version_counter.lock().await;
            *vc = vc.saturating_add(1);
            *vc
        };

        // Update caches
        {
            let mut cfgs = self.configs.lock().await;
            cfgs.insert(collector_id.to_string(), new_config.clone());
        }
        {
            let mut snaps = self.snapshots.lock().await;
            snaps.insert(
                collector_id.to_string(),
                ConfigSnapshot {
                    config: current,
                    timestamp: SystemTime::now(),
                    version: new_version.saturating_sub(1), // Previous version
                },
            );
        }

        Ok(ConfigSnapshot {
            config: new_config,
            timestamp: SystemTime::now(),
            version: new_version,
        })
    }

    /// Rollback configuration to previous snapshot
    pub async fn rollback_config(
        &self,
        collector_id: &str,
    ) -> Result<CollectorConfig, ConfigManagerError> {
        let snapshot = {
            let mut snaps = self.snapshots.lock().await;
            snaps.remove(collector_id)
        };

        let Some(prev) = snapshot else {
            return Err(ConfigManagerError::RollbackFailed(format!(
                "No snapshot available for {}",
                collector_id
            )));
        };

        self.persist_config(collector_id, &prev.config).await?;
        let mut cfgs = self.configs.lock().await;
        cfgs.insert(collector_id.to_string(), prev.config.clone());
        Ok(prev.config)
    }

    /// Persist configuration to disk (atomic replace)
    async fn persist_config(
        &self,
        collector_id: &str,
        config: &CollectorConfig,
    ) -> Result<(), ConfigManagerError> {
        let config_path = self.config_path(collector_id);
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let temp_path = temp_path_for(&config_path);
        let content = toml::to_string_pretty(config)?;
        fs::write(&temp_path, content).await?;

        #[cfg(windows)]
        {
            if config_path.exists() {
                fs::remove_file(&config_path).await.map_err(|e| {
                    ConfigManagerError::PersistenceFailed(format!(
                        "Failed to remove existing config file: {}",
                        e
                    ))
                })?;
            }
        }

        fs::rename(&temp_path, &config_path).await.map_err(|e| {
            ConfigManagerError::PersistenceFailed(format!("Failed to rename temp file: {}", e))
        })?;

        debug!(
            file = %config_path.display(),
            "Persisted configuration"
        );
        Ok(())
    }

    /// Check if configuration change requires restart
    pub fn requires_restart(old: &CollectorConfig, new: &CollectorConfig) -> bool {
        old.binary_path != new.binary_path
            || old.args != new.args
            || old.env != new.env
            || old.working_dir != new.working_dir
    }

    /// Get list of changed fields between two configs
    pub fn get_changed_fields(old: &CollectorConfig, new: &CollectorConfig) -> Vec<String> {
        let mut fields = Vec::new();
        if old.binary_path != new.binary_path {
            fields.push("binary_path".to_string());
        }
        if old.args != new.args {
            fields.push("args".to_string());
        }
        if old.env != new.env {
            fields.push("env".to_string());
        }
        if old.working_dir != new.working_dir {
            fields.push("working_dir".to_string());
        }
        if old.resource_limits != new.resource_limits {
            fields.push("resource_limits".to_string());
        }
        if old.auto_restart != new.auto_restart {
            fields.push("auto_restart".to_string());
        }
        if old.max_restarts != new.max_restarts {
            fields.push("max_restarts".to_string());
        }
        fields
    }

    /// List all configurations (by scanning directory)
    pub async fn list_configs(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(mut rd) = fs::read_dir(&self.config_dir).await else {
            return out;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("toml")
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                out.push(stem.to_string());
            }
        }
        out
    }

    /// Delete configuration file (and cache)
    pub async fn delete_config(&self, collector_id: &str) -> Result<(), ConfigManagerError> {
        let path = self.config_path(collector_id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        self.configs.lock().await.remove(collector_id);
        self.snapshots.lock().await.remove(collector_id);
        Ok(())
    }

    fn apply_changes(
        &self,
        cfg: &mut CollectorConfig,
        changes: &HashMap<String, JsonValue>,
    ) -> Result<(), ConfigManagerError> {
        for (k, v) in changes {
            match k.as_str() {
                "binary_path" => {
                    let s = v.as_str().ok_or_else(|| {
                        ConfigManagerError::InvalidConfigChange(
                            "binary_path must be string".to_string(),
                        )
                    })?;
                    cfg.binary_path = PathBuf::from(s);
                }
                "args" => {
                    let arr = v.as_array().ok_or_else(|| {
                        ConfigManagerError::InvalidConfigChange("args must be array".to_string())
                    })?;
                    cfg.args = arr
                        .iter()
                        .map(|x| x.as_str().unwrap_or("").to_string())
                        .collect();
                }
                "env" => {
                    let obj = v.as_object().ok_or_else(|| {
                        ConfigManagerError::InvalidConfigChange("env must be object".to_string())
                    })?;
                    let mut env = HashMap::new();
                    for (ek, ev) in obj {
                        env.insert(ek.clone(), ev.as_str().unwrap_or("").to_string());
                    }
                    cfg.env = env;
                }
                "working_dir" => {
                    if v.is_null() {
                        cfg.working_dir = None;
                    } else {
                        let s = v.as_str().ok_or_else(|| {
                            ConfigManagerError::InvalidConfigChange(
                                "working_dir must be string or null".to_string(),
                            )
                        })?;
                        cfg.working_dir = Some(PathBuf::from(s));
                    }
                }
                "resource_limits" => {
                    let obj = v.as_object().ok_or_else(|| {
                        ConfigManagerError::InvalidConfigChange(
                            "resource_limits must be object".to_string(),
                        )
                    })?;
                    let mut limits = cfg.resource_limits.clone().unwrap_or_default();
                    if let Some(x) = obj.get("max_memory_bytes") {
                        limits.max_memory_bytes = x.as_u64();
                    }
                    if let Some(x) = obj.get("max_cpu_percent") {
                        limits.max_cpu_percent = x.as_u64().and_then(|u| u32::try_from(u).ok());
                    }
                    cfg.resource_limits = Some(limits);
                }
                "auto_restart" => {
                    cfg.auto_restart = v.as_bool().ok_or_else(|| {
                        ConfigManagerError::InvalidConfigChange(
                            "auto_restart must be bool".to_string(),
                        )
                    })?;
                }
                "max_restarts" => {
                    cfg.max_restarts =
                        v.as_u64()
                            .and_then(|u| u32::try_from(u).ok())
                            .ok_or_else(|| {
                                ConfigManagerError::InvalidConfigChange(
                                    "max_restarts must be u32".to_string(),
                                )
                            })?;
                }
                other => {
                    warn!(field = %other, "Unknown config field in update; ignoring");
                }
            }
        }
        Ok(())
    }
}

fn temp_path_for(config_path: &Path) -> PathBuf {
    let mut tmp = config_path.to_path_buf();
    if let Some(fname) = tmp.file_name().map(|s| s.to_owned()) {
        let tmpname = format!("{}.tmp", fname.to_string_lossy());
        tmp.set_file_name(tmpname);
    } else {
        tmp.push("config.tmp");
    }
    tmp
}
