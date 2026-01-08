//! Configuration management with hierarchical overrides using figment.
//!
//! Supports multiple configuration sources with precedence:
//! 1. Command-line flags (highest precedence)
//! 2. Environment variables (`PROCMOND`_*, `DAEMONEYE_AGENT`_*, `DAEMONEYE_CLI`_*)
//! 3. User configuration file (~/.config/daemoneye/config.toml)
//! 4. System configuration file (/etc/daemoneye/config.toml)
//! 5. Embedded defaults (lowest precedence)

#[cfg(not(windows))]
use anyhow::Context;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tracing::{info, warn};
use unidirs::Directories;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
    /// `EventBus` broker configuration
    pub broker: BrokerConfig,
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

/// Process manager configuration for collector lifecycle management.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessManagerConfig {
    /// Graceful shutdown timeout in seconds
    pub graceful_shutdown_timeout_seconds: u64,
    /// Force shutdown timeout in seconds
    pub force_shutdown_timeout_seconds: u64,
    /// Health check interval in seconds
    pub health_check_interval_seconds: u64,
    /// Enable automatic restart on collector failure
    pub enable_auto_restart: bool,
    /// Maximum restart attempts before giving up
    pub max_restart_attempts: u32,
}

impl Default for ProcessManagerConfig {
    fn default() -> Self {
        Self {
            graceful_shutdown_timeout_seconds: 30,
            force_shutdown_timeout_seconds: 5,
            health_check_interval_seconds: 60,
            enable_auto_restart: false,
            max_restart_attempts: 3,
        }
    }
}

/// `EventBus` broker configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrokerConfig {
    /// Socket path for the embedded broker
    pub socket_path: String,
    /// Enable the embedded broker
    pub enabled: bool,
    /// Broker startup timeout in seconds
    pub startup_timeout_seconds: u64,
    /// Broker shutdown timeout in seconds
    pub shutdown_timeout_seconds: u64,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Message buffer size per connection
    pub message_buffer_size: usize,
    /// Topic hierarchy configuration
    pub topic_hierarchy: TopicHierarchyConfig,
    /// Collector binary paths (`collector_type` -> `binary_path`)
    #[serde(default)]
    pub collector_binaries: std::collections::HashMap<String, PathBuf>,
    /// Process manager configuration
    #[serde(default)]
    pub process_manager: ProcessManagerConfig,
    /// Configuration directory for collector configs
    #[serde(default = "default_config_directory")]
    pub config_directory: PathBuf,
}

// BrokerConfig implementation moved below Default impl

/// Topic hierarchy configuration for the `EventBus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopicHierarchyConfig {
    /// Enable wildcard topic matching
    pub enable_wildcards: bool,
    /// Maximum topic depth allowed
    pub max_topic_depth: usize,
    /// Event topic prefixes
    pub event_topics: EventTopicsConfig,
    /// Control topic prefixes
    pub control_topics: ControlTopicsConfig,
}

/// Event topic configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventTopicsConfig {
    /// Process event topics (events.process.*)
    pub process: String,
    /// Network event topics (events.network.*)
    pub network: String,
    /// Filesystem event topics (events.filesystem.*)
    pub filesystem: String,
    /// Performance event topics (events.performance.*)
    pub performance: String,
}

/// Control topic configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlTopicsConfig {
    /// Collector lifecycle topics (control.collector.*)
    pub collector: String,
    /// Health monitoring topics (control.health.*)
    pub health: String,
}

// Default implementation is now derived

