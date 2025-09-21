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
            path: "/var/run/daemoneye/procmond.sock".to_string(),
            max_connections: 10,
            connection_timeout_secs: 30,
            message_timeout_secs: 60,
        }
    }
}

/// Create the secure directory for IPC sockets with appropriate permissions
fn ensure_secure_directory(socket_path: &str) -> IpcResult<()> {
    use std::fs;
    use std::path::Path;

    let socket_path = Path::new(socket_path);
    if let Some(parent_dir) = socket_path.parent() {
        // Create parent directory if it doesn't exist
        if !parent_dir.exists() {
            fs::create_dir_all(parent_dir).map_err(|e| {
                IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Failed to create directory {}: {}", parent_dir.display(), e),
                ))
            })?;
        }

        // Set secure permissions on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o750); // rwxr-x--- (owner: rwx, group: r-x, other: ---)
            fs::set_permissions(parent_dir, perms).map_err(|e| {
                IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Failed to set directory permissions for {}: {}",
                        parent_dir.display(),
                        e
                    ),
                ))
            })?;
        }
    }

    Ok(())
}

/// Create an IPC server using the interprocess transport
pub fn create_ipc_server(config: IpcConfig) -> IpcResult<daemoneye_lib::ipc::InterprocessServer> {
    use daemoneye_lib::ipc::{IpcConfig as LibIpcConfig, PanicStrategy, TransportType};

    // Ensure the secure directory exists with proper permissions
    ensure_secure_directory(&config.path)?;

    let lib_config = LibIpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: config.path,
        max_frame_bytes: 1024 * 1024, // 1MB
        accept_timeout_ms: config.connection_timeout_secs * 1000,
        read_timeout_ms: config.message_timeout_secs * 1000,
        write_timeout_ms: config.message_timeout_secs * 1000,
        max_connections: config.max_connections,
        panic_strategy: PanicStrategy::Abort, // Production configuration
    };

    Ok(daemoneye_lib::ipc::InterprocessServer::new(lib_config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_config_default() {
        let config = IpcConfig::default();
        assert_eq!(config.path, "/var/run/daemoneye/procmond.sock");
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
