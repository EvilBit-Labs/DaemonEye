//! IPC server trait definition and common functionality.

use crate::ipc::{IpcConfig, IpcResult, SimpleMessageHandler};
use async_trait::async_trait;

/// Trait for IPC server implementations
#[async_trait]
pub trait IpcServer: Send + Sync {
    /// Start the IPC server
    async fn start(&mut self) -> IpcResult<()>;

    /// Stop the IPC server gracefully
    async fn stop(&mut self) -> IpcResult<()>;

    /// Check if the server is currently running
    #[allow(dead_code)]
    fn is_running(&self) -> bool;

    /// Get the server configuration
    #[allow(dead_code)]
    fn config(&self) -> &IpcConfig;

    /// Set the message handler for processing incoming messages
    fn set_handler(&mut self, handler: SimpleMessageHandler);

    /// Get server statistics
    #[allow(dead_code)]
    async fn get_stats(&self) -> IpcResult<ServerStats>;
}

/// Server statistics for monitoring and debugging
#[derive(Debug, Clone, Default)]
pub struct ServerStats {
    /// Number of active connections
    pub active_connections: usize,
    /// Total number of connections handled
    pub total_connections: u64,
    /// Total number of messages processed
    pub messages_processed: u64,
    /// Total number of errors encountered
    pub errors_encountered: u64,
    /// Server uptime in seconds
    #[allow(dead_code)]
    pub uptime_seconds: u64,
    /// Last activity timestamp
    pub last_activity: Option<std::time::SystemTime>,
}

impl ServerStats {
    /// Create new server statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Update statistics with a new connection
    pub fn record_connection(&mut self) {
        self.active_connections += 1;
        self.total_connections += 1;
        self.last_activity = Some(std::time::SystemTime::now());
    }

    /// Update statistics when a connection is closed
    pub fn record_disconnection(&mut self) {
        if self.active_connections > 0 {
            self.active_connections -= 1;
        }
    }

    /// Update statistics with a processed message
    pub fn record_message(&mut self) {
        self.messages_processed += 1;
        self.last_activity = Some(std::time::SystemTime::now());
    }

    /// Update statistics with an error
    pub fn record_error(&mut self) {
        self.errors_encountered += 1;
    }
}

/// Common server functionality that can be shared across implementations
#[derive(Debug)]
pub struct ServerCommon {
    config: IpcConfig,
    stats: ServerStats,
    start_time: Option<std::time::SystemTime>,
}

impl ServerCommon {
    /// Create a new server common instance
    pub fn new(config: IpcConfig) -> Self {
        Self {
            config,
            stats: ServerStats::new(),
            start_time: None,
        }
    }

    /// Get the server configuration
    pub fn config(&self) -> &IpcConfig {
        &self.config
    }

    /// Get mutable access to statistics
    pub fn stats_mut(&mut self) -> &mut ServerStats {
        &mut self.stats
    }

    /// Get immutable access to statistics
    pub fn stats(&self) -> &ServerStats {
        &self.stats
    }

    /// Record server start
    pub fn record_start(&mut self) {
        self.start_time = Some(std::time::SystemTime::now());
    }

    /// Check if server is running
    pub fn is_running(&self) -> bool {
        self.start_time.is_some()
    }

    /// Get server uptime
    #[allow(dead_code)]
    pub fn uptime(&self) -> Option<std::time::Duration> {
        self.start_time
            .and_then(|start| std::time::SystemTime::now().duration_since(start).ok())
    }

    /// Update statistics with uptime
    #[allow(dead_code)]
    pub fn update_uptime(&mut self) {
        if let Some(duration) = self.uptime() {
            self.stats.uptime_seconds = duration.as_secs();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_stats_creation() {
        let stats = ServerStats::new();
        assert_eq!(stats.active_connections, 0);
        assert_eq!(stats.total_connections, 0);
        assert_eq!(stats.messages_processed, 0);
        assert_eq!(stats.errors_encountered, 0);
        assert_eq!(stats.uptime_seconds, 0);
        assert!(stats.last_activity.is_none());
    }

    #[test]
    fn test_server_stats_recording() {
        let mut stats = ServerStats::new();

        // Record connection
        stats.record_connection();
        assert_eq!(stats.active_connections, 1);
        assert_eq!(stats.total_connections, 1);
        assert!(stats.last_activity.is_some());

        // Record message
        stats.record_message();
        assert_eq!(stats.messages_processed, 1);

        // Record error
        stats.record_error();
        assert_eq!(stats.errors_encountered, 1);

        // Record disconnection
        stats.record_disconnection();
        assert_eq!(stats.active_connections, 0);
    }

    #[test]
    fn test_server_common_creation() {
        let config = IpcConfig::default();
        let common = ServerCommon::new(config);

        assert!(!common.is_running());
        assert_eq!(common.stats().active_connections, 0);
    }

    #[test]
    fn test_server_common_lifecycle() {
        let config = IpcConfig::default();
        let mut common = ServerCommon::new(config);

        // Initially not running
        assert!(!common.is_running());
        assert!(common.uptime().is_none());

        // Record start
        common.record_start();
        assert!(common.is_running());
        assert!(common.uptime().is_some());

        // Update uptime
        common.update_uptime();
        // uptime_seconds is a u64, so it's always >= 0
        let _uptime = common.stats().uptime_seconds;
    }

    #[test]
    fn test_server_common_stats_mutation() {
        let config = IpcConfig::default();
        let mut common = ServerCommon::new(config);

        // Record some activity
        common.stats_mut().record_connection();
        common.stats_mut().record_message();
        common.stats_mut().record_error();

        assert_eq!(common.stats().active_connections, 1);
        assert_eq!(common.stats().total_connections, 1);
        assert_eq!(common.stats().messages_processed, 1);
        assert_eq!(common.stats().errors_encountered, 1);
    }
}