/// Default configuration directory for collector configs.
///
/// Returns system-wide configuration directory paths for system-level components
/// (daemoneye-agent and collectors) using `unidirs::ServiceDirs`. User-level components
/// (daemoneye-cli) should use user-specific directories instead.
///
/// Uses platform-aware system-wide directories via `unidirs::ServiceDirs`:
/// - **Linux**: `/var/lib/evilbitlabs/daemoneye/configs` (via `ServiceDirs`)
/// - **macOS**: `/Library/Application Support/evilbitlabs/daemoneye/configs` (via `ServiceDirs`)
/// - **Windows**: `C:\ProgramData\evilbitlabs\daemoneye\configs` (via `ServiceDirs`)
/// - **Other Unix**: Platform-appropriate system directory (via `ServiceDirs`)
fn default_config_directory() -> PathBuf {
    // Use ServiceDirs for system-wide directories (organization, application)
    let service_dirs = unidirs::ServiceDirs::new("evilbitlabs", "daemoneye");

    // Convert Utf8Path to PathBuf and append "configs"
    service_dirs
        .config_dir()
        .to_path_buf()
        .join("configs")
        .into()
}

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

impl Default for BrokerConfig {
    fn default() -> Self {
        let socket_path = default_socket_path();

        Self {
            socket_path,
            enabled: true,
            startup_timeout_seconds: 30,
            shutdown_timeout_seconds: 60,
            max_connections: 100,
            message_buffer_size: 1000,
            topic_hierarchy: TopicHierarchyConfig::default(),
            collector_binaries: std::collections::HashMap::new(),
            process_manager: ProcessManagerConfig::default(),
            config_directory: default_config_directory(),
        }
    }
}

/// Determines the default socket path with platform-specific fallbacks.
///
/// Uses `unidirs::ServiceDirs` to access the system-wide data directory for
/// system-level components (daemoneye-agent and collectors). The socket is placed
/// in the data directory rather than runtime directory for persistence across
/// system restarts.
///
/// Platform-specific paths (via `ServiceDirs` `data_dir`):
/// - **Linux**: `/var/lib/evilbitlabs/daemoneye/daemoneye-eventbus.sock`
/// - **macOS**: `/Library/Application Support/evilbitlabs/daemoneye/daemoneye-eventbus.sock`
/// - **Windows**: Uses named pipes (`\\.\pipe\daemoneye-eventbus`) which don't
///   require directory paths
///
/// The function validates path length against Unix domain socket limits
/// (108 bytes including null terminator) and falls back to `/tmp/de.sock` if the
/// computed path exceeds the limit.
///
/// # Platform Limitations
///
/// - **Unix**: Paths must be ≤ 107 characters (108 including null terminator)
/// - **Windows**: Named pipes are used (no path length concerns)
fn default_socket_path() -> String {
    #[cfg(unix)]
    {
        // Unix domain sockets have a 108-byte limit (including null terminator)
        // We check against 107 to leave room for the null terminator
        const UNIX_SOCKET_MAX_LEN: usize = 107;

        // Use ServiceDirs for system-wide data directory
        let service_dirs = unidirs::ServiceDirs::new("evilbitlabs", "daemoneye");
        let data_dir = service_dirs.data_dir();

        // Build socket path
        let socket_name = "daemoneye-eventbus.sock";
        let socket_path = data_dir.join(socket_name);
        let socket_path_str = socket_path.to_string();

        if socket_path_str.len() <= UNIX_SOCKET_MAX_LEN {
            socket_path_str
        } else {
            // Fallback to shorter path in /tmp
            // Use a very short filename to ensure we stay under the limit
            let short_path = "/tmp/de.sock".to_owned();
            warn!(
                original_path = %socket_path_str,
                fallback_path = %short_path,
                "Socket path exceeds Unix domain socket length limit ({} bytes), using fallback",
                UNIX_SOCKET_MAX_LEN + 1
            );
            short_path
        }
    }

    #[cfg(windows)]
    {
        // Windows named pipes don't have the same path length restrictions
        r"\\.\pipe\daemoneye-eventbus".to_owned()
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        // For non-Unix, non-Windows platforms, document the limitation
        // In practice, these platforms may need TCP loopback as a transport
        // For now, we use a simple path that may not work on all platforms
        warn!(
            "Unsupported platform detected. Socket path may not function correctly. \
            Consider using TCP loopback (127.0.0.1) as an alternative transport."
        );
        // Use a minimal path that's unlikely to cause issues
        "/tmp/daemoneye-eventbus.sock".to_owned()
    }
}

