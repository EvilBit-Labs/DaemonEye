//! Integration tests for dual-protocol architecture
//!
//! Tests the coexistence of EventBus broker and IPC server within daemoneye-agent

use anyhow::Result;
use daemoneye_agent::{BrokerManager, IpcServerManager, create_cli_ipc_config};
use daemoneye_lib::config::BrokerConfig;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Create test configurations with unique paths
fn create_test_configs() -> (BrokerConfig, daemoneye_lib::ipc::IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_path = temp_dir.path();

    let broker_config = BrokerConfig {
        socket_path: temp_path
            .join("test-broker.sock")
            .to_string_lossy()
            .to_string(),
        enabled: true,
        startup_timeout_seconds: 10,
        shutdown_timeout_seconds: 10,
        max_connections: 10,
        message_buffer_size: 100,
        topic_hierarchy: daemoneye_lib::config::TopicHierarchyConfig::default(),
    };

    let mut ipc_config = create_cli_ipc_config();
    ipc_config.endpoint_path = temp_path
        .join("test-cli.sock")
        .to_string_lossy()
        .to_string();
    ipc_config.max_connections = 4;

    (broker_config, ipc_config, temp_dir)
}

#[tokio::test]
async fn test_dual_protocol_startup_and_shutdown() {
    let (broker_config, ipc_config, _temp_dir) = create_test_configs();

    // Create both managers
    let broker_manager = BrokerManager::new(broker_config);
    let ipc_server_manager = IpcServerManager::new(ipc_config);

    // Start both services
    let broker_start = broker_manager.start();
    let ipc_start = ipc_server_manager.start();

    // Both should start successfully
    let (broker_result, ipc_result) = tokio::join!(broker_start, ipc_start);
    assert!(broker_result.is_ok(), "Broker should start successfully");
    assert!(ipc_result.is_ok(), "IPC server should start successfully");

    // Wait for both to become healthy
    let broker_healthy = timeout(
        Duration::from_secs(5),
        broker_manager.wait_for_healthy(Duration::from_secs(5)),
    );
    let ipc_healthy = timeout(
        Duration::from_secs(5),
        ipc_server_manager.wait_for_healthy(Duration::from_secs(5)),
    );

    let (broker_health_result, ipc_health_result) = tokio::join!(broker_healthy, ipc_healthy);
    assert!(broker_health_result.is_ok(), "Broker should become healthy");
    assert!(
        ipc_health_result.is_ok(),
        "IPC server should become healthy"
    );

    // Verify both are running
    assert!(
        broker_manager.is_running().await,
        "Broker should be running"
    );
    // Note: IPC server doesn't expose is_running in the current implementation

    // Shutdown both services
    let broker_shutdown = broker_manager.shutdown();
    let ipc_shutdown = ipc_server_manager.shutdown();

    let (broker_shutdown_result, ipc_shutdown_result) = tokio::join!(broker_shutdown, ipc_shutdown);
    assert!(
        broker_shutdown_result.is_ok(),
        "Broker should shutdown successfully"
    );
    assert!(
        ipc_shutdown_result.is_ok(),
        "IPC server should shutdown successfully"
    );
}

#[tokio::test]
async fn test_services_coexist_without_interference() {
    let (broker_config, ipc_config, _temp_dir) = create_test_configs();

    // Create both managers
    let broker_manager = BrokerManager::new(broker_config);
    let ipc_server_manager = IpcServerManager::new(ipc_config);

    // Start broker first
    broker_manager.start().await.expect("Broker should start");
    broker_manager
        .wait_for_healthy(Duration::from_secs(5))
        .await
        .expect("Broker should be healthy");

    // Start IPC server second
    ipc_server_manager
        .start()
        .await
        .expect("IPC server should start");
    ipc_server_manager
        .wait_for_healthy(Duration::from_secs(5))
        .await
        .expect("IPC server should be healthy");

    // Both should be healthy simultaneously
    let broker_health = broker_manager.health_check().await;
    let ipc_health = ipc_server_manager.health_check().await;

    assert_eq!(broker_health, daemoneye_agent::BrokerHealth::Healthy);
    assert_eq!(ipc_health, daemoneye_agent::IpcServerHealth::Healthy);

    // Get statistics from broker (should work while IPC server is running)
    let stats = broker_manager.statistics().await;
    assert!(stats.is_some(), "Should be able to get broker statistics");

    // Shutdown in reverse order
    ipc_server_manager
        .shutdown()
        .await
        .expect("IPC server should shutdown");
    broker_manager
        .shutdown()
        .await
        .expect("Broker should shutdown");
}

#[tokio::test]
async fn test_independent_health_monitoring() {
    let (broker_config, ipc_config, _temp_dir) = create_test_configs();

    // Create both managers
    let broker_manager = BrokerManager::new(broker_config);
    let ipc_server_manager = IpcServerManager::new(ipc_config);

    // Start both services
    let (broker_start_result, ipc_start_result) =
        tokio::join!(broker_manager.start(), ipc_server_manager.start());
    broker_start_result.expect("Broker should start");
    ipc_start_result.expect("IPC server should start");

    // Wait for both to be healthy
    let (broker_health_result, ipc_health_result) = tokio::join!(
        broker_manager.wait_for_healthy(Duration::from_secs(5)),
        ipc_server_manager.wait_for_healthy(Duration::from_secs(5))
    );
    broker_health_result.expect("Broker should be healthy");
    ipc_health_result.expect("IPC server should be healthy");

    // Health checks should be independent
    let broker_health = broker_manager.health_check().await;
    let ipc_health = ipc_server_manager.health_check().await;

    assert_eq!(broker_health, daemoneye_agent::BrokerHealth::Healthy);
    assert_eq!(ipc_health, daemoneye_agent::IpcServerHealth::Healthy);

    // Shutdown broker only
    broker_manager
        .shutdown()
        .await
        .expect("Broker should shutdown");

    // IPC server should still be healthy
    let ipc_health_after = ipc_server_manager.health_check().await;
    assert_eq!(ipc_health_after, daemoneye_agent::IpcServerHealth::Healthy);

    // Shutdown IPC server
    ipc_server_manager
        .shutdown()
        .await
        .expect("IPC server should shutdown");
}

#[tokio::test]
async fn test_different_endpoint_paths() -> Result<()> {
    let (broker_config, ipc_config, _temp_dir) = create_test_configs();

    // Verify they use different endpoint paths
    assert_ne!(broker_config.socket_path, ipc_config.endpoint_path);

    let broker_manager = BrokerManager::new(broker_config.clone());
    let ipc_server_manager = IpcServerManager::new(ipc_config.clone());

    // Verify path accessors return correct values
    assert_eq!(broker_manager.socket_path(), &broker_config.socket_path);
    assert_eq!(
        ipc_server_manager.endpoint_path(),
        &ipc_config.endpoint_path
    );

    // Both should be able to start with different paths
    let (start_broker, start_ipc) =
        tokio::join!(broker_manager.start(), ipc_server_manager.start());
    start_broker?;
    start_ipc?;

    // Both should become healthy
    let (health_broker, health_ipc) = tokio::join!(
        broker_manager.wait_for_healthy(Duration::from_secs(5)),
        ipc_server_manager.wait_for_healthy(Duration::from_secs(5))
    );
    health_broker?;
    health_ipc?;

    // Cleanup
    let (shutdown_broker, shutdown_ipc) =
        tokio::join!(broker_manager.shutdown(), ipc_server_manager.shutdown());
    shutdown_broker?;
    shutdown_ipc?;

    Ok(())
}
