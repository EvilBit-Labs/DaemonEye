//! Statistics tracking for the RPC service.

/// Statistics for the RPC service.
#[derive(Debug, Clone, Default)]
pub struct RpcServiceStats {
    /// Total requests received.
    pub requests_received: u64,
    /// Successful requests.
    pub requests_succeeded: u64,
    /// Failed requests.
    pub requests_failed: u64,
    /// Timed out requests.
    pub requests_timed_out: u64,
    /// Health check requests.
    pub health_checks: u64,
    /// Config update requests.
    pub config_updates: u64,
    /// Shutdown requests.
    pub shutdown_requests: u64,
    /// Failed response publishes.
    pub responses_failed: u64,
}