impl BrokerConfig {
    /// Ensures the directory for the socket path exists with proper permissions.
    ///
    /// This function performs filesystem operations and should be called during broker startup.
    /// On Unix systems, it ensures the directory is owned by the current user and has
    /// restrictive permissions (0700) to prevent unauthorized access.
    ///
    /// # Returns
    ///
    /// Returns the socket path on success.
    ///
    /// # Errors
    ///
    /// Returns an error if directory creation or permission setting fails.
    pub fn ensure_socket_directory(&self) -> anyhow::Result<std::path::PathBuf> {
        let socket_path = std::path::Path::new(&self.socket_path);

        #[cfg(windows)]
        {
            // Windows named pipes don't require directory creation
            Ok(socket_path.to_path_buf())
        }

        #[cfg(not(windows))]
        {
            let parent_dir = socket_path.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "Socket path {} has no parent directory.",
                    socket_path.display()
                )
            })?;

            if !parent_dir.exists() {
                std::fs::create_dir_all(parent_dir).with_context(|| {
                    format!(
                        "Failed to create socket directory: {}",
                        parent_dir.display()
                    )
                })?;
                info!(
                    directory = %parent_dir.display(),
                    "Created socket directory"
                );
            }

            // Set restrictive permissions (owner read/write/execute only)
            #[cfg(unix)]
            {
                use std::fs;
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(parent_dir, perms).with_context(|| {
                    format!(
                        "Failed to set permissions on socket directory: {}",
                        parent_dir.display()
                    )
                })?;
            };

            Ok(socket_path.to_path_buf())
        }
    }

    /// Resolves the binary path for a collector type.
    ///
    /// Searches in the following order:
    /// 1. Configured path in `collector_binaries`
    /// 2. Default installation paths
    /// 3. Development build paths
    ///
    /// # Arguments
    ///
    /// * `collector_type` - The type of collector (e.g., "procmond", "netmond")
    ///
    /// # Returns
    ///
    /// Returns the resolved binary path, or None if not found.
    pub fn resolve_collector_binary(&self, collector_type: &str) -> Option<PathBuf> {
        // 1. Check configured paths first
        if let Some(path) = self.collector_binaries.get(collector_type) {
            if path.exists() {
                return Some(path.clone());
            }
            warn!(
                collector_type,
                configured_path = %path.display(),
                "Configured collector binary not found"
            );
        }

        // 2. Check default installation paths
        let default_paths = [
            PathBuf::from(format!("/usr/local/bin/{collector_type}")),
            PathBuf::from(format!("/usr/bin/{collector_type}")),
        ];

        for path in &default_paths {
            if path.exists() {
                info!(
                    collector_type,
                    resolved_path = %path.display(),
                    "Resolved collector binary from default path"
                );
                return Some(path.clone());
            }
        }

        // 3. Check development build paths
        let dev_paths = [
            PathBuf::from(format!("./target/release/{collector_type}")),
            PathBuf::from(format!("./target/debug/{collector_type}")),
        ];

        for path in &dev_paths {
            if path.exists() {
                info!(
                    collector_type,
                    resolved_path = %path.display(),
                    "Resolved collector binary from development path"
                );
                return Some(path.clone());
            }
        }

        warn!(collector_type, "Failed to resolve collector binary path");
        None
    }
}

impl Default for TopicHierarchyConfig {
    fn default() -> Self {
        Self {
            enable_wildcards: true,
            max_topic_depth: 5,
            event_topics: EventTopicsConfig::default(),
            control_topics: ControlTopicsConfig::default(),
        }
    }
}

impl Default for EventTopicsConfig {
    fn default() -> Self {
        Self {
            process: "events.process".to_owned(),
            network: "events.network".to_owned(),
            filesystem: "events.filesystem".to_owned(),
            performance: "events.performance".to_owned(),
        }
    }
}

