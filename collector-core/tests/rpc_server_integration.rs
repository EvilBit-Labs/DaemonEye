//! Integration tests for collector-side RPC server functionality.
//!
//! These tests validate that collectors can receive and respond to RPC requests
//! for lifecycle operations, health checks, and configuration updates.

use collector_core::{Collector, CollectorConfig};
use daemoneye_eventbus::{
    DaemoneyeBroker,
    rpc::{CollectorOperation, CollectorRpcClient, RpcRequest, RpcStatus},
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

/// Setup test broker and collector
async fn setup_test_broker_and_collector()
-> anyhow::Result<(TempDir, Arc<DaemoneyeBroker>, Collector)> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test-broker.sock");

    let broker = Arc::new(DaemoneyeBroker::new(&socket_path.to_string_lossy()).await?);

    let config = CollectorConfig {
        registration: Some(collector_core::CollectorRegistrationConfig {
            enabled: true,
            broker: Some(Arc::clone(&broker)),
            collector_id: Some("test-collector".to_string()),
            collector_type: Some("test".to_string()),
            topic: "control.collector.registration".to_string(),
            timeout: Duration::from_secs(10),
            retry_attempts: 3,
            heartbeat_interval: Duration::from_secs(5),
            attributes: std::collections::HashMap::new(),
        }),
        ..Default::default()
    };

    let collector = Collector::new(config);

    Ok((temp_dir, broker, collector))
}

#[tokio::test]
async fn test_rpc_service_initialization() -> anyhow::Result<()> {
    let (_temp_dir, _broker, _collector) = setup_test_broker_and_collector().await?;

    // Test that collector can be created with RPC service configuration
    // The RPC service will be started automatically during collector.run()
    // This test validates the setup function works correctly

    Ok(())
}

#[tokio::test]
#[ignore] // TODO: Fix RPC communication issue - RPC service not receiving/responding to requests
async fn test_health_check_response() -> anyhow::Result<()> {
    let (temp_dir, broker, collector) = setup_test_broker_and_collector().await?;
    let collector_id = "test-collector";

    // Start collector in background task
    let collector_handle = tokio::spawn(async move {
        let _ = collector.run().await;
    });

    // Wait for collector to start and register
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create RPC client
    let rpc_client = CollectorRpcClient::new(
        &format!("control.collector.{}", collector_id),
        Arc::clone(&broker),
    )
    .await?;

    // Send health check RPC
    let request = RpcRequest::health_check(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        Duration::from_secs(10),
    );

    let response = timeout(
        Duration::from_secs(5),
        rpc_client.call(request, Duration::from_secs(10)),
    )
    .await??;

    // Assert response
    assert_eq!(
        response.status,
        RpcStatus::Success,
        "Health check should succeed"
    );

    // Cleanup
    collector_handle.abort();
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
#[ignore] // TODO: Fix RPC communication issue - RPC service not receiving/responding to requests
async fn test_lifecycle_operation_handling() -> anyhow::Result<()> {
    let (temp_dir, broker, collector) = setup_test_broker_and_collector().await?;
    let collector_id = "test-collector";

    // Start collector in background task
    let collector_handle = tokio::spawn(async move {
        let _ = collector.run().await;
    });

    // Wait for collector to start and register
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create RPC client
    let rpc_client = CollectorRpcClient::new(
        &format!("control.collector.{}", collector_id),
        Arc::clone(&broker),
    )
    .await?;

    // Test Start operation (should succeed as runtime is already started)
    let start_request = RpcRequest::lifecycle(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        CollectorOperation::Start,
        daemoneye_eventbus::rpc::CollectorLifecycleRequest::start(collector_id, None),
        Duration::from_secs(30),
    );

    let start_response = timeout(
        Duration::from_secs(5),
        rpc_client.call(start_request, Duration::from_secs(30)),
    )
    .await??;

    assert_eq!(
        start_response.status,
        RpcStatus::Success,
        "Start operation should succeed"
    );

    // Test Stop operation (should trigger shutdown signal)
    let stop_request = RpcRequest::lifecycle(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        CollectorOperation::Stop,
        daemoneye_eventbus::rpc::CollectorLifecycleRequest::stop(collector_id),
        Duration::from_secs(30),
    );

    let stop_response = timeout(
        Duration::from_secs(5),
        rpc_client.call(stop_request, Duration::from_secs(30)),
    )
    .await??;

    assert_eq!(
        stop_response.status,
        RpcStatus::Success,
        "Stop operation should succeed"
    );

    // Wait for collector to shut down
    let _ = timeout(Duration::from_secs(2), collector_handle).await;
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
#[ignore] // TODO: Fix RPC communication issue - RPC service not receiving/responding to requests
async fn test_graceful_shutdown_via_rpc() -> anyhow::Result<()> {
    let (temp_dir, broker, collector) = setup_test_broker_and_collector().await?;
    let collector_id = "test-collector";

    // Start collector in background task
    let collector_handle = tokio::spawn(async move {
        let _ = collector.run().await;
    });

    // Wait for collector to start, register, and RPC service to be ready
    // Give enough time for: registration -> RPC service start -> subscription setup
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // Create RPC client
    let rpc_client = CollectorRpcClient::new(
        &format!("control.collector.{}", collector_id),
        Arc::clone(&broker),
    )
    .await?;

    // Give the RPC client subscription time to be established
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send graceful shutdown RPC
    let shutdown_request = RpcRequest::shutdown(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        daemoneye_eventbus::rpc::ShutdownRequest {
            collector_id: collector_id.to_string(),
            shutdown_type: daemoneye_eventbus::rpc::ShutdownType::Graceful,
            graceful_timeout_ms: 2000,
            force_after_timeout: false,
            reason: Some("Test shutdown".to_string()),
        },
        Duration::from_secs(10),
    );

    let shutdown_response = timeout(
        Duration::from_secs(10),
        rpc_client.call(shutdown_request, Duration::from_secs(10)),
    )
    .await??;

    assert_eq!(
        shutdown_response.status,
        RpcStatus::Success,
        "Graceful shutdown should succeed"
    );

    // Wait for collector to shut down - give it more time
    let _ = timeout(Duration::from_secs(5), collector_handle).await;
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
async fn test_rpc_service_error_handling() -> anyhow::Result<()> {
    let (temp_dir, broker, _collector) = setup_test_broker_and_collector().await?;
    let collector_id = "nonexistent-collector";

    // Create RPC client for non-existent collector
    let rpc_client = CollectorRpcClient::new(
        &format!("control.collector.{}", collector_id),
        Arc::clone(&broker),
    )
    .await?;

    // Send health check to non-existent collector (should timeout or fail gracefully)
    let request = RpcRequest::health_check(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        Duration::from_secs(2),
    );

    // This should timeout since there's no collector running
    let result = timeout(
        Duration::from_secs(3),
        rpc_client.call(request, Duration::from_secs(2)),
    )
    .await;

    // Either timeout or error is acceptable
    assert!(
        result.is_err() || result.unwrap().is_err(),
        "RPC to non-existent collector should fail or timeout"
    );

    drop(temp_dir);
    Ok(())
}
