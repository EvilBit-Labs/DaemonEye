//! Transport layer for cross-platform IPC communication
//!
//! This module provides client connection management, topic-based routing,
//! and health monitoring for the daemoneye-eventbus system.

use crate::error::{EventBusError, Result};
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{ListenerOptions, Name};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, timeout};

#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use tracing::{debug, error, info, warn};

/// Create a socket name from a path string, handling platform differences
fn create_socket_name(
    socket_path: &str,
) -> std::result::Result<Name<'_>, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::path::Path;
        let path = Path::new(socket_path);
        Ok(path.to_fs_name::<GenericFilePath>()?)
    }
    #[cfg(windows)]
    {
        Ok(socket_path.to_ns_name::<GenericNamespaced>()?)
    }
}

/// Cross-platform socket path configuration
#[derive(Debug, Clone)]
pub struct SocketConfig {
    /// Socket path for Unix systems
    pub unix_path: String,
    /// Named pipe name for Windows
    pub windows_pipe: String,
    /// Maximum number of concurrent connections
    pub connection_limit: usize,
}

impl SocketConfig {
    /// Create a new socket configuration
    pub fn new(instance_id: &str) -> Self {
        Self {
            unix_path: format!("/tmp/daemoneye-{}.sock", instance_id),
            windows_pipe: format!(r"\\.\pipe\DaemonEye-{}", instance_id),
            connection_limit: 100, // Default connection limit
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

/// Client connection configuration for reconnection and health monitoring
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Maximum number of reconnection attempts
    pub max_reconnect_attempts: u32,
    /// Initial reconnection delay
    pub initial_reconnect_delay: Duration,
    /// Maximum reconnection delay
    pub max_reconnect_delay: Duration,
    /// Exponential backoff multiplier
    pub backoff_multiplier: f64,
    /// Connection timeout
    pub connection_timeout: Duration,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Maximum time to wait for health check response
    pub health_check_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_reconnect_attempts: 10,
            initial_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            connection_timeout: Duration::from_secs(5),
            health_check_interval: Duration::from_secs(30),
            health_check_timeout: Duration::from_secs(5),
        }
    }
}

