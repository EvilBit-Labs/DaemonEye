//! Configuration management with hierarchical overrides.
//!
//! Supports multiple configuration sources with precedence:
//! 1. Command-line flags (highest precedence)
//! 2. Environment variables (PROCMOND_*, SENTINELAGENT_*, SENTINELCLI_*)
//! 3. User configuration file (~/.config/sentineld/config.yaml)
//! 4. System configuration file (/etc/sentineld/config.yaml)
//! 5. Embedded defaults (lowest precedence)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Configuration loading and validation errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration file not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Invalid configuration format: {0}")]
    InvalidFormat(#[from] serde_yaml::Error),

    #[error("IO error reading configuration: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Configuration validation failed: {message}")]
    ValidationError { message: String },
}

/// Main configuration structure for `SentinelD` components.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Config {
    /// Application-specific configuration
    pub app: AppConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Alerting configuration
    pub alerting: AlertingConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
}

/// Application-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    /// Scan interval in milliseconds
    pub scan_interval_ms: u64,
    /// Batch size for process collection
    pub batch_size: usize,
    /// Maximum number of processes to collect per scan
    pub max_processes: Option<usize>,
    /// Enable enhanced metadata collection (requires privileges)
    pub enhanced_metadata: bool,
}

/// Database configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Database file path
    pub path: PathBuf,
    /// Data retention period in days
    pub retention_days: u32,
    /// Maximum database size in MB
    pub max_size_mb: Option<u64>,
    /// Enable database encryption
    pub encryption_enabled: bool,
}

/// Alerting configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertingConfig {
    /// Alert sinks configuration
    pub sinks: Vec<AlertSinkConfig>,
    /// Alert deduplication window in seconds
    pub dedup_window_seconds: u64,
    /// Maximum alert rate per minute
    pub max_alerts_per_minute: Option<u32>,
    /// Threshold in seconds for considering an alert as recent
    pub recent_threshold_seconds: u64,
}

/// Individual alert sink configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertSinkConfig {
    /// Sink type identifier
    pub sink_type: String,
    /// Sink-specific configuration
    pub config: serde_yaml::Value,
    /// Enable/disable this sink
    pub enabled: bool,
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Log format (json, human)
    pub format: String,
    /// Log file path (optional, stdout if not specified)
    pub file: Option<PathBuf>,
    /// Enable structured logging
    pub structured: bool,
}

// Default implementation is now derived

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            scan_interval_ms: 30000,
            batch_size: 1000,
            max_processes: None,
            enhanced_metadata: false,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/sentineld/processes.db"),
            retention_days: 30,
            max_size_mb: None,
            encryption_enabled: false,
        }
    }
}

impl Default for AlertingConfig {
    /// Creates the default `AlertingConfig`.
    ///
    /// Defaults:
    /// - `sinks`: empty list
    /// - `dedup_window_seconds`: 300
    /// - `max_alerts_per_minute`: `None`
    /// - `recent_threshold_seconds`: 3600
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::config::AlertingConfig;
    /// let cfg = AlertingConfig::default();
    /// assert!(cfg.sinks.is_empty());
    /// assert_eq!(cfg.dedup_window_seconds, 300);
    /// assert!(cfg.max_alerts_per_minute.is_none());
    /// assert_eq!(cfg.recent_threshold_seconds, 3600);
    /// ```
    fn default() -> Self {
        Self {
            sinks: vec![],
            dedup_window_seconds: 300,
            max_alerts_per_minute: None,
            recent_threshold_seconds: 3600,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: "human".to_owned(),
            file: None,
            structured: false,
        }
    }
}

/// Configuration loader with hierarchical override support.
pub struct ConfigLoader {
    component: String,
}

impl ConfigLoader {
    /// Create a new configuration loader for the specified component.
    pub fn new(component: &str) -> Self {
        Self {
            component: component.to_owned(),
        }
    }

    /// Load configuration with hierarchical overrides.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let mut config = Config::default();

        // Load from system configuration file
        if let Ok(system_config) = Self::load_system_config() {
            config = Self::merge_configs(config, system_config);
        }

