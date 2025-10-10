//! Configuration management system for collector lifecycle management.
//!
//! This module provides dynamic configuration management capabilities including
//! hot-reload, validation, rollback, and change notification for collector
//! components managed through the busrt message broker.

use crate::busrt_types::{
    BusrtError, CollectorConfig, ConfigValidationError, GetConfigRequest, GetConfigResponse,
    UpdateConfigRequest, UpdateConfigResponse, ValidateConfigRequest, ValidateConfigResponse,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::SystemTime,
};
use tokio::{
    fs,
    sync::{Mutex, broadcast},
    task::JoinHandle,
    time::{Duration, interval},
};
use tracing::{debug, info, instrument, warn};

/// Configuration management system
pub struct ConfigManager {
    /// Configuration store
    config_store: Arc<RwLock<HashMap<String, StoredConfig>>>,
    /// Configuration change broadcaster
    change_events: broadcast::Sender<ConfigChangeEvent>,
    /// File watcher handle
    file_watcher_handle: Option<JoinHandle<()>>,
    /// Configuration validation rules
    validation_rules: Arc<RwLock<HashMap<String, ValidationRule>>>,
    /// Configuration backup store for rollback
    backup_store: Arc<Mutex<HashMap<String, Vec<ConfigBackup>>>>,
    /// Configuration manager settings
    settings: ConfigManagerSettings,
}

/// Configuration manager settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigManagerSettings {
    /// Enable file watching for configuration changes
    pub enable_file_watching: bool,
    /// Configuration file directory
    pub config_directory: PathBuf,
    /// Maximum number of backups to keep per collector
    pub max_backups_per_collector: usize,
    /// Configuration validation timeout
    pub validation_timeout: Duration,
    /// Enable automatic rollback on validation failure
    pub enable_auto_rollback: bool,
    /// File watch polling interval
    pub file_watch_interval: Duration,
}

impl Default for ConfigManagerSettings {
    fn default() -> Self {
        Self {
            enable_file_watching: true,
            config_directory: PathBuf::from("/etc/daemoneye/collectors"),
            max_backups_per_collector: 10,
            validation_timeout: Duration::from_secs(30),
            enable_auto_rollback: true,
            file_watch_interval: Duration::from_secs(5),
        }
    }
}

/// Stored configuration with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredConfig {
    /// Collector configuration
    config: CollectorConfig,
    /// Configuration version
    version: u64,
    /// Last update timestamp
    last_updated: SystemTime,
    /// Configuration source (file, api, etc.)
    source: ConfigSource,
    /// Configuration checksum for integrity
    checksum: String,
    /// Validation status
    validation_status: ValidationStatus,
}

/// Configuration source tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ConfigSource {
    File(PathBuf),
    Api,
    Default,
    Backup,
}

/// Configuration validation status
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ValidationStatus {
    Valid,
    Invalid(Vec<ConfigValidationError>),
    Pending,
    Unknown,
}

/// Configuration change event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigChangeEvent {
    /// Configuration updated
    ConfigUpdated {
        collector_id: String,
        old_version: u64,
        new_version: u64,
        timestamp: SystemTime,
    },
    /// Configuration validation failed
    ValidationFailed {
        collector_id: String,
        errors: Vec<ConfigValidationError>,
        timestamp: SystemTime,
    },
    /// Configuration rolled back
    ConfigRolledBack {
        collector_id: String,
        from_version: u64,
        to_version: u64,
        reason: String,
        timestamp: SystemTime,
    },
    /// File configuration changed
    FileConfigChanged {
        collector_id: String,
        file_path: PathBuf,
        timestamp: SystemTime,
    },
}

/// Configuration backup for rollback
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigBackup {
    /// Backup timestamp
    timestamp: SystemTime,
    /// Configuration version
    version: u64,
    /// Backed up configuration
    config: CollectorConfig,
    /// Backup reason
    reason: String,
}