/// Transport server for accepting client connections
pub struct TransportServer {
    config: SocketConfig,
    listener: Option<LocalSocketListener>,
    accept_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl TransportServer {
    /// Create a new transport server
    pub async fn new(config: SocketConfig) -> Result<Self> {
        let socket_path = config.get_socket_path();

        // Clean up any existing socket file
        #[cfg(unix)]
        if std::path::Path::new(&socket_path).exists()
            && let Err(e) = std::fs::remove_file(&socket_path)
        {
            warn!(
                "Failed to remove existing socket file {}: {}",
                socket_path, e
            );
        }

        // Create the listener using the appropriate name type
        let name = create_socket_name(&socket_path).map_err(|e| {
            EventBusError::transport(format!("Invalid socket name {}: {}", socket_path, e))
        })?;

        let opts = ListenerOptions::new().name(name);
        let listener = opts.create_tokio().map_err(|e| {
            EventBusError::transport(format!("Failed to bind to socket {}: {}", socket_path, e))
        })?;

        info!("Transport server created for: {}", socket_path);
        Ok(Self {
            config,
            listener: Some(listener),
            accept_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Accept a new client connection
    pub async fn accept(&self) -> Result<TransportClient> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| EventBusError::transport("Server not listening"))?;

        let count = self
            .accept_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Connection limit to prevent resource exhaustion
        if count >= self.config.connection_limit as u64 {
            return Err(EventBusError::transport(
                "Server shutdown or connection limit reached",
            ));
        }

        // Accept the connection
        let stream = listener
            .accept()
            .await
            .map_err(|e| EventBusError::transport(format!("Failed to accept connection: {}", e)))?;

        debug!("Accepted new client connection");
        Ok(TransportClient::from_stream(stream, self.config.clone()))
    }

    /// Get the socket configuration
    pub fn config(&self) -> &SocketConfig {
        &self.config
    }

    /// Shutdown the server
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Transport server shutdown");

        // Close the listener
        if let Some(_listener) = self.listener.take() {
            // Listener will be dropped here, closing the socket
            debug!("Closed transport server listener");
        }

        // Clean up socket file on Unix systems
        #[cfg(unix)]
        {
            let socket_path = self.config.get_socket_path();
            if std::path::Path::new(&socket_path).exists() {
                if let Err(e) = std::fs::remove_file(&socket_path) {
                    warn!("Failed to remove socket file {}: {}", socket_path, e);
                } else {
                    debug!("Removed socket file: {}", socket_path);
                }
            }
        }

        Ok(())
    }
}

/// Transport client for connecting to the server with reconnection and health monitoring
#[derive(Debug)]
pub struct TransportClient {
    config: ClientConfig,
    socket_config: SocketConfig,
    stream: Option<LocalSocketStream>,
    connected: bool,
    reconnect_attempts: u32,
    last_health_check: std::time::Instant,
    buffer: Vec<u8>,
}

impl TransportClient {
    /// Create a new transport client (for compatibility)
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
            socket_config: SocketConfig::new("default"),
            stream: None,
            connected: false,
            reconnect_attempts: 0,
            last_health_check: std::time::Instant::now(),
            buffer: Vec::new(),
        }
    }

    /// Create a transport client from an existing stream
    pub fn from_stream(stream: LocalSocketStream, socket_config: SocketConfig) -> Self {
        Self {
            config: ClientConfig::default(),
            socket_config,
            stream: Some(stream),
            connected: true,
            reconnect_attempts: 0,
            last_health_check: std::time::Instant::now(),
            buffer: Vec::new(),
        }
    }

    /// Connect to a transport server with reconnection logic
    pub async fn connect(socket_config: &SocketConfig) -> Result<Self> {
        Self::connect_with_config(socket_config, ClientConfig::default()).await
    }

    /// Connect to a transport server with custom client configuration
    pub async fn connect_with_config(
        socket_config: &SocketConfig,
        config: ClientConfig,
    ) -> Result<Self> {
        let socket_path = socket_config.get_socket_path();
        info!("Connecting to transport server: {}", socket_path);

        let mut client = Self {
            config,
            socket_config: socket_config.clone(),
            stream: None,
            connected: false,
            reconnect_attempts: 0,
            last_health_check: std::time::Instant::now(),
            buffer: Vec::new(),
        };

        client.establish_connection().await?;
        Ok(client)
    }

    /// Establish connection with timeout and retry logic
    async fn establish_connection(&mut self) -> Result<()> {
        let socket_path = self.socket_config.get_socket_path();

        // Attempt to connect with timeout
        let name = create_socket_name(&socket_path).map_err(|e| {
            EventBusError::transport(format!("Invalid socket name {}: {}", socket_path, e))
        })?;
        let connection_future = LocalSocketStream::connect(name);
        let timeout_duration = self.config.connection_timeout;

        match timeout(timeout_duration, connection_future).await {
            Ok(Ok(stream)) => {
                self.stream = Some(stream);
                self.connected = true;
                self.reconnect_attempts = 0;
                info!(
                    "Successfully connected to transport server: {}",
                    socket_path
                );
                Ok(())
            }
            Ok(Err(e)) => {
                self.connected = false;
                Err(EventBusError::transport(format!(
                    "Failed to connect to {}: {}",
                    socket_path, e
                )))
            }
            Err(_) => {
                self.connected = false;
                Err(EventBusError::transport(format!(
                    "Connection to {} timed out after {:?}",
                    socket_path, timeout_duration
                )))
            }
        }
    }

    /// Reconnect with exponential backoff
    pub async fn reconnect(&mut self) -> Result<()> {
        if self.reconnect_attempts >= self.config.max_reconnect_attempts {
            return Err(EventBusError::transport(format!(
                "Maximum reconnection attempts ({}) exceeded",
                self.config.max_reconnect_attempts
            )));
        }

        // Calculate backoff delay
        let delay = std::cmp::min(
            Duration::from_millis(
                (self.config.initial_reconnect_delay.as_millis() as f64
                    * self
                        .config
                        .backoff_multiplier
                        .powi(self.reconnect_attempts as i32)) as u64,
            ),
            self.config.max_reconnect_delay,
        );

        warn!(
            "Reconnection attempt {} after {:?}",
            self.reconnect_attempts + 1,
            delay
        );

        sleep(delay).await;
        self.reconnect_attempts += 1;

        // Close existing connection if any
        self.stream = None;
        self.connected = false;

        self.establish_connection().await
    }

    /// Send a message through the transport with automatic reconnection
    pub async fn send(&mut self, data: &[u8]) -> Result<()> {
        if !self.connected {
            self.reconnect().await?;
        }

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| EventBusError::transport("No active connection"))?;

        // Send length prefix first
        let length = data.len() as u32;
        let length_bytes = length.to_le_bytes();

        stream.write_all(&length_bytes).await.map_err(|e| {
            EventBusError::transport(format!("Failed to write length prefix: {}", e))
        })?;

        // Send the actual data
        stream
            .write_all(data)
            .await
            .map_err(|e| EventBusError::transport(format!("Failed to write data: {}", e)))?;

        stream
            .flush()
            .await
            .map_err(|e| EventBusError::transport(format!("Failed to flush data: {}", e)))?;

        debug!("Sent {} bytes through transport", data.len());
        Ok(())
    }

    /// Receive a message from the transport with automatic reconnection
    pub async fn receive(&mut self) -> Result<Vec<u8>> {
        if !self.connected {
            self.reconnect().await?;
        }

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| EventBusError::transport("No active connection"))?;

        // Read length prefix first
        let mut length_bytes = [0u8; 4];
        stream.read_exact(&mut length_bytes).await.map_err(|e| {
            EventBusError::transport(format!("Failed to read length prefix: {}", e))
        })?;

        let length = u32::from_le_bytes(length_bytes) as usize;

        // Read the actual data
        let mut data = vec![0u8; length];
        stream
            .read_exact(&mut data)
            .await
            .map_err(|e| EventBusError::transport(format!("Failed to read data: {}", e)))?;

        debug!("Received {} bytes through transport", data.len());
        Ok(data)
    }

    /// Perform health check
    pub async fn health_check(&mut self) -> Result<bool> {
        if self.last_health_check.elapsed() < self.config.health_check_interval {
            return Ok(self.connected);
        }

        self.last_health_check = std::time::Instant::now();

        if !self.connected {
            return Ok(false);
        }

        // Send a simple ping message
        let ping_data = b"PING";
        let timeout_duration = self.config.health_check_timeout;
        let health_check_future = self.send(ping_data);

        match timeout(timeout_duration, health_check_future).await {
            Ok(Ok(())) => {
                debug!("Health check passed");
                Ok(true)
            }
            Ok(Err(e)) => {
                warn!("Health check failed: {}", e);
                self.connected = false;
                Ok(false)
            }
            Err(_) => {
                warn!("Health check timeout");
                self.connected = false;
                Ok(false)
            }
        }
    }

    /// Check if the connection is still alive
    pub async fn is_alive(&mut self) -> bool {
        self.health_check().await.unwrap_or(false)
    }

    /// Get connection statistics
    pub fn get_connection_stats(&self) -> ConnectionStats {
        ConnectionStats {
            connected: self.connected,
            reconnect_attempts: self.reconnect_attempts,
            last_health_check: self.last_health_check,
        }
    }

    /// Close the transport connection
    pub async fn close(mut self) -> Result<()> {
        self.connected = false;
        self.buffer.clear();

        // Close the stream
        if let Some(stream) = self.stream.take() {
            drop(stream); // Stream will be closed when dropped
        }

        debug!("Transport connection closed");
        Ok(())
    }
}

