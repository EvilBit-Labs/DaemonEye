//! Integration tests for collector-side RPC server functionality.
//!
//! These tests validate that collectors can receive and respond to RPC requests
//! for lifecycle operations, health checks, and configuration updates.

use collector_core::{Collector, CollectorConfig};
use daemoneye_eventbus::{
    DaemoneyeBroker,
    process_manager::{CollectorProcessManager, ProcessManagerConfig},
    rpc::{
        CollectorOperation, CollectorRpcClient, CollectorRpcService, RpcRequest, RpcStatus,
        ServiceCapabilities, TimeoutLimits,
    },
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

/// Initialize tracing for tests
fn init_tracing() {
    // Initialize tracing subscriber - ignore errors if already initialized
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug")))
        .with(fmt::layer().with_test_writer())
        .try_init();
}

/// Spawn a registration handler that auto-accepts registration requests.
/// Returns a shutdown sender to stop the handler.
async fn spawn_registration_handler(
    broker: Arc<DaemoneyeBroker>,
    registration_topic: &str,
) -> anyhow::Result<oneshot::Sender<()>> {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let topic = registration_topic.to_string();

    // Create a simple RPC service to handle registration requests
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Register, CollectorOperation::Deregister],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test".to_string()],
    };
    let process_manager = CollectorProcessManager::new(ProcessManagerConfig::default());
    let rpc_service = Arc::new(CollectorRpcService::new(
        "registration-handler".to_string(),
        capabilities,
        process_manager,
    ));

    // Subscribe to registration topic
    let subscriber_id = Uuid::new_v4();
    let mut receiver = broker.subscribe_raw(&topic, subscriber_id).await?;

    // Spawn handler task
    let broker_clone = Arc::clone(&broker);
    eprintln!(
        "REG_HANDLER: Starting registration handler on topic: {}",
        topic
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    eprintln!("REG_HANDLER: Shutdown signal received");
                    break;
                }
                msg = receiver.recv() => {
                    let Some(message) = msg else {
                        eprintln!("REG_HANDLER: Channel closed");
                        break;
                    };
                    eprintln!("REG_HANDLER: Received message, payload size: {}", message.payload.len());

                    // Deserialize RPC request
                    let request: RpcRequest = match bincode::serde::decode_from_slice(
                        &message.payload,
                        bincode::config::standard(),
                    ) {
                        Ok((req, _)) => req,
                        Err(e) => {
                            eprintln!("REG_HANDLER: Failed to deserialize request: {:?}", e);
                            continue;
                        }
                    };
                    eprintln!("REG_HANDLER: Received request: operation={:?}, client_id={}", request.operation, request.client_id);

                    // Handle the request
                    let response = rpc_service.handle_request(request.clone()).await;
                    eprintln!("REG_HANDLER: Response status: {:?}", response.status);

                    // Serialize and publish response to the client's response topic
                    let response_topic = format!("control.rpc.response.{}", request.client_id);
                    eprintln!("REG_HANDLER: Publishing response to topic: {}", response_topic);
                    if let Ok(payload) = bincode::serde::encode_to_vec(&response, bincode::config::standard()) {
                        let result = broker_clone.publish(&response_topic, &response.request_id, payload).await;
                        eprintln!("REG_HANDLER: Publish result: {:?}", result.is_ok());
                    }
                }
            }
        }
    });

    Ok(shutdown_tx)
}

/// Setup test broker and collector with registration handler
async fn setup_test_broker_and_collector() -> anyhow::Result<(
    TempDir,
    Arc<DaemoneyeBroker>,
    Collector,
    oneshot::Sender<()>,
)> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test-broker.sock");

    let broker = Arc::new(DaemoneyeBroker::new(&socket_path.to_string_lossy()).await?);

    // Start registration handler BEFORE creating collector
    let registration_topic = "control.collector.registration";
    let shutdown_tx = spawn_registration_handler(Arc::clone(&broker), registration_topic).await?;

    let config = CollectorConfig {
        registration: Some(collector_core::CollectorRegistrationConfig {
            enabled: true,
            broker: Some(Arc::clone(&broker)),
            collector_id: Some("test-collector".to_string()),
            collector_type: Some("test".to_string()),
            topic: registration_topic.to_string(),
            timeout: Duration::from_secs(10),
            retry_attempts: 3,
            heartbeat_interval: Duration::from_secs(5),
            attributes: std::collections::HashMap::new(),
        }),
        ..Default::default()
    };

    let collector = Collector::new(config);

    Ok((temp_dir, broker, collector, shutdown_tx))
}

