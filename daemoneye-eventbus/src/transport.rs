//! Transport layer for cross-platform IPC communication

use crate::error::{EventBusError, Result};
use tracing::{debug, error, info};

/// Cross-platform socket path configuration
#[derive(Debug, Clone)]
pub struct SocketConfig {
    /// Socket path for Unix systems
    pub unix_path: String,
    /// Named pipe name for Windows
    pub windows_pipe: String,
}

impl SocketConfig {
    /// Create a new socket configuration
    pub fn new(instance_id: &str) -> Self {
        Self {
            unix_path: format!("/tmp/daemoneye-{}.sock", instance_id),
            windows_pipe: format!(r"\\.\pipe\DaemonEye-{}", instance_id),
        }
    }

    /// Get the appropriate socket path for the current platform
    pub fn get_socket_path(&self) -> String {
        #[cfg(unix)]
        {
            self.unix_path.clone()
        }
        #[cfg(windows)]
        {
            self.windows_pipe.clone()
        }
    }
}

/// Transport server for accepting client connections
pub struct TransportServer {
    config: SocketConfig,
    accept_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl TransportServer {
    /// Create a new transport server
    pub async fn new(config: SocketConfig) -> Result<Self> {
        let socket_path = config.get_socket_path();
        info!("Transport server created for: {}", socket_path);
        Ok(Self {
            config,
            accept_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Accept a new client connection
    pub async fn accept(&self) -> Result<TransportClient> {
        // Simulate blocking accept with proper async sleep
        // In a real implementation, this would be a real async accept on a socket/pipe
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let count = self
            .accept_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Simulate connection limit - stop accepting after a reasonable number
        if count >= 100 {
            return Err(EventBusError::transport(
                "Server shutdown or connection limit reached",
            ));
        }

        debug!("Accepting new client connection");
        Ok(TransportClient::new())
    }

    /// Get the socket configuration
    pub fn config(&self) -> &SocketConfig {
        &self.config
    }
}

/// Transport client for connecting to the server
pub struct TransportClient {
    connected: bool,
    buffer: Vec<u8>,
}

impl TransportClient {
    /// Create a new transport client
    pub fn new() -> Self {
        Self {
            connected: true,
            buffer: Vec::new(),
        }
    }

    /// Connect to a transport server
    pub async fn connect(config: &SocketConfig) -> Result<Self> {
        let socket_path = config.get_socket_path();
        info!("Connecting to transport server: {}", socket_path);
        Ok(Self {
            connected: true,
            buffer: Vec::new(),
        })
    }

    /// Send a message through the transport
    pub async fn send(&mut self, data: &[u8]) -> Result<()> {
        if !self.connected {
            return Err(EventBusError::transport("Transport connection is closed"));
        }

        // Simulate async write operation
        tokio::task::yield_now().await;

        // In a real implementation, this would write to a Unix socket or named pipe
        debug!("Sent {} bytes through transport", data.len());
        Ok(())
    }

    /// Receive a message from the transport
    pub async fn receive(&mut self) -> Result<Vec<u8>> {
        if !self.connected {
            return Err(EventBusError::transport("Transport connection is closed"));
        }

        // Simulate async read operation
        tokio::task::yield_now().await;

        // In a real implementation, this would read from a Unix socket or named pipe
        if self.buffer.is_empty() {
            return Err(EventBusError::transport("No data available"));
        }

        debug!("Received message through transport");
        Ok(std::mem::take(&mut self.buffer))
    }

    /// Check if the connection is still alive
    pub async fn is_alive(&mut self) -> bool {
        self.connected
    }

    /// Close the transport connection
    pub async fn close(mut self) -> Result<()> {
        if !self.connected {
            return Ok(());
        }

        self.connected = false;
        self.buffer.clear();

        // Simulate async shutdown operation
        tokio::task::yield_now().await;

        debug!("Transport connection closed");
        Ok(())
    }
}

impl Default for TransportClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Transport connection manager for handling multiple clients
pub struct TransportManager {
    server: TransportServer,
    clients: Vec<TransportClient>,
}

impl TransportManager {
    /// Create a new transport manager
    pub async fn new(config: SocketConfig) -> Result<Self> {
        let server = TransportServer::new(config).await?;
        Ok(Self {
            server,
            clients: Vec::new(),
        })
    }

    /// Accept new clients
    pub async fn accept_clients(&mut self) -> Result<()> {
        loop {
            match self.server.accept().await {
                Ok(client) => {
                    info!(
                        "New client connected, total clients: {}",
                        self.clients.len() + 1
                    );
                    self.clients.push(client);
                }
                Err(e) => {
                    error!("Failed to accept client: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Get the number of connected clients
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Broadcast a message to all clients
    pub async fn broadcast(&mut self, data: &[u8]) -> Result<()> {
        let mut failed_clients = Vec::new();

        for (i, client) in self.clients.iter_mut().enumerate() {
            if let Err(e) = client.send(data).await {
                error!("Failed to send to client {}: {}", i, e);
                failed_clients.push(i);
            }
        }

        // Remove failed clients
        for &i in failed_clients.iter().rev() {
            self.clients.remove(i);
        }

        Ok(())
    }

    /// Close all connections
    pub async fn close_all(self) -> Result<()> {
        for client in self.clients {
            client.close().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_socket_config() {
        let config = SocketConfig::new("test");
        let path = config.get_socket_path();

        #[cfg(unix)]
        assert!(path.contains("daemoneye-test"));
        #[cfg(windows)]
        assert!(path.contains("DaemonEye-test"));
    }
}