/// Configuration validation rule
#[derive(Debug, Clone)]
#[allow(dead_code)] // API design - fields used in future validation features
struct ValidationRule {
    /// Rule name
    name: String,
    /// Validation function
    validator: fn(&CollectorConfig) -> Result<(), Vec<ConfigValidationError>>,
    /// Rule description
    description: String,
}

impl ConfigManager {
    /// Create a new configuration manager
    pub fn new(settings: ConfigManagerSettings) -> Self {
        let (change_events, _) = broadcast::channel(1000);

        Self {
            config_store: Arc::new(RwLock::new(HashMap::new())),
            change_events,
            file_watcher_handle: None,
            validation_rules: Arc::new(RwLock::new(HashMap::new())),
            backup_store: Arc::new(Mutex::new(HashMap::new())),
            settings,
        }
    }

    /// Start the configuration manager
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting configuration manager");

        // Create config directory if it doesn't exist
        if !self.settings.config_directory.exists() {
            fs::create_dir_all(&self.settings.config_directory).await?;
            info!(
                directory = %self.settings.config_directory.display(),
                "Created configuration directory"
            );
        }

        // Register default validation rules
        self.register_default_validation_rules().await?;

        // Start file watcher if enabled
        if self.settings.enable_file_watching {
            self.start_file_watcher().await?;
        }

