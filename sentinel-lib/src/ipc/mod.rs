//! IPC communication support for interprocess transport.
//!
//! This module provides codec functionality for secure, efficient communication
//! between procmond and sentinelagent using the interprocess crate with
//! protobuf message framing and CRC32 integrity validation.

pub mod client;
pub mod codec;
pub mod interprocess_transport;

pub use client::{ClientStats, ConnectionState, ResilientIpcClient};
/// Re-export commonly used types
pub use codec::{IpcCodec, IpcError, IpcResult};
pub use interprocess_transport::{InterprocessClient, InterprocessServer};

/// IPC configuration for transport layer
#[derive(Debug, Clone)]
pub struct IpcConfig {
    /// Transport type selection
    pub transport: TransportType,
    /// Endpoint path (Unix socket path or Windows pipe name)
    pub endpoint_path: String,
    /// Maximum frame size in bytes (default 1MB)
    pub max_frame_bytes: usize,
    /// Accept timeout in milliseconds
    pub accept_timeout_ms: u64,
    /// Read timeout in milliseconds
    pub read_timeout_ms: u64,
    /// Write timeout in milliseconds
    pub write_timeout_ms: u64,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Panic strategy for production environments
    pub panic_strategy: PanicStrategy,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            transport: TransportType::Interprocess,
            endpoint_path: default_endpoint_path(),
            max_frame_bytes: 1024 * 1024, // 1MB
            accept_timeout_ms: 5000,      // 5 seconds
            read_timeout_ms: 30000,       // 30 seconds
            write_timeout_ms: 10000,      // 10 seconds
            max_connections: 16,
            panic_strategy: PanicStrategy::Unwind, // Default to unwind for development
        }
    }
}

/// Transport type selection
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportType {
    /// Use tokio native IPC transport (default)
    Interprocess,
}

/// Panic strategy for production environments
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PanicStrategy {
    /// Allow panics to unwind (default for development)
    Unwind,
    /// Abort on panic (recommended for production)
    Abort,
}

impl IpcConfig {
    /// Create a production-ready configuration with abort-on-panic strategy
    pub fn production() -> Self {
        Self {
            panic_strategy: PanicStrategy::Abort,
            ..Self::default()
        }
    }

    /// Configure the panic strategy
    #[must_use]
    pub const fn with_panic_strategy(mut self, strategy: PanicStrategy) -> Self {
        self.panic_strategy = strategy;
        self
    }
}

/// Get the default endpoint path based on the platform
fn default_endpoint_path() -> String {
    #[cfg(unix)]
    {
        "/var/run/sentineld/procmond.sock".to_owned()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\sentineld\procmond".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IpcConfig::default();
        assert_eq!(config.transport, TransportType::Interprocess);
        assert_eq!(config.max_frame_bytes, 1024 * 1024);
        assert_eq!(config.max_connections, 16);
    }

    #[test]
    fn test_default_endpoint_path() {
        let path = default_endpoint_path();
        #[cfg(unix)]
        assert!(path.contains("procmond.sock"));
        #[cfg(windows)]
        assert!(path.contains(r"\\.\pipe\"));
    }
}
