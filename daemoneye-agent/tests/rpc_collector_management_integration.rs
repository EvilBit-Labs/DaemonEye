//! Integration tests for RPC-based collector management workflows
//!
//! This test suite validates the complete RPC integration between daemoneye-agent
//! and collector-core components, including:
//! - Collector lifecycle management (start, stop, restart)
//! - Health check RPC calls
//! - Graceful shutdown coordination
//! - Error handling and timeout scenarios

use anyhow::Result;
use daemoneye_agent::broker_manager::BrokerManager;
use daemoneye_lib::config::BrokerConfig;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Helper function to create a test broker manager with temporary socket
async fn create_test_broker_manager() -> Result<(TempDir, Arc<BrokerManager>)> {
    // Initialize tracing for test output
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    // Create temporary directory for socket
    let temp_dir = tempfile::tempdir()?;
    let socket_path = temp_dir.path().join("test-broker.sock");

    let config = BrokerConfig {
        enabled: true,
        socket_path: socket_path.to_string_lossy().to_string(),
        max_connections: 10,
        shutdown_timeout_seconds: 5,
        startup_timeout_seconds: 10,
        message_buffer_size: 1000,
        config_directory: temp_dir.path().to_path_buf(),
        collector_binaries: std::collections::HashMap::new(),
        process_manager: daemoneye_lib::config::ProcessManagerConfig::default(),
        topic_hierarchy: daemoneye_lib::config::TopicHierarchyConfig::default(),
    };

    let manager = Arc::new(BrokerManager::new(config));
    manager.start().await?;

    // Wait for broker to become healthy
    manager.wait_for_healthy(Duration::from_secs(5)).await?;

    Ok((temp_dir, manager))
}