impl Default for ControlTopicsConfig {
    fn default() -> Self {
        Self {
            collector: "control.collector".to_owned(),
            health: "control.health".to_owned(),
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

        // System configuration file (optional, platform-specific)
        #[cfg(unix)]
        {
            let system_config_path = std::path::Path::new("/etc/daemoneye/config.toml");
            if system_config_path.exists() {
                figment = figment.merge(Toml::file(system_config_path));
            }
        }

        #[cfg(windows)]
        {
            // Windows: ProgramData provides machine-wide configuration
            if let Ok(program_data) = std::env::var("PROGRAMDATA") {
                let system_config_path = std::path::Path::new(&program_data)
                    .join("DaemonEye")
                    .join("config.toml");
                if system_config_path.exists() {
                    figment = figment.merge(Toml::file(&system_config_path));
                }
            }
        }

        // Other platforms rely solely on user-scoped configuration (loaded below).

        // User configuration file (optional)
        match Self::user_config_path() {
            Ok(user_config_path) => {
                if user_config_path.exists() {
                    figment = figment.merge(Toml::file(&user_config_path));
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Skipping user-scoped configuration because the directory could not be determined"
                );
            }
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
    /// 3. Returns an error if no user configuration directory can be determined
    fn user_config_path() -> Result<PathBuf, ConfigError> {
        // Try platform-aware config directory first
        if let Some(config_dir) = dirs::config_dir() {
            return Ok(config_dir.join("daemoneye").join("config.toml"));
        }

        // Fallback to HOME environment variable
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home)
                .join(".config")
                .join("daemoneye")
                .join("config.toml"));
        }

        Err(ConfigError::ValidationError {
            message: "Unable to determine a user configuration directory. Ensure HOME is set or provide an explicit configuration path.".to_owned(),
        })
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
    fn test_broker_config_creation() {
        let broker_config = BrokerConfig::default();
        if cfg!(windows) {
            assert_eq!(broker_config.socket_path, r"\\.\pipe\daemoneye-eventbus");
        } else {
            assert!(
                broker_config
                    .socket_path
                    .ends_with("daemoneye-eventbus.sock")
            );
        }
        assert!(broker_config.enabled);
        assert_eq!(broker_config.startup_timeout_seconds, 30);
        assert_eq!(broker_config.shutdown_timeout_seconds, 60);
        assert_eq!(broker_config.max_connections, 100);
        assert_eq!(broker_config.message_buffer_size, 1000);
    }

    #[test]
    fn test_topic_hierarchy_config_creation() {
        let topic_config = TopicHierarchyConfig::default();
        assert!(topic_config.enable_wildcards);
        assert_eq!(topic_config.max_topic_depth, 5);
        assert_eq!(topic_config.event_topics.process, "events.process");
        assert_eq!(topic_config.control_topics.health, "control.health");
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

[broker]
socket_path = "/tmp/test-broker.sock"
enabled = true
startup_timeout_seconds = 30
shutdown_timeout_seconds = 60
max_connections = 100
message_buffer_size = 1000

[broker.topic_hierarchy]
enable_wildcards = true
max_topic_depth = 5

[broker.topic_hierarchy.event_topics]
process = "events.process"
network = "events.network"
filesystem = "events.filesystem"
performance = "events.performance"

[broker.topic_hierarchy.control_topics]
collector = "control.collector"
health = "control.health"

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

        // Verify broker configuration
        assert_eq!(config.broker.socket_path, "/tmp/test-broker.sock");
        assert!(config.broker.enabled);
        assert_eq!(config.broker.startup_timeout_seconds, 30);
        assert_eq!(config.broker.shutdown_timeout_seconds, 60);
        assert_eq!(config.broker.max_connections, 100);
        assert_eq!(config.broker.message_buffer_size, 1000);

        // Verify topic hierarchy configuration
        assert!(config.broker.topic_hierarchy.enable_wildcards);
        assert_eq!(config.broker.topic_hierarchy.max_topic_depth, 5);
        assert_eq!(
            config.broker.topic_hierarchy.event_topics.process,
            "events.process"
        );
        assert_eq!(
            config.broker.topic_hierarchy.control_topics.health,
            "control.health"
        );

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