        // Load from user configuration file
        if let Ok(user_config) = Self::load_user_config() {
            config = Self::merge_configs(config, user_config);
        }

        // Apply environment variable overrides
        config = self.apply_env_overrides(config);

        // Validate final configuration
        Self::validate_config(&config)?;

        Ok(config)
    }

    /// Load configuration synchronously (for CLI usage).
    pub fn load_blocking(&self) -> Result<Config, ConfigError> {
        let mut config = Config::default();

        // Apply environment variable overrides
        config = self.apply_env_overrides(config);

        // Validate final configuration
        Self::validate_config(&config)?;

        Ok(config)
    }

    /// Load system-wide configuration file.
    fn load_system_config() -> Result<Config, ConfigError> {
        let path = PathBuf::from("/etc/sentineld/config.yaml");
        if !path.exists() {
            return Err(ConfigError::FileNotFound { path });
        }

        let content = std::fs::read_to_string(&path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Load user-specific configuration file.
    fn load_user_config() -> Result<Config, ConfigError> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
        let path = PathBuf::from(home).join(".config/sentineld/config.yaml");
        if !path.exists() {
            return Err(ConfigError::FileNotFound { path });
        }

        let content = std::fs::read_to_string(&path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Apply environment variable overrides to a Config using the loader's component name as a prefix.
    ///
    /// Environment variables are read with the prefix derived from `self.component` converted to
    /// uppercase (e.g. component "procmond" => "`PROCMOND_SCAN_INTERVAL_MS`"). When present, the
    /// following variables override their corresponding config fields:
    ///
    /// - `{PREFIX}_SCAN_INTERVAL_MS` -> `config.app.scan_interval_ms` (parsed as integer)
    /// - `{PREFIX}_BATCH_SIZE` -> `config.app.batch_size` (parsed as integer)
    /// - `{PREFIX}_LOG_LEVEL` -> `config.logging.level` (string)
    /// - `{PREFIX}_LOG_FORMAT` -> `config.logging.format` (string)
    /// - `{PREFIX}_DATABASE_PATH` -> `config.database.path` (string -> `PathBuf`)
    /// - `{PREFIX}_RECENT_THRESHOLD_SECONDS` -> `config.alerting.recent_threshold_seconds` (parsed as integer)
    ///
    /// Parsing failures for numeric values are ignored and leave the existing config value unchanged.
    /// The function returns a new `Config` with any applied overrides; it does not modify external state.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::env;
    /// use sentinel_lib::config::{ConfigLoader, Config};
    /// // create a loader for component "procmond"
    /// let loader = ConfigLoader::new("procmond");
    /// let mut cfg = Config::default();
    ///
    /// // override scan interval via environment
    /// env::set_var("PROCMOND_SCAN_INTERVAL_MS", "45000");
    ///
    /// let cfg = loader.apply_env_overrides(cfg);
    /// assert_eq!(cfg.app.scan_interval_ms, 45000);
    /// ```
    fn apply_env_overrides(&self, mut config: Config) -> Config {
        // Apply component-specific environment variables
        let prefix = self.component.to_uppercase();

        if let Ok(val) = std::env::var(format!("{prefix}_SCAN_INTERVAL_MS")) {
            if let Ok(interval) = val.parse() {
                config.app.scan_interval_ms = interval;
            }
        }

        if let Ok(val) = std::env::var(format!("{prefix}_BATCH_SIZE")) {
            if let Ok(size) = val.parse() {
                config.app.batch_size = size;
            }
        }

        if let Ok(val) = std::env::var(format!("{prefix}_LOG_LEVEL")) {
            config.logging.level = val;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_LOG_FORMAT")) {
            config.logging.format = val;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_DATABASE_PATH")) {
            config.database.path = val.into();
        }

        if let Ok(val) = std::env::var(format!("{prefix}_RECENT_THRESHOLD_SECONDS")) {
            if let Ok(threshold) = val.parse() {
                config.alerting.recent_threshold_seconds = threshold;
            }
        }

        config
    }

    /// Merge two configurations, with the second taking precedence.
    fn merge_configs(_base: Config, override_config: Config) -> Config {
        // For simplicity, we'll use the override config for most fields
        // In a more sophisticated implementation, we'd merge individual fields
        override_config
    }

    /// Validate the final configuration.
    fn validate_config(config: &Config) -> Result<(), ConfigError> {
        if config.app.scan_interval_ms == 0 {
            return Err(ConfigError::ValidationError {
                message: "scan_interval_ms must be greater than 0".to_owned(),
            });
        }

        if config.app.batch_size == 0 {
            return Err(ConfigError::ValidationError {
                message: "batch_size must be greater than 0".to_owned(),
            });
        }

        if config.database.retention_days == 0 {
            return Err(ConfigError::ValidationError {
                message: "retention_days must be greater than 0".to_owned(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    // use std::env; // Removed due to unsafe_code = "forbid"

    #[tokio::test]
    async fn test_config_loader_default() {
        let loader = ConfigLoader::new("procmond");
        let config = loader.load().expect("Failed to load config in test");

        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.app.batch_size, 1000);
        assert_eq!(config.database.retention_days, 30);
    }

    #[tokio::test]
    async fn test_config_loader_env_overrides() {
        // Note: This test is disabled due to unsafe_code = "forbid"
        // Environment variable testing will be implemented in Task 8
        // when we have proper test infrastructure
        let loader = ConfigLoader::new("procmond");
        let config = loader.load().expect("Failed to load config in test");

        // Test with default values for now
        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();
        config.app.scan_interval_ms = 0;

        let _loader = ConfigLoader::new("procmond");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = Config::default();
        let _loader = ConfigLoader::new("procmond");
        let result = ConfigLoader::validate_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_loader_creation() {
        let loader = ConfigLoader::new("test-component");
        assert_eq!(loader.component, "test-component");
    }

    #[test]
    fn test_config_loader_load_blocking() {
        let loader = ConfigLoader::new("procmond");
        let config = loader
            .load_blocking()
            .expect("Failed to load config in test");
        assert_eq!(config.app.scan_interval_ms, 30000);
    }

    #[test]
    fn test_config_merge() {
        let base = Config::default();
        let mut override_config = Config::default();
        override_config.app.scan_interval_ms = 60000;

        let _loader = ConfigLoader::new("test");
        let merged = ConfigLoader::merge_configs(base, override_config);
        assert_eq!(merged.app.scan_interval_ms, 60000);
    }

    #[test]
    fn test_config_error_display() {
        let errors = vec![
            ConfigError::ValidationError {
                message: "test error".to_owned(),
            },
            ConfigError::IoError(std::io::Error::other("test error")),
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_app_config_creation() {
        let app_config = AppConfig::default();
        assert_eq!(app_config.scan_interval_ms, 30000);
        assert_eq!(app_config.batch_size, 1000);
    }

    #[test]
    fn test_database_config_creation() {
        let db_config = DatabaseConfig::default();
        assert_eq!(
            db_config.path,
            std::path::PathBuf::from("/var/lib/sentineld/processes.db")
        );
        assert_eq!(db_config.retention_days, 30);
    }

    #[test]
    fn test_logging_config_creation() {
        let logging_config = LoggingConfig::default();
        assert_eq!(logging_config.level, "info");
        assert_eq!(logging_config.format, "human");
    }

    #[test]
    fn test_alerting_config_creation() {
        let alerting_config = AlertingConfig::default();
        assert!(alerting_config.sinks.is_empty());
        assert_eq!(alerting_config.recent_threshold_seconds, 3600);
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let yaml = serde_yaml::to_string(&config).expect("Failed to serialize config in test");
        let deserialized: Config =
            serde_yaml::from_str(&yaml).expect("Failed to deserialize config in test");
        assert_eq!(
            config.app.scan_interval_ms,
            deserialized.app.scan_interval_ms
        );
        assert_eq!(
            config.alerting.recent_threshold_seconds,
            deserialized.alerting.recent_threshold_seconds
        );
    }

    #[test]
    fn test_alerting_config_recent_threshold() {
        let mut config = AlertingConfig::default();
        assert_eq!(config.recent_threshold_seconds, 3600);

        // Test custom threshold
        config.recent_threshold_seconds = 1800;
        assert_eq!(config.recent_threshold_seconds, 1800);
    }
}
