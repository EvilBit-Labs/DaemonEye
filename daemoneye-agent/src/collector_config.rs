//! Collector configuration for daemoneye-agent.
//!
//! This module handles loading and validating collector configurations from
//! `/etc/daemoneye/collectors.json` (or a configurable path). The configuration
//! defines which collectors to spawn, their binary paths, and collector-specific
//! settings.
//!
//! # Configuration Format
//!
//! ```json
//! {
//!   "collectors": [
//!     {
//!       "id": "procmond",
//!       "collector_type": "process-monitor",
//!       "binary_path": "/usr/bin/procmond",
//!       "enabled": true,
//!       "auto_restart": true,
//!       "startup_timeout_secs": 60,
//!       "config": {
//!         "collection_interval_secs": 30,
//!         "enhanced_metadata": true,
//!         "compute_hashes": false
//!       }
//!     }
//!   ]
//! }
//! ```

// Module items will be used when main.rs integrates the loading state machine (Task #15)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{info, warn};

/// Default path for collector configuration file.
#[cfg(unix)]
pub const DEFAULT_COLLECTORS_CONFIG_PATH: &str = "/etc/daemoneye/collectors.json";

#[cfg(windows)]
pub const DEFAULT_COLLECTORS_CONFIG_PATH: &str = r"C:\ProgramData\DaemonEye\config\collectors.json";

/// Default startup timeout for collectors in seconds.
pub const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 60;

/// Default heartbeat interval for collectors in seconds.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Errors that can occur when loading or validating collector configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CollectorConfigError {
    /// Configuration file not found.
    #[error("collector configuration file not found: {path}")]
    FileNotFound { path: PathBuf },

    /// Failed to read configuration file.
    #[error("failed to read collector configuration: {0}")]
    IoError(#[from] std::io::Error),

    /// Failed to parse JSON configuration.
    #[error("failed to parse collector configuration: {0}")]
    ParseError(#[from] serde_json::Error),

    /// Configuration validation failed.
    #[error("collector configuration validation failed: {message}")]
    ValidationError { message: String },

    /// Collector binary not found or not executable.
    #[error("collector binary not found or not executable: {path}")]
    BinaryNotFound { path: PathBuf },

    /// Duplicate collector ID.
    #[error("duplicate collector ID: {id}")]
    DuplicateCollectorId { id: String },
}

/// Root configuration structure for collectors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectorsConfig {
    /// List of collector configurations.
    #[serde(default)]
    pub collectors: Vec<CollectorEntry>,
}

/// Configuration for a single collector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectorEntry {
    /// Unique identifier for this collector instance.
    pub id: String,

    /// Type of collector (e.g., "process-monitor", "network-monitor").
    pub collector_type: String,

    /// Path to the collector binary.
    pub binary_path: PathBuf,

    /// Whether this collector is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Whether to automatically restart the collector on failure.
    #[serde(default)]
    pub auto_restart: bool,

    /// Startup timeout in seconds (how long to wait for "ready" status).
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_secs: u64,

    /// Heartbeat interval in seconds.
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,

    /// Collector-specific configuration (passed to collector via environment or config file).
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

const fn default_enabled() -> bool {
    true
}

const fn default_startup_timeout() -> u64 {
    DEFAULT_STARTUP_TIMEOUT_SECS
}

const fn default_heartbeat_interval() -> u64 {
    DEFAULT_HEARTBEAT_INTERVAL_SECS
}

impl CollectorsConfig {
    /// Load collector configuration from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be read
    /// - The JSON is malformed
    /// - Validation fails (e.g., duplicate IDs, invalid paths)
    pub fn load_from_file(path: &Path) -> Result<Self, CollectorConfigError> {
        if !path.exists() {
            return Err(CollectorConfigError::FileNotFound {
                path: path.to_path_buf(),
            });
        }

        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;

        config.validate()?;

        info!(
            path = %path.display(),
            collector_count = config.collectors.len(),
            "Loaded collector configuration"
        );

        Ok(config)
    }

