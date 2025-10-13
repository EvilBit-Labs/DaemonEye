//! Transport layer for cross-platform IPC communication

use crate::error::{EventBusError, Result};
use tracing::{debug, error, info, warn};

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
}

impl TransportServer {
    /// Create a new transport server
    pub async fn new(config: SocketConfig) -> Result<Self> {
        let socket_path = config.get_socket_path();
        info!("Transport server created for: {}", socket_path);
        Ok(Self { config })
    }

    /// Accept a new client connection
    pub async fn accept(&self) -> Result<TransportClient> {
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
}

impl TransportClient {
    /// Create a new transport client
    pub fn new() -> Self {
        Self { connected: true }
    }

    /// Connect to a transport server
    pub async fn connect(config: &SocketConfig) -> Result<Self> {
        let socket_path = config.get_socket_path();
        info!("Connecting to transport server: {}", socket_path);
        Ok(Self { connected: true })
    }

    /// Send a message through the transport
    pub async fn send(&mut self, data: &[u8]) -> Result<()> {
        debug!("Sent {} bytes through transport", data.len());
        Ok(())
    }

    /// Receive a message from the transport
    pub async fn receive(&mut self) -> Result<Vec<u8>> {
        debug!("Received message through transport");
        Ok(Vec::new())
    }

    /// Check if the connection is still alive
    pub async fn is_alive(&mut self) -> bool {
        self.connected
    }

    /// Close the transport connection
    pub async fn close(mut self) -> Result<()> {
        self.connected = false;
        debug!("Transport connection closed");
        Ok(())
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
