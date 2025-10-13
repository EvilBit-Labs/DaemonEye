//! Configuration management with hierarchical overrides using figment.
//!
//! Supports multiple configuration sources with precedence:
//! 1. Command-line flags (highest precedence)
//! 2. Environment variables (`PROCMOND`_*, `DAEMONEYE_AGENT`_*, `DAEMONEYE_CLI`_*)
//! 3. User configuration file (~/.config/daemoneye/config.toml)
//! 4. System configuration file (/etc/daemoneye/config.toml)
//! 5. Embedded defaults (lowest precedence)

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Configuration loading and validation errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("Configuration file not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Invalid configuration format: {0}")]
    InvalidFormat(#[from] figment::Error),

    #[error("IO error reading configuration: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Configuration validation failed: {message}")]
    ValidationError { message: String },
}

/// Main configuration structure for `DaemonEye` components.
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
    /// Sink-specific configuration supporting numeric, boolean, array, and table values
    #[serde(default = "default_sink_config")]
    pub config: serde_json::Value,
    /// Enable/disable this sink
    pub enabled: bool,
}

/// Default configuration for alert sinks (empty object)
fn default_sink_config() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
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
            path: PathBuf::from("/var/lib/daemoneye/processes.db"),
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
    /// use daemoneye_lib::config::AlertingConfig;
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

    /// Load configuration with hierarchical overrides using figment.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let mut figment = Figment::new()
            // Start with embedded defaults
            .merge(Serialized::defaults(Config::default()));

        // System configuration file (optional)
        let system_config_path = "/etc/daemoneye/config.toml";
        if std::path::Path::new(system_config_path).exists() {
            figment = figment.merge(Toml::file(system_config_path));
        }

        // User configuration file (optional)
        let user_config_path = Self::user_config_path();
        if user_config_path.exists() {
            figment = figment.merge(Toml::file(&user_config_path));
        }

        // Environment variables with component prefix
        figment = figment.merge(
            Env::prefixed(&format!(
                "{}_",
                self.component.replace('-', "_").to_uppercase()
            ))
            .split("__"),
        );

        let config = figment.extract()?;

        // Validate final configuration
        Self::validate_config(&config)?;

        Ok(config)
    }

    /// Load configuration synchronously (for CLI usage).
    pub fn load_blocking(&self) -> Result<Config, ConfigError> {
        // Use the same figment-based loading for consistency
        self.load()
    }

    /// Get the user configuration file path using platform-aware directory lookup.
    ///
    /// Priority:
    /// 1. Platform-specific config directory (via `dirs::config_dir()`)
    /// 2. HOME environment variable (if available)
    /// 3. /tmp as last-resort fallback
    fn user_config_path() -> PathBuf {
        // Try platform-aware config directory first
        if let Some(config_dir) = dirs::config_dir() {
            return config_dir.join("daemoneye").join("config.toml");
        }

        // Fallback to HOME environment variable
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("daemoneye")
                .join("config.toml");
        }

        // Last-resort fallback to /tmp
        PathBuf::from("/tmp")
            .join(".config")
            .join("daemoneye")
            .join("config.toml")
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
    async fn test_config_loader_figment_defaults() {
        let loader = ConfigLoader::new("procmond");
        let config = loader.load().expect("Failed to load config in test");

        // Test with figment-loaded defaults
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
    fn test_config_loader_handles_missing_files() {
        // This test verifies that configuration loading works even when
        // system and user config files don't exist (should fall back to defaults)
        let loader = ConfigLoader::new("test-component");
        let config = loader
            .load()
            .expect("Failed to load config with missing files");

        // Should get default values when no config files exist
        assert_eq!(config.app.scan_interval_ms, 30000);
        assert_eq!(config.app.batch_size, 1000);
    }

    #[test]
    fn test_config_figment_serialization() {
        let config = Config::default();
        let figment = Figment::new().merge(Serialized::defaults(config.clone()));
        let extracted: Config = figment.extract().expect("Failed to extract config in test");
        assert_eq!(config.app.scan_interval_ms, extracted.app.scan_interval_ms);
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
            std::path::PathBuf::from("/var/lib/daemoneye/processes.db")
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
    fn test_config_toml_serialization() {
        let config = Config::default();
        let toml = toml::to_string(&config).expect("Failed to serialize config to TOML in test");
        let deserialized: Config =
            toml::from_str(&toml).expect("Failed to deserialize config from TOML in test");
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

    #[test]
    fn test_alert_sink_config_with_different_value_types() {
        use serde_json::json;

        // Test with string values
        let sink_with_strings = AlertSinkConfig {
            sink_type: "webhook".to_owned(),
            config: json!({
                "url": "https://example.com/webhook",
                "method": "POST"
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_strings
                .config
                .get("url")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com/webhook")
        );

        // Test with numeric values
        let sink_with_numbers = AlertSinkConfig {
            sink_type: "syslog".to_owned(),
            config: json!({
                "port": 514,
                "timeout_ms": 5000,
                "retry_count": 3
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_numbers
                .config
                .get("port")
                .and_then(serde_json::Value::as_u64),
            Some(514)
        );
        assert_eq!(
            sink_with_numbers
                .config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_u64),
            Some(5000)
        );
        assert_eq!(
            sink_with_numbers
                .config
                .get("retry_count")
                .and_then(serde_json::Value::as_u64),
            Some(3)
        );

        // Test with boolean values
        let sink_with_bools = AlertSinkConfig {
            sink_type: "custom".to_owned(),
            config: json!({
                "use_tls": true,
                "verify_ssl": false
            }),
            enabled: true,
        };
        assert_eq!(
            sink_with_bools
                .config
                .get("use_tls")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            sink_with_bools
                .config
                .get("verify_ssl")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );

        // Test with array values
        let sink_with_arrays = AlertSinkConfig {
            sink_type: "multi".to_owned(),
            config: json!({
                "endpoints": ["http://endpoint1.com", "http://endpoint2.com"],
                "priorities": [1, 2, 3]
            }),
            enabled: true,
        };
        assert!(
            sink_with_arrays
                .config
                .get("endpoints")
                .is_some_and(serde_json::Value::is_array)
        );
        assert_eq!(
            sink_with_arrays
                .config
                .get("endpoints")
                .and_then(serde_json::Value::as_array)
                .map(std::vec::Vec::len),
            Some(2)
        );
    }

    #[test]
    fn test_config_toml_with_complex_sink_config() {
        let toml_str = r#"
[app]
scan_interval_ms = 30000
batch_size = 1000
enhanced_metadata = false

[database]
path = "/tmp/test.db"
retention_days = 30
encryption_enabled = false

[logging]
level = "info"
format = "human"
structured = false

[alerting]
dedup_window_seconds = 300
recent_threshold_seconds = 3600

[[alerting.sinks]]
sink_type = "webhook"
enabled = true
[alerting.sinks.config]
url = "https://example.com/webhook"
timeout_ms = 5000
retry_count = 3

[[alerting.sinks]]
sink_type = "syslog"
enabled = true
[alerting.sinks.config]
facility = "daemon"
port = 514
use_tls = false
"#;

        let config: Config =
            toml::from_str(toml_str).expect("Failed to parse TOML with complex sink config");

        assert_eq!(config.alerting.sinks.len(), 2);

        // Verify webhook sink
        let webhook_sink = config
            .alerting
            .sinks
            .first()
            .expect("expected first alert sink");
        assert_eq!(webhook_sink.sink_type, "webhook");
        assert!(webhook_sink.enabled);
        assert_eq!(
            webhook_sink
                .config
                .get("url")
                .and_then(serde_json::Value::as_str),
            Some("https://example.com/webhook")
        );
        assert_eq!(
            webhook_sink
                .config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_i64),
            Some(5000)
        );
        assert_eq!(
            webhook_sink
                .config
                .get("retry_count")
                .and_then(serde_json::Value::as_i64),
            Some(3)
        );

        // Verify syslog sink
        let syslog_sink = config
            .alerting
            .sinks
            .get(1)
            .expect("expected second alert sink");
        assert_eq!(syslog_sink.sink_type, "syslog");
        assert!(syslog_sink.enabled);
        assert_eq!(
            syslog_sink
                .config
                .get("facility")
                .and_then(serde_json::Value::as_str),
            Some("daemon")
        );
        assert_eq!(
            syslog_sink
                .config
                .get("port")
                .and_then(serde_json::Value::as_i64),
            Some(514)
        );
        assert_eq!(
            syslog_sink
                .config
                .get("use_tls")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }
}