    /// Load collector configuration from the default path, or return empty config if not found.
    ///
    /// This method is lenient - if the config file doesn't exist, it returns an empty
    /// configuration rather than an error. This supports deployments without explicit
    /// collector configuration.
    pub fn load_or_default() -> Self {
        let default_path = Path::new(DEFAULT_COLLECTORS_CONFIG_PATH);

        match Self::load_from_file(default_path) {
            Ok(config) => config,
            Err(CollectorConfigError::FileNotFound { path }) => {
                info!(
                    path = %path.display(),
                    "Collector configuration file not found, using empty configuration"
                );
                Self::default()
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to load collector configuration, using empty configuration"
                );
                Self::default()
            }
        }
    }

    /// Validate the configuration.
    ///
    /// Checks for:
    /// - Duplicate collector IDs
    /// - Empty collector IDs or types
    /// - Binary path existence and executability (for enabled collectors)
    pub fn validate(&self) -> Result<(), CollectorConfigError> {
        let mut seen_ids = std::collections::HashSet::new();

        for collector in &self.collectors {
            // Check for empty ID
            if collector.id.trim().is_empty() {
                return Err(CollectorConfigError::ValidationError {
                    message: "collector ID cannot be empty".to_owned(),
                });
            }

            // Check for empty type
            if collector.collector_type.trim().is_empty() {
                return Err(CollectorConfigError::ValidationError {
                    message: format!(
                        "collector type cannot be empty for collector '{}'",
                        collector.id
                    ),
                });
            }

            // Check for duplicate IDs
            if !seen_ids.insert(&collector.id) {
                return Err(CollectorConfigError::DuplicateCollectorId {
                    id: collector.id.clone(),
                });
            }

            // Validate binary path for enabled collectors
            if collector.enabled {
                Self::validate_binary_path(&collector.binary_path, &collector.id)?;
            }

            // Validate timeout values
            if collector.startup_timeout_secs == 0 {
                return Err(CollectorConfigError::ValidationError {
                    message: format!(
                        "startup_timeout_secs must be greater than 0 for collector '{}'",
                        collector.id
                    ),
                });
            }

            if collector.heartbeat_interval_secs == 0 {
                return Err(CollectorConfigError::ValidationError {
                    message: format!(
                        "heartbeat_interval_secs must be greater than 0 for collector '{}'",
                        collector.id
                    ),
                });
            }
        }

        Ok(())
    }

    /// Validate that a binary path exists and is executable.
    fn validate_binary_path(path: &Path, collector_id: &str) -> Result<(), CollectorConfigError> {
        if !path.exists() {
            warn!(
                collector_id = %collector_id,
                path = %path.display(),
                "Collector binary not found"
            );
            return Err(CollectorConfigError::BinaryNotFound {
                path: path.to_path_buf(),
            });
        }

        // Check if file is executable (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path)?;
            let permissions = metadata.permissions();
            // Check if any execute bit is set (owner, group, or other)
            if permissions.mode() & 0o111 == 0 {
                warn!(
                    collector_id = %collector_id,
                    path = %path.display(),
                    "Collector binary is not executable"
                );
                return Err(CollectorConfigError::BinaryNotFound {
                    path: path.to_path_buf(),
                });
            }
        }

        Ok(())
    }

    /// Get all enabled collectors.
    pub fn enabled_collectors(&self) -> impl Iterator<Item = &CollectorEntry> {
        self.collectors.iter().filter(|c| c.enabled)
    }

    /// Get a collector by ID.
    pub fn get_collector(&self, id: &str) -> Option<&CollectorEntry> {
        self.collectors.iter().find(|c| c.id == id)
    }

    /// Get the number of enabled collectors.
    pub fn enabled_count(&self) -> usize {
        self.collectors.iter().filter(|c| c.enabled).count()
    }

    /// Get all collector IDs (enabled and disabled).
    pub fn collector_ids(&self) -> Vec<&str> {
        self.collectors.iter().map(|c| c.id.as_str()).collect()
    }

    /// Get all enabled collector IDs.
    pub fn enabled_collector_ids(&self) -> Vec<&str> {
        self.collectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.as_str())
            .collect()
    }
}

