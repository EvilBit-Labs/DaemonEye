//! Unit tests for busrt client implementation.

use super::*;
use crate::busrt_types::{TransportConfig, TransportType};
use std::{sync::atomic::Ordering, time::Duration};
use tempfile::TempDir;

#[tokio::test]
async fn test_client_creation() {
    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/test_client.sock".to_string()),
        address: None,
        port: None,
    };

    let client = CollectorBusrtClient::new(transport).await;
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_reconnection_config() {
    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/test_reconnect.sock".to_string()),
        address: None,
        port: None,
    };

    let mut client = CollectorBusrtClient::new(transport).await.unwrap();

    let config = ReconnectionConfig {
        enabled: true,
        initial_delay: Duration::from_millis(50),
        max_delay: Duration::from_secs(5),
        backoff_multiplier: 1.5,
        max_attempts: 10,
    };

    // Test that we can set the configuration without errors
    client.set_reconnection_config(config.clone());
    // Configuration is applied internally - validated through connection behavior
}

#[tokio::test]
async fn test_connection_timeout() {
    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/test_timeout.sock".to_string()),
        address: None,
        port: None,
    };

    let mut client = CollectorBusrtClient::new(transport).await.unwrap();

    let timeout = Duration::from_secs(5);
    // Test that we can set the timeout without errors
    client.set_connection_timeout(timeout);
    // Timeout is applied internally - validated through connection behavior
}

#[tokio::test]
async fn test_connection_state() {
    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/test_state.sock".to_string()),
        address: None,
        port: None,
    };

    let client = CollectorBusrtClient::new(transport).await.unwrap();

    // Initially not connected - validated via public API
    assert!(
        !client.is_connected(),
        "Client should not be connected initially"
    );
}

#[tokio::test]
async fn test_statistics_tracking() {
    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some("/tmp/test_stats.sock".to_string()),
        address: None,
        port: None,
    };

    let client = CollectorBusrtClient::new(transport).await.unwrap();

    let stats = client.get_statistics().await;
    assert_eq!(stats.messages_published.load(Ordering::Relaxed), 0);
    assert_eq!(stats.messages_received.load(Ordering::Relaxed), 0);
    assert_eq!(stats.connection_failures.load(Ordering::Relaxed), 0);
}

/// Test that demonstrates client connection failure when broker is not started.
///
/// This test validates that clients properly handle the case where the broker
/// is not available, which can happen during:
/// - Initial startup race conditions
/// - Broker crashes
/// - Network/socket issues
#[tokio::test]
async fn test_client_connection_without_broker() {
    let _ = tracing_subscriber::fmt::try_init();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let socket_path = temp_dir.path().join("nonexistent_broker.sock");

    let transport = TransportConfig {
        transport_type: TransportType::UnixSocket,
        path: Some(socket_path.to_string_lossy().to_string()),
        address: None,
        port: None,
    };

    let mut client = CollectorBusrtClient::new(transport)
        .await
        .expect("Client creation should succeed");

    // Configure with short timeouts and limited retries for faster test
    client.set_connection_timeout(Duration::from_millis(500));
    let reconnect_config = ReconnectionConfig {
        enabled: false, // Disable reconnection for this test
        ..Default::default()
    };
    client.set_reconnection_config(reconnect_config);

    // Connection should fail when broker is not running
    let connection_result = client.connect().await;
    assert!(
        connection_result.is_err(),
        "Client connection should fail when broker is not available"
    );
    assert!(
        !client.is_connected(),
        "Client should not be in connected state after failed connection"
    );
}

/// Integration test demonstrating proper broker-client connection pattern.
///
/// **Note**: This test is currently ignored because the full busrt protocol implementation
/// is still in progress. The test fixture and pattern are documented here for when
/// the actual protocol implementation is complete.
#[tokio::test]
#[ignore = "Busrt protocol implementation in progress - test infrastructure ready"]
async fn test_broker_client_integration() {
    use test_helpers::fixtures::BusrtTestFixture;

    let _ = tracing_subscriber::fmt::try_init();

    // Step 1: Start the broker (mimics daemoneye-agent startup)
    let fixture = BusrtTestFixture::new()
        .await
        .expect("Failed to create test fixture with broker");

    // Step 2: Create and connect client (mimics procmond startup)
    let mut client = CollectorBusrtClient::new(fixture.client_config())
        .await
        .expect("Failed to create client");

    // Configure with reasonable timeouts for testing
    client.set_connection_timeout(Duration::from_secs(2));
    let reconnect_config = ReconnectionConfig {
        max_attempts: 3,
        initial_delay: Duration::from_millis(100),
        ..Default::default()
    };
    client.set_reconnection_config(reconnect_config);

    // Step 3: Attempt connection (this should succeed now that broker is running)
    let connection_result = client.connect().await;
    assert!(
        connection_result.is_ok(),
        "Client should connect successfully when broker is running: {:?}",
        connection_result.err()
    );
    assert!(client.is_connected(), "Client should be in connected state");

    // Step 4: Verify client statistics
    let stats = client.get_statistics().await;
    assert_eq!(
        stats.messages_published.load(Ordering::Relaxed),
        0,
        "No messages should be published initially"
    );

    // Step 5: Disconnect client gracefully
    client
        .disconnect()
        .await
        .expect("Client should disconnect gracefully");
    assert!(
        !client.is_connected(),
        "Client should be disconnected after disconnect call"
    );

    // Step 6: Shutdown broker
    fixture
        .shutdown()
        .await
        .expect("Broker should shutdown gracefully");
}