impl Default for TransportClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Connection statistics for monitoring
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    /// Whether the connection is currently active
    pub connected: bool,
    /// Number of reconnection attempts made
    pub reconnect_attempts: u32,
    /// Timestamp of last health check
    pub last_health_check: std::time::Instant,
}

/// Client connection manager for handling multiple clients with topic-based routing
pub struct ClientConnectionManager {
    /// Active client connections mapped by client ID
    clients: std::collections::HashMap<String, ManagedClient>,
    /// Client configuration
    config: ClientConfig,
    /// Topic subscriptions per client
    subscriptions: std::collections::HashMap<String, Vec<String>>,
    /// Connection statistics
    stats: ConnectionManagerStats,
}

/// Managed client with metadata and health monitoring
#[derive(Debug)]
pub struct ManagedClient {
    /// Transport client
    client: TransportClient,
    /// Client identifier
    client_id: String,
    /// Connection timestamp
    connected_at: std::time::Instant,
    /// Last activity timestamp
    last_activity: std::time::Instant,
    /// Health status
    healthy: bool,
}

/// Connection manager statistics
#[derive(Debug, Clone, Default)]
pub struct ConnectionManagerStats {
    /// Total clients connected
    pub total_clients: usize,
    /// Healthy clients
    pub healthy_clients: usize,
    /// Total messages sent
    pub messages_sent: u64,
    /// Total messages received
    pub messages_received: u64,
    /// Total reconnection attempts
    pub reconnection_attempts: u64,
    /// Failed connections
    pub failed_connections: u64,
}