        info!("Configuration manager started");
        Ok(())
    }

    /// Stop the configuration manager
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping configuration manager");

        // Stop file watcher
        if let Some(handle) = self.file_watcher_handle.take() {
            handle.abort();
            // JoinHandle<()> returns Result<(), JoinError> on await
            if let Err(e) = handle.await {
                if !e.is_cancelled() {
                    warn!(error = %e, "File watcher task join error");
                }
            }
        }

        info!("Configuration manager stopped");
        Ok(())
    }

    /// Update collector configuration
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    pub async fn update_config(
        &self,
        request: UpdateConfigRequest,
    ) -> Result<UpdateConfigResponse, BusrtError> {
        info!(
            collector_id = %request.collector_id,
            validate_only = request.validate_only,
            "Updating collector configuration"
        );

        // Validate configuration
        let validation_result = self.validate_config_internal(&request.new_config).await?;

        if !validation_result.valid {
            return Ok(UpdateConfigResponse {
                success: false,
                error_message: Some("Configuration validation failed".to_string()),
                validation_errors: validation_result.validation_errors,
            });
        }

        if request.validate_only {
            return Ok(UpdateConfigResponse {
                success: true,
                error_message: None,
                validation_errors: Vec::new(),
            });
        }

        // Create backup of current configuration
        self.create_config_backup(&request.collector_id, "Pre-update backup".to_string())
            .await?;

        // Calculate configuration checksum
        let config_json = serde_json::to_string(&request.new_config).map_err(|e| {
            BusrtError::SerializationFailed(format!("Config serialization failed: {}", e))
        })?;
        let checksum = format!("{:x}", md5::compute(&config_json));

        // Update configuration
        let (old_version, new_version) = {
            let mut store = self.config_store.write().unwrap();
            let old_version = store
                .get(&request.collector_id)
                .map(|c| c.version)
                .unwrap_or(0);
            let new_version = old_version + 1;

            let stored_config = StoredConfig {
                config: request.new_config.clone(),
                version: new_version,
                last_updated: SystemTime::now(),
                source: ConfigSource::Api,
                checksum,
                validation_status: ValidationStatus::Valid,
            };

            store.insert(request.collector_id.clone(), stored_config);
            (old_version, new_version)
        };

        // Send configuration change event
        let event = ConfigChangeEvent::ConfigUpdated {
            collector_id: request.collector_id.clone(),
            old_version,
            new_version,
            timestamp: SystemTime::now(),
        };
        let _ = self.change_events.send(event);

        // Write configuration to file if file watching is enabled
        if self.settings.enable_file_watching {
            self.write_config_to_file(&request.collector_id, &request.new_config)
                .await?;
        }

        info!(
            collector_id = %request.collector_id,
            old_version = old_version,
            new_version = new_version,
            "Configuration updated successfully"
        );

        Ok(UpdateConfigResponse {
            success: true,
            error_message: None,
            validation_errors: Vec::new(),
        })
    }

    /// Get collector configuration
    #[instrument(skip(self), fields(collector_id = %request.collector_id))]
    pub async fn get_config(
        &self,
        request: GetConfigRequest,
    ) -> Result<GetConfigResponse, BusrtError> {
        let store = self.config_store.read().unwrap();
        let stored_config = store.get(&request.collector_id).ok_or_else(|| {
            BusrtError::TopicNotFound(format!(
                "Config not found for collector: {}",
                request.collector_id
            ))
        })?;

        Ok(GetConfigResponse {
            config: stored_config.config.clone(),
        })
    }

    /// Validate collector configuration
    #[instrument(skip(self))]
    pub async fn validate_config(
        &self,
        request: ValidateConfigRequest,
    ) -> Result<ValidateConfigResponse, BusrtError> {
        self.validate_config_internal(&request.config).await
    }

    /// Rollback configuration to previous version
    pub async fn rollback_config(
        &self,
        collector_id: &str,
        target_version: Option<u64>,
    ) -> Result<(), BusrtError> {
        info!(
            collector_id = %collector_id,
            target_version = ?target_version,
            "Rolling back configuration"
        );

        let backup_config = {
            let backup_store = self.backup_store.lock().await;
            let backups = backup_store.get(collector_id).ok_or_else(|| {
                BusrtError::TopicNotFound(format!(
                    "No backups found for collector: {}",
                    collector_id
                ))
            })?;

            if let Some(version) = target_version {
                backups
                    .iter()
                    .find(|b| b.version == version)
                    .ok_or_else(|| {
                        BusrtError::TopicNotFound(format!(
                            "Backup version {} not found for collector: {}",
                            version, collector_id
                        ))
                    })?
                    .clone()
            } else {
                // Get most recent backup
                backups
                    .iter()
                    .max_by_key(|b| b.timestamp)
                    .ok_or_else(|| {
                        BusrtError::TopicNotFound(format!(
                            "No backups available for collector: {}",
                            collector_id
                        ))
                    })?
                    .clone()
            }
        };

        // Update configuration with backup
        let (from_version, to_version) = {
            let mut store = self.config_store.write().unwrap();
            let from_version = store.get(collector_id).map(|c| c.version).unwrap_or(0);

            let config_json = serde_json::to_string(&backup_config.config).map_err(|e| {
                BusrtError::SerializationFailed(format!("Config serialization failed: {}", e))
            })?;
            let checksum = format!("{:x}", md5::compute(&config_json));

            let stored_config = StoredConfig {
                config: backup_config.config.clone(),
                version: backup_config.version,
                last_updated: SystemTime::now(),
                source: ConfigSource::Backup,
                checksum,
                validation_status: ValidationStatus::Valid,
            };

            store.insert(collector_id.to_string(), stored_config);
            (from_version, backup_config.version)
        };

        // Persist the rolled-back configuration to disk
        if self.settings.enable_file_watching {
            self.write_config_to_file(collector_id, &backup_config.config)
                .await?;
        }

        // Send rollback event
        let event = ConfigChangeEvent::ConfigRolledBack {
            collector_id: collector_id.to_string(),
            from_version,
            to_version,
            reason: "Manual rollback".to_string(),
            timestamp: SystemTime::now(),
        };
        let _ = self.change_events.send(event);

        info!(
            collector_id = %collector_id,
            from_version = from_version,
            to_version = to_version,
            "Configuration rolled back successfully"
        );

        Ok(())
    }

    /// Subscribe to configuration change events
    pub fn subscribe_to_config_events(&self) -> broadcast::Receiver<ConfigChangeEvent> {
        self.change_events.subscribe()
    }

    /// Load configuration from file
    pub async fn load_config_from_file(
        &self,
        collector_id: &str,
    ) -> Result<CollectorConfig, BusrtError> {
        let config_path = self.get_config_file_path(collector_id);

        if !config_path.exists() {
            return Err(BusrtError::TopicNotFound(format!(
                "Configuration file not found: {}",
                config_path.display()
            )));
        }

        let config_content = fs::read_to_string(&config_path).await.map_err(|e| {
            BusrtError::SerializationFailed(format!("Failed to read config file: {}", e))
        })?;

        let config: CollectorConfig = serde_json::from_str(&config_content).map_err(|e| {
            BusrtError::SerializationFailed(format!("Failed to parse config file: {}", e))
        })?;

        Ok(config)
    }

    /// Internal configuration validation
    async fn validate_config_internal(
        &self,
        config: &CollectorConfig,
    ) -> Result<ValidateConfigResponse, BusrtError> {
        let mut validation_errors = Vec::new();

        // Run all validation rules
        let rules = self.validation_rules.read().unwrap();
        for rule in rules.values() {
            if let Err(mut errors) = (rule.validator)(config) {
                validation_errors.append(&mut errors);
            }
        }

        let valid = validation_errors.is_empty();

        Ok(ValidateConfigResponse {
            valid,
            validation_errors,
        })
    }

    /// Create configuration backup
    async fn create_config_backup(
        &self,
        collector_id: &str,
        reason: String,
    ) -> Result<(), BusrtError> {
        let current_config = {
            let store = self.config_store.read().unwrap();
            store.get(collector_id).cloned()
        };

        if let Some(stored_config) = current_config {
            let backup = ConfigBackup {
                timestamp: SystemTime::now(),
                version: stored_config.version,
                config: stored_config.config,
                reason,
            };

            let mut backup_store = self.backup_store.lock().await;
            let backups = backup_store
                .entry(collector_id.to_string())
                .or_insert_with(Vec::new);

            backups.push(backup);

            // Maintain backup limit
            if backups.len() > self.settings.max_backups_per_collector {
                backups.remove(0);
            }

            debug!(
                collector_id = %collector_id,
                backup_count = backups.len(),
                "Configuration backup created"
            );
        }

        Ok(())
    }

    /// Write configuration to file
    async fn write_config_to_file(
        &self,
        collector_id: &str,
        config: &CollectorConfig,
    ) -> Result<(), BusrtError> {
        let config_path = self.get_config_file_path(collector_id);

        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                BusrtError::SerializationFailed(format!("Failed to create config directory: {}", e))
            })?;
        }

        let config_json = serde_json::to_string_pretty(config).map_err(|e| {
            BusrtError::SerializationFailed(format!("Config serialization failed: {}", e))
        })?;

        fs::write(&config_path, config_json).await.map_err(|e| {
            BusrtError::SerializationFailed(format!("Failed to write config file: {}", e))
        })?;

        debug!(
            collector_id = %collector_id,
            config_path = %config_path.display(),
            "Configuration written to file"
        );

        Ok(())
    }

    /// Get configuration file path for collector
    fn get_config_file_path(&self, collector_id: &str) -> PathBuf {
        self.settings
            .config_directory
            .join(format!("{}.json", collector_id))
    }

    /// Register default validation rules
    async fn register_default_validation_rules(&self) -> Result<()> {
        let mut rules = self.validation_rules.write().unwrap();

        // Collector type validation
        rules.insert(
            "collector_type".to_string(),
            ValidationRule {
                name: "collector_type".to_string(),
                validator: |config| {
                    if config.collector_type.is_empty() {
                        Err(vec![ConfigValidationError {
                            field: "collector_type".to_string(),
                            message: "Collector type cannot be empty".to_string(),
                            error_code: "EMPTY_COLLECTOR_TYPE".to_string(),
                        }])
                    } else {
                        Ok(())
                    }
                },
                description: "Validates collector type is not empty".to_string(),
            },
        );

        // Scan interval validation
        rules.insert(
            "scan_interval".to_string(),
            ValidationRule {
                name: "scan_interval".to_string(),
                validator: |config| {
                    if config.scan_interval_ms == 0 {
                        Err(vec![ConfigValidationError {
                            field: "scan_interval_ms".to_string(),
                            message: "Scan interval must be greater than 0".to_string(),
                            error_code: "INVALID_SCAN_INTERVAL".to_string(),
                        }])
                    } else if config.scan_interval_ms < 100 {
                        Err(vec![ConfigValidationError {
                            field: "scan_interval_ms".to_string(),
                            message: "Scan interval must be at least 100ms".to_string(),
                            error_code: "SCAN_INTERVAL_TOO_LOW".to_string(),
                        }])
                    } else {
                        Ok(())
                    }
                },
                description: "Validates scan interval is reasonable".to_string(),
            },
        );

        // Batch size validation
        rules.insert(
            "batch_size".to_string(),
            ValidationRule {
                name: "batch_size".to_string(),
                validator: |config| {
                    if config.batch_size == 0 {
                        Err(vec![ConfigValidationError {
                            field: "batch_size".to_string(),
                            message: "Batch size must be greater than 0".to_string(),
                            error_code: "INVALID_BATCH_SIZE".to_string(),
                        }])
                    } else if config.batch_size > 10000 {
                        Err(vec![ConfigValidationError {
                            field: "batch_size".to_string(),
                            message: "Batch size must not exceed 10000".to_string(),
                            error_code: "BATCH_SIZE_TOO_HIGH".to_string(),
                        }])
                    } else {
                        Ok(())
                    }
                },
                description: "Validates batch size is within reasonable limits".to_string(),
            },
        );

        // Timeout validation
        rules.insert(
            "timeout".to_string(),
            ValidationRule {
                name: "timeout".to_string(),
                validator: |config| {
                    if config.timeout_seconds == 0 {
                        Err(vec![ConfigValidationError {
                            field: "timeout_seconds".to_string(),
                            message: "Timeout must be greater than 0".to_string(),
                            error_code: "INVALID_TIMEOUT".to_string(),
                        }])
                    } else if config.timeout_seconds > 3600 {
                        Err(vec![ConfigValidationError {
                            field: "timeout_seconds".to_string(),
                            message: "Timeout must not exceed 1 hour".to_string(),
                            error_code: "TIMEOUT_TOO_HIGH".to_string(),
                        }])
                    } else {
                        Ok(())
                    }
                },
                description: "Validates timeout is within reasonable limits".to_string(),
            },
        );

        info!("Default validation rules registered");
        Ok(())
    }

    /// Start file watcher for configuration changes
    async fn start_file_watcher(&mut self) -> Result<()> {
        let config_directory = self.settings.config_directory.clone();
        let change_events = self.change_events.clone();
        let file_watch_interval = self.settings.file_watch_interval;

        let handle = tokio::spawn(async move {
            let mut interval = interval(file_watch_interval);
            let mut file_timestamps: HashMap<PathBuf, SystemTime> = HashMap::new();

            loop {
                interval.tick().await;

                // Check for configuration file changes
                if let Ok(entries) = fs::read_dir(&config_directory).await {
                    let mut entries = entries;
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("json") {
                            if let Ok(metadata) = entry.metadata().await {
                                if let Ok(modified) = metadata.modified() {
                                    let last_known = file_timestamps.get(&path).copied();

                                    if last_known.is_none_or(|last| modified > last) {
                                        file_timestamps.insert(path.clone(), modified);

                                        // Extract collector ID from filename
                                        if let Some(collector_id) =
                                            path.file_stem().and_then(|s| s.to_str())
                                        {
                                            let event = ConfigChangeEvent::FileConfigChanged {
                                                collector_id: collector_id.to_string(),
                                                file_path: path.clone(),
                                                timestamp: SystemTime::now(),
                                            };
                                            let _ = change_events.send(event);

                                            debug!(
                                                collector_id = %collector_id,
                                                file_path = %path.display(),
                                                "Configuration file changed"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        self.file_watcher_handle = Some(handle);
        info!("File watcher started");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_config_manager_lifecycle() {
        let temp_dir = TempDir::new().unwrap();
        let settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let mut manager = ConfigManager::new(settings);
        manager.start().await.unwrap();

        // Test configuration update
        let config = CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 1000,
            batch_size: 100,
            timeout_seconds: 30,
            capabilities: vec!["process".to_string()],
            settings: HashMap::new(),
        };

        let update_request = UpdateConfigRequest {
            collector_id: "test-collector".to_string(),
            new_config: config.clone(),
            validate_only: false,
        };

        let update_response = manager.update_config(update_request).await.unwrap();
        assert!(update_response.success);

        // Test get configuration
        let get_request = GetConfigRequest {
            collector_id: "test-collector".to_string(),
        };
        let get_response = manager.get_config(get_request).await.unwrap();
        assert_eq!(get_response.config.collector_type, "process");

        manager.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_config_validation() {
        let temp_dir = TempDir::new().unwrap();
        let settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..ConfigManagerSettings::default()
        };
        let mut manager = ConfigManager::new(settings);
        manager.start().await.unwrap();

        // Test invalid configuration
        let invalid_config = CollectorConfig {
            collector_type: "".to_string(), // Invalid: empty
            scan_interval_ms: 0,            // Invalid: zero
            batch_size: 0,                  // Invalid: zero
            timeout_seconds: 0,             // Invalid: zero
            capabilities: vec![],
            settings: HashMap::new(),
        };

        let validate_request = ValidateConfigRequest {
            config: invalid_config,
        };
        let validate_response = manager.validate_config(validate_request).await.unwrap();
        assert!(!validate_response.valid);
        assert_eq!(validate_response.validation_errors.len(), 4);

        // Test valid configuration
        let valid_config = CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 1000,
            batch_size: 100,
            timeout_seconds: 30,
            capabilities: vec!["process".to_string()],
            settings: HashMap::new(),
        };

        let validate_request = ValidateConfigRequest {
            config: valid_config,
        };
        let validate_response = manager.validate_config(validate_request).await.unwrap();
        assert!(validate_response.valid);
        assert!(validate_response.validation_errors.is_empty());

        manager.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_config_rollback() {
        let temp_dir = TempDir::new().unwrap();
        let settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let mut manager = ConfigManager::new(settings);
        manager.start().await.unwrap();

        // Create initial configuration
        let config1 = CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 1000,
            batch_size: 100,
            timeout_seconds: 30,
            capabilities: vec!["process".to_string()],
            settings: HashMap::new(),
        };

        let update_request1 = UpdateConfigRequest {
            collector_id: "rollback-test".to_string(),
            new_config: config1,
            validate_only: false,
        };
        manager.update_config(update_request1).await.unwrap();

        // Update configuration
        let config2 = CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 2000, // Changed
            batch_size: 200,        // Changed
            timeout_seconds: 60,    // Changed
            capabilities: vec!["process".to_string()],
            settings: HashMap::new(),
        };

        let update_request2 = UpdateConfigRequest {
            collector_id: "rollback-test".to_string(),
            new_config: config2,
            validate_only: false,
        };
        manager.update_config(update_request2).await.unwrap();

        // Rollback configuration
        manager
            .rollback_config("rollback-test", None)
            .await
            .unwrap();

        // Verify rollback
        let get_request = GetConfigRequest {
            collector_id: "rollback-test".to_string(),
        };
        let get_response = manager.get_config(get_request).await.unwrap();
        assert_eq!(get_response.config.scan_interval_ms, 1000); // Should be rolled back

        manager.stop().await.unwrap();
    }
}
