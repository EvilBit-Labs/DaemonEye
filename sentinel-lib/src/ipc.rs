//! IPC communication support for interprocess transport.
//!
//! This module provides codec functionality for secure, efficient communication
//! between procmond and sentinelagent using the interprocess crate with
//! protobuf message framing and CRC32 integrity validation.

pub mod codec;
pub mod interprocess_transport;

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
    /// CRC32 variant for integrity validation
    pub crc32_variant: Crc32Variant,
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
            crc32_variant: Crc32Variant::Ieee,
        }
    }
}

/// Transport type selection
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportType {
    /// Use tokio native IPC transport (default)
    Interprocess,
}

/// CRC32 variant for integrity validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crc32Variant {
    /// IEEE 802.3 CRC32 (default)
    Ieee,
    /// Castagnoli CRC32C
    Castagnoli,
}

/// Get the default endpoint path based on the platform
fn default_endpoint_path() -> String {
    #[cfg(unix)]
    {
        "/var/run/sentineld/procmond.sock".to_string()
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\sentineld\procmond".to_string()
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
        assert_eq!(config.crc32_variant, Crc32Variant::Ieee);
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
