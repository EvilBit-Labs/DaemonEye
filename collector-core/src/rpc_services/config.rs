//! Configuration for the RPC service manager.

use std::time::Duration;

/// Configuration for the RPC service manager
#[derive(Debug, Clone)]
pub struct RpcServiceConfig {
    /// Collector identifier
    pub collector_id: String,
    /// RPC topic to subscribe to
    pub rpc_topic: String,
    /// Default timeout for RPC operations
    pub default_timeout: Duration,
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
