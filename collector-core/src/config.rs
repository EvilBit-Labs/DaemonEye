//! Configuration management for the collector-core framework.
//!
//! This module provides configuration structures and validation for the
//! collector runtime and event sources. It integrates with the existing
//! daemoneye-lib ConfigLoader for hierarchical configuration management.

use daemoneye_eventbus::DaemoneyeBroker;
use daemoneye_lib::config::{Config as DaemonEyeConfig, ConfigError, ConfigLoader};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};

/// Configuration for the collector-core runtime.
///
/// This structure contains settings that control the behavior of the collector
/// runtime, including event processing, resource limits, and operational parameters.
///
/// # Examples
///
/// ```rust
/// use collector_core::CollectorConfig;
/// use std::time::Duration;
///
/// let config = CollectorConfig {
///     max_event_sources: 10,
///     event_buffer_size: 1000,
///     shutdown_timeout: Duration::from_secs(30),
///     health_check_interval: Duration::from_secs(60),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    /// Maximum number of event sources that can be registered
    pub max_event_sources: usize,

    /// Buffer size for the event channel between sources and runtime
    pub event_buffer_size: usize,

    /// Timeout for graceful shutdown of event sources
    pub shutdown_timeout: Duration,

    /// Interval for health checks of registered event sources
    pub health_check_interval: Duration,

    /// Maximum time to wait for event source startup
    pub startup_timeout: Duration,

    /// Enable detailed logging of event processing
    pub enable_debug_logging: bool,

    /// Maximum number of events to batch before processing
    pub max_batch_size: usize,

    /// Timeout for event batching (flush incomplete batches)
    pub batch_timeout: Duration,

    /// Backpressure threshold for event channel
    pub backpressure_threshold: usize,

    /// Maximum backpressure wait time before dropping events
    pub max_backpressure_wait: Duration,

    /// Enable telemetry collection
    pub enable_telemetry: bool,

    /// Telemetry collection interval
    pub telemetry_interval: Duration,

    /// Component name for configuration loading
    pub component_name: String,

    /// DaemoneyeEventBus socket path for embedded broker communication
    pub daemoneye_socket_path: Option<String>,

    /// IPC endpoint path for direct communication (e.g., with procmond)
    pub ipc_endpoint: Option<String>,

    /// Optional registration settings for broker auto-registration
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registration: Option<CollectorRegistrationConfig>,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            max_event_sources: 16,
            event_buffer_size: 1000,
            shutdown_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(60),
            startup_timeout: Duration::from_secs(10),
            enable_debug_logging: false,
            max_batch_size: 100,
            batch_timeout: Duration::from_millis(100),
            backpressure_threshold: 800, // 80% of buffer size
            max_backpressure_wait: Duration::from_millis(500),
            enable_telemetry: true,
            telemetry_interval: Duration::from_secs(30),
            component_name: "collector-core".to_string(),
            daemoneye_socket_path: None,
            ipc_endpoint: None,
            registration: None,
        }
    }
}

/// Registration configuration for automatic broker registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorRegistrationConfig {
    /// Enable broker registration for this collector.
    pub enabled: bool,
    /// Target control topic for registration RPC operations.
    pub topic: String,
    /// Optional override for collector identifier.
    #[serde(default)]
    pub collector_id: Option<String>,
    /// Optional override for collector type reported to the broker.
    #[serde(default)]
    pub collector_type: Option<String>,
    /// Interval between heartbeats reported to the registry.
    pub heartbeat_interval: Duration,
    /// Deadline for registration RPC operations.
    pub timeout: Duration,
    /// Number of retry attempts for registration failures.
    pub retry_attempts: u32,
    /// Additional metadata attributes reported during registration.
    #[serde(default)]
    pub attributes: HashMap<String, Value>,
    /// Handle to the embedded broker when operating in-process.
    #[serde(skip)]
    pub broker: Option<Arc<DaemoneyeBroker>>,
}

impl Default for CollectorRegistrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            topic: "control.collector.registration".to_string(),
            collector_id: None,
            collector_type: None,
            heartbeat_interval: Duration::from_secs(30),
            timeout: Duration::from_secs(10),
            retry_attempts: 3,
            attributes: HashMap::new(),
            broker: None,
        }
    }
}