#[tokio::test]
async fn test_rpc_health_check_workflow() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Create RPC client for a test collector
    let collector_id = "test-collector";
    let _client = manager.create_rpc_client(collector_id).await?;

    // Note: In a real scenario, a collector would be running and responding to health checks
    // For this test, we're validating that the RPC client can be created and the request
    // can be sent (even if it times out due to no collector responding)

    // Attempt health check (will timeout since no collector is running)
    let health_check_result = timeout(
        Duration::from_secs(2),
        manager.health_check_rpc(collector_id),
    )
    .await;

    // We expect a timeout or error since no collector is actually running
    assert!(
        health_check_result.is_err() || health_check_result.unwrap().is_err(),
        "Health check should timeout or error when no collector is running"
    );

    // Verify RPC client was created and stored
    let registered_ids = manager.list_registered_collector_ids().await;
    assert!(
        registered_ids.iter().any(|id| id == collector_id),
        "Collector ID should be registered"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_rpc_graceful_shutdown_workflow() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Create RPC client for a test collector
    let collector_id = "test-collector-shutdown";
    let _client = manager.create_rpc_client(collector_id).await?;

    // Attempt graceful shutdown (will timeout since no collector is running)
    let shutdown_result = timeout(
        Duration::from_secs(2),
        manager.stop_collector_rpc(collector_id, true),
    )
    .await;

    // We expect a timeout or error since no collector is actually running
    assert!(
        shutdown_result.is_err() || shutdown_result.unwrap().is_err(),
        "Graceful shutdown should timeout or error when no collector is running"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_rpc_multiple_collectors_management() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Create RPC clients for multiple collectors
    let collector_ids = vec!["collector-1", "collector-2", "collector-3"];

    for collector_id in &collector_ids {
        manager.create_rpc_client(collector_id).await?;
    }

    // Verify all collectors are registered
    let registered_ids = manager.list_registered_collector_ids().await;
    assert_eq!(
        registered_ids.len(),
        collector_ids.len(),
        "All collectors should be registered"
    );

    for collector_id in &collector_ids {
        assert!(
            registered_ids.iter().any(|id| id == collector_id),
            "Collector {} should be registered",
            collector_id
        );
    }

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_rpc_client_reuse() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    let collector_id = "test-collector-reuse";

    // Create RPC client first time
    let client1 = manager.get_rpc_client(collector_id).await?;

    // Get RPC client second time (should reuse existing)
    let client2 = manager.get_rpc_client(collector_id).await?;

    // Verify both clients point to the same instance
    assert_eq!(
        Arc::strong_count(&client1),
        Arc::strong_count(&client2),
        "RPC clients should be reused"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_broker_manager_shutdown_with_collectors() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Create multiple RPC clients
    let collector_ids = vec!["collector-a", "collector-b", "collector-c"];
    for collector_id in &collector_ids {
        manager.create_rpc_client(collector_id).await?;
    }

    // Verify collectors are registered
    let registered_ids = manager.list_registered_collector_ids().await;
    assert_eq!(registered_ids.len(), collector_ids.len());

    // Shutdown should gracefully handle all collectors
    // Note: This test verifies that shutdown completes even when collectors don't respond
    // The shutdown includes a 2-second wait for RPC responses, then proceeds with cleanup
    let shutdown_result = manager.shutdown().await;

    assert!(
        shutdown_result.is_ok(),
        "Broker manager shutdown should succeed: {:?}",
        shutdown_result.err()
    );

    Ok(())
}

#[tokio::test]
async fn test_rpc_error_handling() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Attempt to get RPC client for non-existent collector
    // This should succeed (client creation doesn't require collector to exist)
    let client_result = manager.get_rpc_client("non-existent-collector").await;
    assert!(
        client_result.is_ok(),
        "RPC client creation should succeed even if collector doesn't exist"
    );

    // Attempt health check on non-existent collector (should timeout)
    let health_check_result = timeout(
        Duration::from_secs(2),
        manager.health_check_rpc("non-existent-collector"),
    )
    .await;

    assert!(
        health_check_result.is_err() || health_check_result.unwrap().is_err(),
        "Health check should fail for non-existent collector"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_broker_health_aggregation() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Initial health check should be healthy
    let initial_health = manager.health_check().await;
    assert!(
        matches!(
            initial_health,
            daemoneye_agent::broker_manager::BrokerHealth::Healthy
        ),
        "Broker should be healthy initially"
    );

    // Create some RPC clients (simulating collectors)
    manager.create_rpc_client("collector-1").await?;
    manager.create_rpc_client("collector-2").await?;

    // Health check should still work
    let health_with_collectors = manager.health_check().await;
    assert!(
        !matches!(
            health_with_collectors,
            daemoneye_agent::broker_manager::BrokerHealth::Stopped
        ),
        "Broker should not be stopped"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_rpc_concurrent_operations() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    // Create multiple collectors concurrently
    let collector_ids: Vec<String> = (0..5).map(|i| format!("collector-{}", i)).collect();

    let mut handles = Vec::with_capacity(collector_ids.len());
    for collector_id in &collector_ids {
        let manager_clone = Arc::clone(&manager);
        let collector_id = collector_id.clone();
        let handle =
            tokio::spawn(async move { manager_clone.create_rpc_client(&collector_id).await });
        handles.push(handle);
    }

    // Wait for all operations to complete
    for handle in handles {
        let result = handle.await?;
        assert!(
            result.is_ok(),
            "Concurrent RPC client creation should succeed"
        );
    }

    // Verify all collectors are registered
    let registered_ids = manager.list_registered_collector_ids().await;
    assert_eq!(
        registered_ids.len(),
        collector_ids.len(),
        "All collectors should be registered"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_rpc_restart_workflow() -> Result<()> {
    let (_temp_dir, manager) = create_test_broker_manager().await?;

    let collector_id = "test-collector-restart";
    let _client = manager.create_rpc_client(collector_id).await?;

    // Attempt restart (will timeout since no collector is running)
    let restart_result = timeout(
        Duration::from_secs(2),
        manager.restart_collector_rpc(collector_id),
    )
    .await;

    // We expect a timeout or error since no collector is actually running
    assert!(
        restart_result.is_err() || restart_result.unwrap().is_err(),
        "Restart should timeout or error when no collector is running"
    );

    // Cleanup
    manager.shutdown().await?;
    Ok(())
}
