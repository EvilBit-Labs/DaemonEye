//! Configuration for the RPC service handler.

use super::PROCMOND_CONTROL_TOPIC;
use std::time::Duration;

/// Default timeout for RPC operations in seconds.
const DEFAULT_RPC_TIMEOUT_SECS: u64 = 30;

/// Configuration for the RPC service handler.
#[derive(Debug, Clone)]
pub struct RpcServiceConfig {
    /// Collector identifier.
    pub collector_id: String,
    /// Control topic to subscribe to.
    pub control_topic: String,
    /// Response topic prefix.
    pub response_topic_prefix: String,
    /// Default operation timeout.
    pub default_timeout: Duration,
    /// Maximum concurrent RPC requests.
    pub max_concurrent_requests: usize,
}

impl Default for RpcServiceConfig {
    fn default() -> Self {
        Self {
            collector_id: "procmond".to_owned(),
            control_topic: PROCMOND_CONTROL_TOPIC.to_owned(),
            response_topic_prefix: "control.rpc.response".to_owned(),
            default_timeout: Duration::from_secs(DEFAULT_RPC_TIMEOUT_SECS),
            max_concurrent_requests: 10,
        }
    }
}