impl ClientConnectionManager {
    /// Create a new client connection manager
    pub fn new(config: ClientConfig) -> Self {
        Self {
            clients: std::collections::HashMap::new(),
            config,
            subscriptions: std::collections::HashMap::new(),
            stats: ConnectionManagerStats::default(),
        }
    }

    /// Add a new client connection
    pub async fn add_client(
        &mut self,
        client_id: String,
        socket_config: &SocketConfig,
    ) -> Result<()> {
        info!("Adding new client: {}", client_id);

        let client =
            TransportClient::connect_with_config(socket_config, self.config.clone()).await?;

        let managed_client = ManagedClient {
            client,
            client_id: client_id.clone(),
            connected_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            healthy: true,
        };

        self.clients.insert(client_id.clone(), managed_client);
        self.subscriptions.insert(client_id, Vec::new());
        self.stats.total_clients = self.clients.len();
        self.update_healthy_clients_count();

        Ok(())
    }

    /// Remove a client connection
    pub async fn remove_client(&mut self, client_id: &str) -> Result<()> {
        if let Some(managed_client) = self.clients.remove(client_id) {
            info!("Removing client: {}", client_id);
            managed_client.client.close().await?;
            self.subscriptions.remove(client_id);
            self.stats.total_clients = self.clients.len();
            self.update_healthy_clients_count();
        }
        Ok(())
    }

    /// Subscribe a client to topic patterns
    pub async fn subscribe_client(
        &mut self,
        client_id: &str,
        topic_patterns: Vec<String>,
    ) -> Result<()> {
        if !self.clients.contains_key(client_id) {
            return Err(EventBusError::transport(format!(
                "Client {} not found",
                client_id
            )));
        }

        debug!(
            "Subscribing client {} to patterns: {:?}",
            client_id, topic_patterns
        );
        self.subscriptions
            .insert(client_id.to_string(), topic_patterns);
        Ok(())
    }

    /// Unsubscribe a client from all topics
    pub async fn unsubscribe_client(&mut self, client_id: &str) -> Result<()> {
        if let Some(patterns) = self.subscriptions.get_mut(client_id) {
            debug!("Unsubscribing client {} from all topics", client_id);
            patterns.clear();
        }
        Ok(())
    }

