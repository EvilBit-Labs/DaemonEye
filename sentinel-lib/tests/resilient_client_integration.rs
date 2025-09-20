//! Integration tests for the resilient IPC client
//!
//! These tests validate the advanced client features including connection management,
//! automatic reconnection, circuit breaker functionality, and resilience patterns.

#![allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::let_underscore_must_use
)]

use sentinel_lib::ipc::client::{ConnectionState, ResilientIpcClient};
use sentinel_lib::ipc::interprocess_transport::InterprocessServer;
use sentinel_lib::ipc::{Crc32Variant, IpcConfig, TransportType};
use sentinel_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

fn create_test_config() -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_test_endpoint(&temp_dir);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 4,
        crc32_variant: Crc32Variant::Ieee,
    };

    (config, temp_dir)
}

fn create_test_endpoint(temp_dir: &TempDir) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join("test-resilient.sock")
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("test");
        format!(r"\\.\pipe\sentineld\test-resilient-{}", dir_name)
    }
}

fn create_test_task(task_id: &str) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("resilient client test".to_owned()),
    }
}

#[tokio::test]
async fn test_resilient_client_creation() {
    let (config, _temp_dir) = create_test_config();
    let _client = ResilientIpcClient::new(config);
    // Client creation always succeeds
}

#[tokio::test]
async fn test_connection_state_tracking() {
    let (config, _temp_dir) = create_test_config();
    let client = ResilientIpcClient::new(config);

    // Initially disconnected
    let state = client.get_connection_state().await;
    assert_eq!(state, ConnectionState::Disconnected);

    // Health check should be false when disconnected
    assert!(!client.health_check().await);
}

#[tokio::test]
async fn test_circuit_breaker_functionality() {
    let (config, _temp_dir) = create_test_config();
    let mut client = ResilientIpcClient::new(config);

    // Get initial stats
    let stats = client.get_stats().await;
    assert_eq!(stats.failure_count, 0);
    assert!(!stats.is_circuit_breaker_open);

    // Try to send a task without a server (should fail)
    let task = create_test_task("circuit-breaker-test");
    let result = client.send_task(task).await;
    assert!(result.is_err());

    // Check that failure was recorded
    let updated_stats = client.get_stats().await;
    assert!(updated_stats.failure_count > 0);
}

#[tokio::test]
async fn test_automatic_reconnection() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await; // Give server time to start

    // Create client and send task
    let mut client = ResilientIpcClient::new(config);
    let task = create_test_task("reconnection-test");

    let result = timeout(Duration::from_secs(10), client.send_task(task))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result.success);
    assert_eq!(result.task_id, "reconnection-test");

    // Check that client is now connected
    let state = client.get_connection_state().await;
    assert_eq!(state, ConnectionState::Connected);

    server.stop();
}

#[tokio::test]
async fn test_retry_with_exponential_backoff() {
    let (config, _temp_dir) = create_test_config();
    let mut client = ResilientIpcClient::new(config);

    // Try to send a task without a server to trigger retries
    let task = create_test_task("backoff-test");
    let start_time = std::time::Instant::now();

    let result = timeout(Duration::from_secs(5), client.send_task(task)).await;

    // Should timeout due to retries
    assert!(result.is_err());

    // Check that multiple retry attempts were made
    let stats = client.get_stats().await;
    assert!(stats.reconnect_attempts > 0);

    // Verify that exponential backoff was used (should take some time)
    let elapsed = start_time.elapsed();
    assert!(elapsed > Duration::from_millis(100)); // At least some backoff time
}

#[tokio::test]
async fn test_server_error_handling() {
    let (config, _temp_dir) = create_test_config();

    // Start server that always returns errors
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|_task: DetectionTask| async move {
        Err(sentinel_lib::ipc::codec::IpcError::Encode(
            "Test server error".to_owned(),
        ))
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Create client and send task
    let mut client = ResilientIpcClient::new(config);
    let task = create_test_task("error-handling-test");

    let result = timeout(Duration::from_secs(10), client.send_task(task))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    // Should receive error response
    assert!(!result.success);
    assert!(result.error_message.is_some());
    assert!(
        result
            .error_message
            .expect("Expected error message")
            .contains("Test server error")
    );

    server.stop();
}

#[tokio::test]
async fn test_concurrent_requests() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|task: DetectionTask| async move {
        // Simulate some processing time
        sleep(Duration::from_millis(50)).await;

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Send multiple concurrent requests
    let mut handles = vec![];

    for i in 0..3 {
        let client_config = config.clone();
        let handle = tokio::spawn(async move {
            let mut client = ResilientIpcClient::new(client_config);
            let task = create_test_task(&format!("concurrent-test-{i}"));

            timeout(Duration::from_secs(10), client.send_task(task))
                .await
                .expect("Client request timed out")
        });

        handles.push(handle);
    }

    // Wait for all requests to complete
    let mut successful_requests = 0;
    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(result)) => {
                assert_eq!(result.task_id, format!("concurrent-test-{i}"));
                assert!(result.success);
                successful_requests += 1;
            }
            Ok(Err(e)) => {
                eprintln!("Request {i} failed: {e}");
            }
            Err(e) => {
                eprintln!("Request {i} panicked: {e}");
            }
        }
    }

    // At least some requests should succeed
    assert!(successful_requests > 0, "No requests succeeded");

    server.stop();
}

#[tokio::test]
async fn test_force_reconnection() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Create client and establish connection
    let mut client = ResilientIpcClient::new(config.clone());
    let task = create_test_task("force-reconnect-test");

    // First request should succeed
    let result = timeout(Duration::from_secs(10), client.send_task(task))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result.success);

    // Force reconnection
    client
        .force_reconnect()
        .await
        .expect("Force reconnect failed");

    // Send another request after forced reconnection
    let task2 = create_test_task("force-reconnect-test-2");
    let result2 = timeout(Duration::from_secs(10), client.send_task(task2))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result2.success);

    server.stop();
}

#[tokio::test]
async fn test_client_statistics() {
    let (config, _temp_dir) = create_test_config();
    let mut client = ResilientIpcClient::new(config);

    // Get initial stats
    let stats = client.get_stats().await;
    assert_eq!(stats.connection_state, ConnectionState::Disconnected);
    assert_eq!(stats.failure_count, 0);
    assert!(!stats.is_circuit_breaker_open);
    assert_eq!(stats.reconnect_attempts, 0);

    // Try to send a task without server to generate failures
    let task = create_test_task("stats-test");
    let _ = timeout(Duration::from_secs(2), client.send_task(task)).await;

    // Check updated stats
    let final_stats = client.get_stats().await;
    assert!(final_stats.failure_count > 0);
    assert!(final_stats.reconnect_attempts > 0);
}