impl CollectorEntry {
    /// Create a new collector entry with minimal required fields.
    pub fn new(
        id: impl Into<String>,
        collector_type: impl Into<String>,
        binary_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            collector_type: collector_type.into(),
            binary_path: binary_path.into(),
            enabled: true,
            auto_restart: false,
            startup_timeout_secs: DEFAULT_STARTUP_TIMEOUT_SECS,
            heartbeat_interval_secs: DEFAULT_HEARTBEAT_INTERVAL_SECS,
            config: HashMap::new(),
        }
    }

    /// Builder method to set enabled status.
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Builder method to set `auto_restart`.
    #[must_use]
    pub const fn with_auto_restart(mut self, auto_restart: bool) -> Self {
        self.auto_restart = auto_restart;
        self
    }

    /// Builder method to set startup timeout.
    #[must_use]
    pub const fn with_startup_timeout(mut self, secs: u64) -> Self {
        self.startup_timeout_secs = secs;
        self
    }

    /// Builder method to set heartbeat interval.
    #[must_use]
    pub const fn with_heartbeat_interval(mut self, secs: u64) -> Self {
        self.heartbeat_interval_secs = secs;
        self
    }

    /// Builder method to add config value.
    #[must_use]
    pub fn with_config(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.config.insert(key.into(), value);
        self
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn sample_config_json() -> &'static str {
        r#"{
            "collectors": [
                {
                    "id": "procmond",
                    "collector_type": "process-monitor",
                    "binary_path": "/usr/bin/procmond",
                    "enabled": true,
                    "auto_restart": true,
                    "startup_timeout_secs": 60,
                    "heartbeat_interval_secs": 30,
                    "config": {
                        "collection_interval_secs": 30,
                        "enhanced_metadata": true,
                        "compute_hashes": false
                    }
                }
            ]
        }"#
    }

    #[test]
    fn test_parse_valid_config() {
        let config: CollectorsConfig = serde_json::from_str(sample_config_json()).unwrap();

        assert_eq!(config.collectors.len(), 1);
        let procmond = &config.collectors[0];
        assert_eq!(procmond.id, "procmond");
        assert_eq!(procmond.collector_type, "process-monitor");
        assert_eq!(procmond.binary_path, PathBuf::from("/usr/bin/procmond"));
        assert!(procmond.enabled);
        assert!(procmond.auto_restart);
        assert_eq!(procmond.startup_timeout_secs, 60);
        assert_eq!(procmond.heartbeat_interval_secs, 30);

        // Check nested config
        assert_eq!(
            procmond.config.get("collection_interval_secs"),
            Some(&serde_json::Value::Number(30.into()))
        );
        assert_eq!(
            procmond.config.get("enhanced_metadata"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn test_default_values() {
        let json = r#"{
            "collectors": [
                {
                    "id": "test",
                    "collector_type": "test-type",
                    "binary_path": "/usr/bin/test"
                }
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();
        let collector = &config.collectors[0];

        assert!(collector.enabled); // default true
        assert!(!collector.auto_restart); // default false
        assert_eq!(collector.startup_timeout_secs, DEFAULT_STARTUP_TIMEOUT_SECS);
        assert_eq!(
            collector.heartbeat_interval_secs,
            DEFAULT_HEARTBEAT_INTERVAL_SECS
        );
        assert!(collector.config.is_empty());
    }

    #[test]
    fn test_empty_config() {
        let json = r#"{"collectors": []}"#;
        let config: CollectorsConfig = serde_json::from_str(json).unwrap();

        assert!(config.collectors.is_empty());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validation_duplicate_id() {
        let json = r#"{
            "collectors": [
                {"id": "test", "collector_type": "t1", "binary_path": "/bin/test"},
                {"id": "test", "collector_type": "t2", "binary_path": "/bin/test2"}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(matches!(
            result,
            Err(CollectorConfigError::DuplicateCollectorId { id }) if id == "test"
        ));
    }

    #[test]
    fn test_validation_empty_id() {
        let json = r#"{
            "collectors": [
                {"id": "", "collector_type": "test", "binary_path": "/bin/test"}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(matches!(
            result,
            Err(CollectorConfigError::ValidationError { message }) if message.contains("ID cannot be empty")
        ));
    }

    #[test]
    fn test_validation_empty_type() {
        let json = r#"{
            "collectors": [
                {"id": "test", "collector_type": "", "binary_path": "/bin/test"}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(matches!(
            result,
            Err(CollectorConfigError::ValidationError { message }) if message.contains("type cannot be empty")
        ));
    }

    #[test]
    fn test_validation_zero_timeout() {
        let json = r#"{
            "collectors": [
                {"id": "test", "collector_type": "test", "binary_path": "/bin/test", "startup_timeout_secs": 0}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(matches!(
            result,
            Err(CollectorConfigError::ValidationError { message }) if message.contains("startup_timeout_secs")
        ));
    }

    #[test]
    fn test_enabled_collectors_filter() {
        let json = r#"{
            "collectors": [
                {"id": "enabled1", "collector_type": "t", "binary_path": "/bin/e1", "enabled": true},
                {"id": "disabled", "collector_type": "t", "binary_path": "/bin/d", "enabled": false},
                {"id": "enabled2", "collector_type": "t", "binary_path": "/bin/e2", "enabled": true}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.enabled_count(), 2);

        let enabled_ids: Vec<&str> = config.enabled_collector_ids();
        assert_eq!(enabled_ids, vec!["enabled1", "enabled2"]);
    }

    #[test]
    fn test_get_collector() {
        let json = r#"{
            "collectors": [
                {"id": "a", "collector_type": "t1", "binary_path": "/bin/a"},
                {"id": "b", "collector_type": "t2", "binary_path": "/bin/b"}
            ]
        }"#;

        let config: CollectorsConfig = serde_json::from_str(json).unwrap();

        assert!(config.get_collector("a").is_some());
        assert!(config.get_collector("b").is_some());
        assert!(config.get_collector("c").is_none());
    }

    #[test]
    fn test_collector_entry_builder() {
        let entry = CollectorEntry::new("test-id", "test-type", "/usr/bin/test")
            .with_enabled(false)
            .with_auto_restart(true)
            .with_startup_timeout(120)
            .with_heartbeat_interval(15)
            .with_config("key1", serde_json::Value::Bool(true));

        assert_eq!(entry.id, "test-id");
        assert_eq!(entry.collector_type, "test-type");
        assert_eq!(entry.binary_path, PathBuf::from("/usr/bin/test"));
        assert!(!entry.enabled);
        assert!(entry.auto_restart);
        assert_eq!(entry.startup_timeout_secs, 120);
        assert_eq!(entry.heartbeat_interval_secs, 15);
        assert_eq!(
            entry.config.get("key1"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn test_load_from_file() {
        let mut temp_file = NamedTempFile::new().unwrap();

        // Write a config with a disabled collector (so binary validation is skipped)
        let json = r#"{
            "collectors": [
                {"id": "test", "collector_type": "test", "binary_path": "/nonexistent", "enabled": false}
            ]
        }"#;
        temp_file.write_all(json.as_bytes()).unwrap();

        let config = CollectorsConfig::load_from_file(temp_file.path()).unwrap();
        assert_eq!(config.collectors.len(), 1);
        assert_eq!(config.collectors[0].id, "test");
    }

    #[test]
    fn test_load_file_not_found() {
        let result = CollectorsConfig::load_from_file(Path::new("/nonexistent/config.json"));
        assert!(matches!(
            result,
            Err(CollectorConfigError::FileNotFound { .. })
        ));
    }

    #[test]
    fn test_load_or_default_returns_empty_when_no_file() {
        // This test relies on DEFAULT_COLLECTORS_CONFIG_PATH not existing in test environment
        let config = CollectorsConfig::load_or_default();
        // Should return empty config without error
        assert!(config.collectors.is_empty());
    }

    #[test]
    fn test_default_config() {
        let config = CollectorsConfig::default();
        assert!(config.collectors.is_empty());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let original = CollectorsConfig {
            collectors: vec![
                CollectorEntry::new("test", "test-type", "/bin/test")
                    .with_auto_restart(true)
                    .with_config("setting", serde_json::json!(42)),
            ],
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: CollectorsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
    }
}
