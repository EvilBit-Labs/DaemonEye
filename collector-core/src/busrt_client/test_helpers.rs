//! Test helpers and fixtures for busrt client testing.

#[cfg(test)]
pub(crate) mod fixtures {
    use crate::{
        EmbeddedBroker, EmbeddedBrokerConfig,
        busrt_types::{BusrtError, SecurityConfig, TransportConfig, TransportType},
    };
    use std::time::Duration;
    use tempfile::TempDir;

    /// Test helper that sets up both broker and client for integration tests.
    ///
    /// This helper ensures proper startup order: broker starts first, then client connects.
    /// This pattern mimics production where daemoneye-agent (orchestrator) starts the broker
    /// before procmond (client) attempts to connect.
    pub(crate) struct BusrtTestFixture {
        pub broker: EmbeddedBroker,
        /// Temporary directory kept alive for the duration of the test via RAII
        #[allow(dead_code)]
        pub temp_dir: TempDir,
        pub socket_path: std::path::PathBuf,
    }

    impl BusrtTestFixture {
        /// Creates and starts a test broker with a client-ready configuration.
        pub(crate) async fn new() -> Result<Self, BusrtError> {
            let temp_dir = TempDir::new().map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to create temp dir: {}", e))
            })?;
            let socket_path = temp_dir.path().join("busrt_test.sock");

            let config = EmbeddedBrokerConfig {
                max_connections: 10,
                message_buffer_size: 1000,
                transport: TransportConfig {
                    transport_type: TransportType::UnixSocket,
                    path: Some(socket_path.to_string_lossy().to_string()),
                    address: None,
                    port: None,
                },
                security: SecurityConfig::default(),
            };

            let mut broker = EmbeddedBroker::new(config).await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to create broker: {}", e))
            })?;

            // Start the broker and wait for it to be ready
            broker.start().await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to start broker: {}", e))
            })?;

            // Give broker sufficient time to fully initialize and start accepting connections.
            // In production, daemoneye-agent would ensure the broker is fully operational
            // before procmond and other clients attempt to connect.
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Verify the socket was created
            if !socket_path.exists() {
                return Err(BusrtError::ConnectionFailed(
                    "Broker socket file was not created".to_string(),
                ));
            }

            Ok(Self {
                broker,
                temp_dir,
                socket_path,
            })
        }

        /// Creates a client configured to connect to the test broker.
        pub(crate) fn client_config(&self) -> TransportConfig {
            TransportConfig {
                transport_type: TransportType::UnixSocket,
                path: Some(self.socket_path.to_string_lossy().to_string()),
                address: None,
                port: None,
            }
        }

        /// Shuts down the broker gracefully.
        pub(crate) async fn shutdown(mut self) -> Result<(), BusrtError> {
            self.broker.shutdown().await.map_err(|e| {
                BusrtError::ConnectionFailed(format!("Failed to shutdown broker: {}", e))
            })
        }
    }
}
