//! Configuration for the registration manager.

use super::{
    DEFAULT_HEARTBEAT_INTERVAL_SECS, DEFAULT_REGISTRATION_TIMEOUT_SECS, MAX_REGISTRATION_RETRIES,
};
use std::collections::HashMap;
use std::time::Duration;

/// Configuration for the registration manager.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Collector identifier.
    pub collector_id: String,
    /// Collector type (e.g., "process-monitor").
    pub collector_type: String,
    /// Software version.
    pub version: String,
    /// Declared capabilities.
    pub capabilities: Vec<String>,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Registration timeout.
    pub registration_timeout: Duration,
    /// Maximum registration retries.
    pub max_retries: u32,
    /// Additional attributes.
    pub attributes: HashMap<String, serde_json::Value>,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            collector_id: "procmond".to_owned(),
            collector_type: "process-monitor".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: vec![
                "process-collection".to_owned(),
                "lifecycle-tracking".to_owned(),
                "enhanced-metadata".to_owned(),
                "executable-hashing".to_owned(),
            ],
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            registration_timeout: Duration::from_secs(DEFAULT_REGISTRATION_TIMEOUT_SECS),
            max_retries: MAX_REGISTRATION_RETRIES,
            attributes: HashMap::new(),
        }
    }
}
