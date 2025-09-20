//! Configuration management for the collector-core framework.
//!
//! This module provides configuration structures and validation for the
//! collector runtime and event sources.

use serde::{Deserialize, Serialize};
use std::time::Duration;

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
        }
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
}
