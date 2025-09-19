//! IPC (Inter-Process Communication) module for procmond.
//!
//! This module provides the server-side IPC implementation for communication
//! between procmond and sentinelagent using the interprocess crate for
//! cross-platform support.

pub mod error;
pub mod protocol;

// Re-export commonly used types
pub use error::{IpcError, IpcResult};

/// Configuration for IPC server setup
#[derive(Debug, Clone)]
pub struct IpcConfig {
    /// Path for Unix socket or named pipe
    pub path: String,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Connection timeout in seconds
    #[allow(dead_code)]
    pub connection_timeout_secs: u64,
    /// Message timeout in seconds
    #[allow(dead_code)]
    pub message_timeout_secs: u64,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            path: "/tmp/sentineld-procmond.sock".to_string(),
            max_connections: 10,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        }
    }
}

/// Create an IPC server using the interprocess transport
pub fn create_ipc_server(config: IpcConfig) -> IpcResult<sentinel_lib::ipc::InterprocessServer> {
    use sentinel_lib::ipc::{IpcConfig as LibIpcConfig, TransportType};

    let lib_config = LibIpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: config.path,
        max_frame_bytes: 1024 * 1024, // 1MB
        accept_timeout_ms: config.connection_timeout_secs * 1000,
        read_timeout_ms: config.message_timeout_secs * 1000,
        write_timeout_ms: config.message_timeout_secs * 1000,
        max_connections: config.max_connections,
        crc32_variant: sentinel_lib::ipc::Crc32Variant::Ieee,
    };

    sentinel_lib::ipc::InterprocessServer::new(lib_config).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_config_default() {
        let config = IpcConfig::default();
        assert_eq!(config.path, "/tmp/sentineld-procmond.sock");
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.connection_timeout_secs, 30);
        assert_eq!(config.message_timeout_secs, 60);
    }

    #[test]
    fn test_ipc_config_custom() {
        let config = IpcConfig {
            path: "/custom/path.sock".to_string(),
            max_connections: 5,
            connection_timeout_secs: 15,
            message_timeout_secs: 30,
        };

        assert_eq!(config.path, "/custom/path.sock");
        assert_eq!(config.max_connections, 5);
        assert_eq!(config.connection_timeout_secs, 15);
        assert_eq!(config.message_timeout_secs, 30);
    }
}