impl CollectorRegistrationConfig {
    /// Attach a broker handle for in-process registration.
    pub fn with_broker(mut self, broker: Arc<DaemoneyeBroker>) -> Self {
        self.broker = Some(broker);
        self
    }
}

impl CollectorConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates the configuration and returns any errors.
    ///
    /// # Errors
    ///
    /// Returns an error if any configuration values are invalid or would
    /// cause operational issues.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.max_event_sources == 0 {
            anyhow::bail!("max_event_sources must be greater than 0");
        }

        if self.event_buffer_size == 0 {
            anyhow::bail!("event_buffer_size must be greater than 0");
        }

        if self.shutdown_timeout.is_zero() {
            anyhow::bail!("shutdown_timeout must be greater than 0");
        }

        if self.health_check_interval.is_zero() {
            anyhow::bail!("health_check_interval must be greater than 0");
        }

        if self.startup_timeout.is_zero() {
            anyhow::bail!("startup_timeout must be greater than 0");
        }

        if self.max_batch_size == 0 {
            anyhow::bail!("max_batch_size must be greater than 0");
        }

        if self.batch_timeout.is_zero() {
            anyhow::bail!("batch_timeout must be greater than 0");
        }

        if self.backpressure_threshold == 0 {
            anyhow::bail!("backpressure_threshold must be greater than 0");
        }

        if self.backpressure_threshold >= self.event_buffer_size {
            anyhow::bail!("backpressure_threshold must be less than event_buffer_size");
        }

        if self.max_backpressure_wait.is_zero() {
            anyhow::bail!("max_backpressure_wait must be greater than 0");
        }

        if self.telemetry_interval.is_zero() {
            anyhow::bail!("telemetry_interval must be greater than 0");
        }

        if self.component_name.is_empty() {
            anyhow::bail!("component_name cannot be empty");
        }

        if let Some(registration) = &self.registration
            && registration.enabled
        {
            if registration.topic.trim().is_empty() {
                anyhow::bail!("registration.topic cannot be empty");
            }

            if registration.timeout.is_zero() {
                anyhow::bail!("registration.timeout must be greater than 0");
            }

            if registration.heartbeat_interval.is_zero() {
                anyhow::bail!("registration.heartbeat_interval must be greater than 0");
            }
        }

        // Warn about potentially problematic configurations
        if self.event_buffer_size < 100 {
            tracing::warn!(
                "event_buffer_size is very small ({}), may cause backpressure",
                self.event_buffer_size
            );
        }

        if self.max_event_sources > 100 {
            tracing::warn!(
                "max_event_sources is very large ({}), may impact performance",
                self.max_event_sources
            );
        }

        if self.backpressure_threshold as f64 / self.event_buffer_size as f64 > 0.9 {
            tracing::warn!(
                "backpressure_threshold is very high ({}/{}), may not provide effective backpressure",
                self.backpressure_threshold,
                self.event_buffer_size
            );
        }

        Ok(())
    }

    /// Sets the maximum number of event sources.
    pub fn with_max_event_sources(mut self, max: usize) -> Self {
        self.max_event_sources = max;
        self
    }

    /// Sets the event buffer size.
    pub fn with_event_buffer_size(mut self, size: usize) -> Self {
        self.event_buffer_size = size;
        self
    }

    /// Sets the shutdown timeout.
    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Sets the health check interval.
    pub fn with_health_check_interval(mut self, interval: Duration) -> Self {
        self.health_check_interval = interval;
        self
    }

    /// Enables or disables debug logging.
    pub fn with_debug_logging(mut self, enabled: bool) -> Self {
        self.enable_debug_logging = enabled;
        self
    }

    /// Sets the component name for configuration loading.
    pub fn with_component_name(mut self, name: String) -> Self {
        self.component_name = name;
        self
    }

    /// Sets the IPC endpoint path.
    pub fn with_ipc_endpoint(mut self, path: String) -> Self {
        self.ipc_endpoint = Some(path);
        self
    }

    /// Sets the backpressure threshold.
    pub fn with_backpressure_threshold(mut self, threshold: usize) -> Self {
        self.backpressure_threshold = threshold;
        self
    }

    /// Enables or disables telemetry collection.
    pub fn with_telemetry(mut self, enabled: bool) -> Self {
        self.enable_telemetry = enabled;
        self
    }

    /// Sets the telemetry collection interval.
    pub fn with_telemetry_interval(mut self, interval: Duration) -> Self {
        self.telemetry_interval = interval;
        self
    }

    /// Sets the maximum backpressure wait time.
    pub fn with_max_backpressure_wait(mut self, wait: Duration) -> Self {
        self.max_backpressure_wait = wait;
        self
    }

    /// Sets the maximum batch size.
    pub fn with_max_batch_size(mut self, size: usize) -> Self {
        self.max_batch_size = size;
        self
    }

    /// Sets the batch timeout.
    pub fn with_batch_timeout(mut self, timeout: Duration) -> Self {
        self.batch_timeout = timeout;
        self
    }

    /// Sets the registration configuration.
    pub fn with_registration(mut self, registration: CollectorRegistrationConfig) -> Self {
        self.registration = Some(registration);
        self
    }

    /// Assigns a broker handle for registration operations.
    pub fn set_registration_broker(&mut self, broker: Arc<DaemoneyeBroker>) {
        if let Some(config) = self.registration.as_mut() {
            config.broker = Some(broker);
        }
    }

    /// Loads configuration from the existing daemoneye-lib ConfigLoader.
    ///
    /// This method integrates with the hierarchical configuration system
    /// used by other DaemonEye components, applying overrides from:
    /// 1. System configuration files
    /// 2. User configuration files
    /// 3. Environment variables
    ///
    /// # Arguments
    ///
    /// * `component_name` - Component name for environment variable prefixes
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading or validation fails.
    pub fn load_from_daemoneye_config(component_name: &str) -> Result<Self, Box<ConfigError>> {
        let config_loader = ConfigLoader::new(component_name);
        let daemoneye_config = config_loader.load()?;

        let mut collector_config = Self::default().with_component_name(component_name.to_string());

        // Apply daemoneye-lib configuration overrides
        collector_config = collector_config.apply_daemoneye_config(&daemoneye_config);

        // Validate the final configuration
        collector_config
            .validate()
            .map_err(|e| ConfigError::ValidationError {
                message: e.to_string(),
            })?;

        Ok(collector_config)
    }

    /// Applies configuration from daemoneye-lib Config structure.
    ///
    /// This method maps relevant fields from the daemoneye-lib configuration
    /// to collector-core configuration fields.
    fn apply_daemoneye_config(mut self, config: &DaemonEyeConfig) -> Self {
        // Map app configuration
        self.max_batch_size = config.app.batch_size;

        // Calculate event buffer size based on batch size
        self.event_buffer_size = (config.app.batch_size * 10).max(1000);

        // Calculate backpressure threshold as 80% of buffer size
        self.backpressure_threshold = (self.event_buffer_size * 80) / 100;

        // Map logging configuration
        self.enable_debug_logging =
            config.logging.level == "debug" || config.logging.level == "trace";

        // Apply environment-specific overrides
        self.apply_env_overrides()
    }

    /// Applies environment variable overrides specific to collector-core.
    ///
    /// This method reads collector-core specific environment variables
    /// using the component name as a prefix.
    pub fn apply_env_overrides(mut self) -> Self {
        let prefix = format!("{}_COLLECTOR", self.component_name.to_uppercase());

        if let Ok(val) = std::env::var(format!("{prefix}_MAX_EVENT_SOURCES"))
            && let Ok(max_sources) = val.parse()
        {
            self.max_event_sources = max_sources;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_EVENT_BUFFER_SIZE"))
            && let Ok(buffer_size) = val.parse()
        {
            self.event_buffer_size = buffer_size;
            // Recalculate backpressure threshold
            self.backpressure_threshold = (buffer_size * 80) / 100;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_ENABLE_TELEMETRY"))
            && let Ok(enabled) = val.parse()
        {
            self.enable_telemetry = enabled;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_DEBUG_LOGGING"))
            && let Ok(enabled) = val.parse()
        {
            self.enable_debug_logging = enabled;
        }

        if let Ok(val) = std::env::var(format!("{prefix}_IPC_ENDPOINT")) {
            self.ipc_endpoint = Some(val);
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_default_config() {
        let config = CollectorConfig::default();

        assert_eq!(config.max_event_sources, 16);
        assert_eq!(config.event_buffer_size, 1000);
        assert_eq!(config.shutdown_timeout, Duration::from_secs(30));
        assert_eq!(config.health_check_interval, Duration::from_secs(60));
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert!(!config.enable_debug_logging);
        assert_eq!(config.max_batch_size, 100);
        assert_eq!(config.batch_timeout, Duration::from_millis(100));
        assert_eq!(config.backpressure_threshold, 800);
        assert_eq!(config.max_backpressure_wait, Duration::from_millis(500));
        assert!(config.enable_telemetry);
        assert_eq!(config.telemetry_interval, Duration::from_secs(30));
        assert_eq!(config.component_name, "collector-core");
    }

    #[test]
    fn test_config_validation_success() {
        let config = CollectorConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_failures() {
        // Test max_event_sources validation
        let config = CollectorConfig {
            max_event_sources: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test event_buffer_size validation
        let config = CollectorConfig {
            event_buffer_size: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test shutdown_timeout validation
        let config = CollectorConfig {
            shutdown_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test health_check_interval validation
        let config = CollectorConfig {
            health_check_interval: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test startup_timeout validation
        let config = CollectorConfig {
            startup_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test max_batch_size validation
        let config = CollectorConfig {
            max_batch_size: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test batch_timeout validation
        let config = CollectorConfig {
            batch_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test backpressure_threshold validation
        let config = CollectorConfig {
            backpressure_threshold: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test backpressure_threshold >= event_buffer_size validation
        let config = CollectorConfig {
            backpressure_threshold: 1000,
            event_buffer_size: 1000,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test max_backpressure_wait validation
        let config = CollectorConfig {
            max_backpressure_wait: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test telemetry_interval validation
        let config = CollectorConfig {
            telemetry_interval: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Test component_name validation
        let config = CollectorConfig {
            component_name: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder_methods() {
        let config = CollectorConfig::new()
            .with_max_event_sources(32)
            .with_event_buffer_size(2000)
            .with_shutdown_timeout(Duration::from_secs(60))
            .with_health_check_interval(Duration::from_secs(120))
            .with_debug_logging(true);

        assert_eq!(config.max_event_sources, 32);
        assert_eq!(config.event_buffer_size, 2000);
        assert_eq!(config.shutdown_timeout, Duration::from_secs(60));
        assert_eq!(config.health_check_interval, Duration::from_secs(120));
        assert!(config.enable_debug_logging);
    }

    #[test]
    fn test_config_serialization() {
        let config = CollectorConfig::default();

        // Test serialization to JSON
        let json = serde_json::to_string(&config).expect("Failed to serialize config");
        assert!(json.contains("max_event_sources"));
        assert!(json.contains("event_buffer_size"));

        // Test deserialization from JSON
        let deserialized: CollectorConfig =
            serde_json::from_str(&json).expect("Failed to deserialize config");
        assert_eq!(deserialized.max_event_sources, config.max_event_sources);
        assert_eq!(deserialized.event_buffer_size, config.event_buffer_size);
    }

    #[test]
    fn test_config_builder_methods_extended() {
        let config = CollectorConfig::new()
            .with_component_name("test-component".to_string())
            .with_backpressure_threshold(500)
            .with_telemetry(false);

        assert_eq!(config.component_name, "test-component");
        assert_eq!(config.backpressure_threshold, 500);
        assert!(!config.enable_telemetry);
    }

    #[test]
    fn test_apply_env_overrides() {
        // Note: This test doesn't actually set environment variables
        // due to unsafe_code = "forbid" constraint
        let config = CollectorConfig::default()
            .with_component_name("test".to_string())
            .apply_env_overrides();

        // Should return unchanged config since no env vars are set
        assert_eq!(config.component_name, "test");
    }

    #[test]
    fn test_apply_daemoneye_config() {
        let daemoneye_config = DaemonEyeConfig {
            app: daemoneye_lib::config::AppConfig {
                batch_size: 2000,
                ..Default::default()
            },
            logging: daemoneye_lib::config::LoggingConfig {
                level: "debug".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = CollectorConfig::default().apply_daemoneye_config(&daemoneye_config);

        assert_eq!(config.max_batch_size, 2000);
        assert_eq!(config.event_buffer_size, 20000); // batch_size * 10
        assert_eq!(config.backpressure_threshold, 16000); // 80% of buffer size
        assert!(config.enable_debug_logging);
    }
}