    /// Send message to specific client
    pub async fn send_to_client(&mut self, client_id: &str, data: &[u8]) -> Result<()> {
        if let Some(managed_client) = self.clients.get_mut(client_id) {
            match managed_client.client.send(data).await {
                Ok(()) => {
                    managed_client.last_activity = std::time::Instant::now();
                    managed_client.healthy = true;
                    self.stats.messages_sent += 1;
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to send to client {}: {}", client_id, e);
                    managed_client.healthy = false;
                    self.stats.failed_connections += 1;
                    self.update_healthy_clients_count();
                    Err(e)
                }
            }
        } else {
            Err(EventBusError::transport(format!(
                "Client {} not found",
                client_id
            )))
        }
    }

    /// Broadcast message to clients matching topic pattern
    pub async fn broadcast_to_topic(&mut self, topic: &str, data: &[u8]) -> Result<Vec<String>> {
        let mut delivered_clients = Vec::new();
        let mut failed_clients = Vec::new();

        // Collect matching client IDs first to avoid borrowing issues
        let matching_clients: Vec<String> = self
            .subscriptions
            .iter()
            .filter_map(|(client_id, patterns)| {
                if patterns
                    .iter()
                    .any(|pattern| self.topic_matches(topic, pattern))
                {
                    Some(client_id.clone())
                } else {
                    None
                }
            })
            .collect();

        // Send to matching clients
        for client_id in matching_clients {
            match self.send_to_client(&client_id, data).await {
                Ok(()) => delivered_clients.push(client_id),
                Err(_) => failed_clients.push(client_id),
            }
        }

        // Remove failed clients
        for client_id in failed_clients {
            let _ = self.remove_client(&client_id).await;
        }

        Ok(delivered_clients)
    }

    /// Simple topic pattern matching (supports + and # wildcards)
    fn topic_matches(&self, topic: &str, pattern: &str) -> bool {
        let topic_parts: Vec<&str> = topic.split('.').collect();
        let pattern_parts: Vec<&str> = pattern.split('.').collect();

        self.match_parts(&topic_parts, &pattern_parts)
    }

    /// Recursive pattern matching helper
    #[allow(clippy::only_used_in_recursion)]
    fn match_parts(&self, topic_parts: &[&str], pattern_parts: &[&str]) -> bool {
        match (topic_parts.first(), pattern_parts.first()) {
            (None, None) => true,
            (Some(_), None) => false,
            (None, Some(pattern_part)) if *pattern_part == "#" => true,
            (None, Some(_)) => false,
            (Some(_topic_part), Some(pattern_part)) if *pattern_part == "#" => true,
            (Some(_topic_part), Some(pattern_part)) if *pattern_part == "+" => {
                self.match_parts(&topic_parts[1..], &pattern_parts[1..])
            }
            (Some(topic_part), Some(pattern_part)) => {
                if topic_part == pattern_part {
                    self.match_parts(&topic_parts[1..], &pattern_parts[1..])
                } else {
                    false
                }
            }
        }
    }

    /// Perform health checks on all clients
    pub async fn health_check_all(&mut self) -> Result<()> {
        let mut unhealthy_clients = Vec::new();

        for (client_id, managed_client) in &mut self.clients {
            if !managed_client.client.health_check().await? {
                warn!("Client {} failed health check", client_id);
                managed_client.healthy = false;
                unhealthy_clients.push(client_id.clone());
            } else {
                managed_client.healthy = true;
            }
        }

        // Attempt to reconnect unhealthy clients
        for client_id in unhealthy_clients {
            if let Some(managed_client) = self.clients.get_mut(&client_id) {
                info!("Attempting to reconnect client: {}", client_id);
                match managed_client.client.reconnect().await {
                    Ok(()) => {
                        managed_client.healthy = true;
                        info!("Successfully reconnected client: {}", client_id);
                    }
                    Err(e) => {
                        error!("Failed to reconnect client {}: {}", client_id, e);
                        self.stats.reconnection_attempts += 1;
                    }
                }
            }
        }

        self.update_healthy_clients_count();
        Ok(())
    }

    /// Update healthy clients count in statistics
    fn update_healthy_clients_count(&mut self) {
        self.stats.healthy_clients = self.clients.values().filter(|c| c.healthy).count();
    }