#[tokio::test]
async fn test_rpc_service_initialization() -> anyhow::Result<()> {
    let (_temp_dir, _broker, _collector, _shutdown_tx) = setup_test_broker_and_collector().await?;

    // Test that collector can be created with RPC service configuration
    // The RPC service will be started automatically during collector.run()
    // This test validates the setup function works correctly

    Ok(())
}

#[tokio::test]
async fn test_health_check_response() -> anyhow::Result<()> {
    init_tracing();
    eprintln!("TEST: Starting test_health_check_response");

    let (temp_dir, broker, collector, reg_shutdown_tx) = setup_test_broker_and_collector().await?;
    eprintln!("TEST: Setup complete");
    let collector_id = "test-collector";

    // Start collector in background task
    eprintln!("TEST: Starting collector");
    let collector_handle = tokio::spawn(async move {
        eprintln!("TEST: collector.run() starting");
        let result = collector.run().await;
        eprintln!("TEST: collector.run() completed with {:?}", result.is_ok());
    });

    // Wait for collector to start, register, and RPC service to be ready
    eprintln!("TEST: Waiting for collector to start (2 seconds)");
    tokio::time::sleep(Duration::from_millis(2000)).await;
    eprintln!("TEST: Done waiting");

    // Create RPC client
    let target_topic = format!("control.collector.{}", collector_id);
    eprintln!("TEST: Creating RPC client for topic: {}", target_topic);
    let rpc_client = CollectorRpcClient::new(&target_topic, Arc::clone(&broker)).await?;
    eprintln!(
        "TEST: RPC client created, client_id: {}",
        rpc_client.client_id
    );

    // Send health check RPC
    eprintln!(
        "TEST: Sending health check to topic: {}",
        rpc_client.target_topic
    );
    let request = RpcRequest::health_check(
        rpc_client.client_id.clone(),
        rpc_client.target_topic.clone(),
        Duration::from_secs(10),
    );
    eprintln!("TEST: Request id: {}", request.request_id);

    let response = timeout(
        Duration::from_secs(5),
        rpc_client.call(request, Duration::from_secs(10)),
    )
    .await??;

    eprintln!("TEST: Response status: {:?}", response.status);
    eprintln!("TEST: Response error_details: {:?}", response.error_details);
    eprintln!("TEST: Response payload: {:?}", response.payload);

    // Assert response
    assert_eq!(
        response.status,
        RpcStatus::Success,
        "Health check should succeed"
    );

    // Cleanup
    eprintln!("TEST: Cleaning up");
    collector_handle.abort();
    let _ = reg_shutdown_tx.send(());
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
async fn test_lifecycle_operation_handling() -> anyhow::Result<()> {
    let (temp_dir, broker, collector, reg_shutdown_tx) = setup_test_broker_and_collector().await?;
    let collector_id = "test-collector";

    // Start collector in background task
    let collector_handle = tokio::spawn(async move {
        let _ = collector.run().await;
    });

    // Wait for collector to start, register, and RPC service to be ready
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
    let _ = reg_shutdown_tx.send(());
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
async fn test_graceful_shutdown_via_rpc() -> anyhow::Result<()> {
    let (temp_dir, broker, collector, reg_shutdown_tx) = setup_test_broker_and_collector().await?;
    let collector_id = "test-collector";

    // Start collector in background task
    let collector_handle = tokio::spawn(async move {
        let _ = collector.run().await;
    });

    // Wait for collector to start, register, and RPC service to be ready
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Create RPC client
    let rpc_client = CollectorRpcClient::new(
        &format!("control.collector.{}", collector_id),
        Arc::clone(&broker),
    )
    .await?;

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

    // Wait for collector to shut down
    let _ = timeout(Duration::from_secs(5), collector_handle).await;
    let _ = reg_shutdown_tx.send(());
    drop(temp_dir);

    Ok(())
}

#[tokio::test]
async fn test_rpc_service_error_handling() -> anyhow::Result<()> {
    let (temp_dir, broker, _collector, reg_shutdown_tx) = setup_test_broker_and_collector().await?;
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

    let _ = reg_shutdown_tx.send(());
    drop(temp_dir);
    Ok(())
}
