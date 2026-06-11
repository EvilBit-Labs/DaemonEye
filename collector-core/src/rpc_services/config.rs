//! Configuration for the RPC service manager.

use std::time::Duration;

/// Topic prefix the agent uses when targeting a collector by ID.
const COLLECTOR_TOPIC_PREFIX: &str = "control.collector.";

/// Configuration for the RPC service manager
///
/// # Invariant
///
/// `rpc_topic` must be consistent with `collector_id`: the agent builds the
/// target topic as `control.collector.{collector_id}`, so `rpc_topic` must
/// equal that value. The fields are public for ergonomic construction; call
/// [`RpcServiceConfig::validate`] after building a config from independent
/// values to ensure the two have not drifted apart.
#[derive(Debug, Clone)]
pub struct RpcServiceConfig {
    /// Collector identifier
    pub collector_id: String,
    /// RPC topic to subscribe to
    pub rpc_topic: String,
    /// Default timeout for RPC operations
    pub default_timeout: Duration,
}

impl RpcServiceConfig {
    /// Builds the canonical RPC topic for a collector ID.
    ///
    /// This mirrors how the agent derives the target topic
    /// (`control.collector.{collector_id}`).
    pub fn topic_for_collector(collector_id: &str) -> String {
        format!("{COLLECTOR_TOPIC_PREFIX}{collector_id}")
    }

    /// Validates that `rpc_topic` is consistent with `collector_id`.
    ///
    /// Because both fields are public and can be set independently, a caller
    /// can accidentally leave `rpc_topic` pointing at a different collector than
    /// `collector_id`. The agent routes by `control.collector.{collector_id}`,
    /// so a drifted `rpc_topic` means the collector would subscribe to the
    /// wrong topic and never receive its RPC requests.
    ///
    /// # Errors
    ///
    /// Returns an error if `collector_id` is empty or if `rpc_topic` does not
    /// equal `control.collector.{collector_id}`.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.collector_id.is_empty() {
            anyhow::bail!("collector_id must be non-empty");
        }

        let expected_topic = Self::topic_for_collector(&self.collector_id);
        if self.rpc_topic != expected_topic {
            anyhow::bail!(
                "rpc_topic '{}' is inconsistent with collector_id '{}' (expected '{}')",
                self.rpc_topic,
                self.collector_id,
                expected_topic
            );
        }

        Ok(())
    }
}

impl Default for RpcServiceConfig {
    fn default() -> Self {
        Self {
            collector_id: "collector".to_owned(),
            rpc_topic: "control.collector.collector".to_owned(),
            default_timeout: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_consistent() {
        assert!(RpcServiceConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_accepts_consistent_topic() {
        let config = RpcServiceConfig {
            collector_id: "process".to_owned(),
            rpc_topic: RpcServiceConfig::topic_for_collector("process"),
            default_timeout: Duration::from_secs(30),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_drifted_topic() {
        let config = RpcServiceConfig {
            collector_id: "process".to_owned(),
            // Topic points at a different collector than collector_id.
            rpc_topic: "control.collector.network".to_owned(),
            default_timeout: Duration::from_secs(30),
        };
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("inconsistent"), "unexpected error: {error}");
    }

    #[test]
    fn validate_rejects_empty_collector_id() {
        let config = RpcServiceConfig {
            collector_id: String::new(),
            rpc_topic: "control.collector.".to_owned(),
            default_timeout: Duration::from_secs(30),
        };
        assert!(config.validate().is_err());
    }
}