    /// Get connection manager statistics
    pub fn get_stats(&self) -> &ConnectionManagerStats {
        &self.stats
    }

    /// Get list of connected client IDs
    pub fn get_client_ids(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Get client connection info
    pub fn get_client_info(&self, client_id: &str) -> Option<ClientInfo> {
        self.clients
            .get(client_id)
            .map(|managed_client| ClientInfo {
                client_id: managed_client.client_id.clone(),
                connected_at: managed_client.connected_at,
                last_activity: managed_client.last_activity,
                healthy: managed_client.healthy,
                subscriptions: self
                    .subscriptions
                    .get(client_id)
                    .cloned()
                    .unwrap_or_default(),
            })
    }

    /// Shutdown all client connections
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down client connection manager");

        let client_ids: Vec<String> = self.clients.keys().cloned().collect();
        for client_id in client_ids {
            let _ = self.remove_client(&client_id).await;
        }

        Ok(())
    }
}

/// Client information for monitoring
#[derive(Debug, Clone)]
pub struct ClientInfo {
    /// Client identifier
    pub client_id: String,
    /// Connection timestamp
    pub connected_at: std::time::Instant,
    /// Last activity timestamp
    pub last_activity: std::time::Instant,
    /// Health status
    pub healthy: bool,
    /// Subscribed topic patterns
    pub subscriptions: Vec<String>,
}

/// Transport connection manager for handling multiple clients (legacy compatibility)
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

    #[tokio::test]
    async fn test_client_connection_manager() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("test-manager.sock");
        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100, // Default connection limit for tests
        };

        // Start a server first
        let mut server = TransportServer::new(socket_config.clone()).await.unwrap();

        let config = ClientConfig::default();
        let mut manager = ClientConnectionManager::new(config);

        // Add a client (this will now connect to the real server)
        let result = manager
            .add_client("test-client".to_string(), &socket_config)
            .await;
        assert!(result.is_ok());

        // Check stats
        let stats = manager.get_stats();
        assert_eq!(stats.total_clients, 1);
        assert_eq!(stats.healthy_clients, 1);

        // Subscribe client to topics
        let patterns = vec![
            "events.process.*".to_string(),
            "control.health.+".to_string(),
        ];
        let result = manager.subscribe_client("test-client", patterns).await;
        assert!(result.is_ok());

        // Test topic matching
        assert!(manager.topic_matches("events.process.lifecycle", "events.process.#"));
        assert!(manager.topic_matches("control.health.status", "control.health.+"));
        assert!(!manager.topic_matches("events.network.connections", "events.process.#"));

        // Remove client
        let result = manager.remove_client("test-client").await;
        assert!(result.is_ok());

        let stats = manager.get_stats();
        assert_eq!(stats.total_clients, 0);

        // Clean up server
        let _ = server.shutdown().await;
    }

    #[tokio::test]
    async fn test_topic_pattern_matching() {
        let config = ClientConfig::default();
        let manager = ClientConnectionManager::new(config);

        // Test exact match
        assert!(manager.topic_matches("events.process.lifecycle", "events.process.lifecycle"));

        // Test single-level wildcard
        assert!(manager.topic_matches("events.process.lifecycle", "events.process.+"));
        assert!(manager.topic_matches("events.process.metadata", "events.process.+"));
        assert!(!manager.topic_matches("events.process.lifecycle.extra", "events.process.+"));

        // Test multi-level wildcard
        assert!(manager.topic_matches("events.process.lifecycle", "events.#"));
        assert!(manager.topic_matches("events.process.lifecycle.extra", "events.#"));
        assert!(manager.topic_matches("events.network.connections", "events.#"));
        assert!(!manager.topic_matches("control.collector.status", "events.#"));

        // Test no match
        assert!(!manager.topic_matches("events.network.connections", "events.process.+"));
        assert!(!manager.topic_matches("control.collector.status", "events.process.*"));
    }
}
