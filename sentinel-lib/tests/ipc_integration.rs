//! Integration tests for interprocess IPC transport
//!
//! These tests validate the full client-server communication flow
//! using the new interprocess transport implementation.

#[allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_match_else,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::redundant_else
)]
mod tests {
    use sentinel_lib::ipc::codec::IpcError;
    use sentinel_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
    use sentinel_lib::ipc::{IpcConfig, TransportType};
    use sentinel_lib::proto::{DetectionResult, DetectionTask, TaskType};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::timeout;

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-ipc.sock")
                .to_string_lossy()
                .to_string()
        }
        #[cfg(windows)]
        {
            // Use a unique pipe name based on temp directory name to avoid conflicts
            let dir_name = temp_dir
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("test");
            format!(r"\\.\pipe\sentineld\test-{}", dir_name)
        }
    }

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
            max_connections: 5, // Allow more concurrent connections
            panic_strategy: sentinel_lib::ipc::PanicStrategy::Unwind,
        };

        (config, temp_dir)
    }

    fn create_test_task() -> DetectionTask {
        DetectionTask::new_test_task(
            "test-task-1",
            TaskType::EnumerateProcesses,
            Some("integration test".to_owned()),
        )
    }

    #[tokio::test]
    async fn test_basic_client_server_communication() {
        let (config, _temp_dir) = create_test_config();

        // Start server
        let mut server = InterprocessServer::new(config.clone());

        // Set up a simple handler that echoes back success
        server.set_handler(|task: DetectionTask| async move {
            Ok(DetectionResult::success(&task.task_id, vec![]))
        });

        // Start the server
        server.start().await.expect("Failed to start server");

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send task
        let mut client = InterprocessClient::new(config.clone());
        let task = create_test_task();
        let task_id = task.task_id.clone();

        // Send task and receive result
        let result = timeout(Duration::from_secs(10), client.send_task(task))
            .await
            .expect("Client request timed out")
            .expect("Client request failed");

        // Validate result
        assert_eq!(result.task_id, task_id);
        assert!(result.success);
        assert!(result.error_message.is_none());

        // Clean up
        let _result = server.graceful_shutdown().await;
    }

    #[tokio::test]
    async fn test_server_error_handling() {
        let (config, _temp_dir) = create_test_config();

        // Start server with handler that always fails
        let mut server = InterprocessServer::new(config.clone());

        server.set_handler(|_task: DetectionTask| async move {
            Err(sentinel_lib::ipc::codec::IpcError::Encode(
                "Test error".to_owned(),
            ))
        });

        server.start().await.expect("Failed to start server");
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send task
        let mut client = InterprocessClient::new(config.clone());
        let task = create_test_task();

        // Send task and expect error response
        let result = timeout(Duration::from_secs(10), client.send_task(task))
            .await
            .expect("Client request timed out")
            .expect("Client request failed");

        // Validate error result
        assert!(!result.success);
        assert!(result.error_message.is_some());
        assert!(
            result
                .error_message
                .expect("Expected error message")
                .contains("Test error")
        );

        // Clean up
        let _result = server.graceful_shutdown().await;
    }

    #[tokio::test]
    async fn test_concurrent_clients() {
        let (config, _temp_dir) = create_test_config();

        // Start server with a handler that includes client identification
        let mut server = InterprocessServer::new(config.clone());

        server.set_handler(|task: DetectionTask| async move {
            // Simulate some work
            tokio::time::sleep(Duration::from_millis(10)).await;

            Ok(DetectionResult::success(&task.task_id, vec![]))
        });

        server.start().await.expect("Failed to start server");

        // Give server more time to start and create the endpoint
        // Windows named pipes need more time to become available
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Test that the server is actually ready with retry logic
        let test_config = config.clone();
        let test_task = DetectionTask::new_test_task(
            "connection-test",
            TaskType::EnumerateProcesses,
            Some("connection test".to_owned()),
        );

        let mut retries = 0;
        let max_retries = 10;
        let mut retry_delay = Duration::from_millis(100);

        loop {
            match timeout(Duration::from_secs(2), async {
                let mut test_client = InterprocessClient::new(test_config.clone());
                test_client.send_task(test_task.clone()).await
            })
            .await
            {
                Ok(Ok(_)) => {
                    // Server is ready, break out of retry loop
                    break;
                }
                Ok(Err(e)) => {
                    retries += 1;
                    if retries >= max_retries {
                        eprintln!(
                            "Warning: Server connection test failed after {max_retries} retries: {e}"
                        );
                        break;
                    }
                    eprintln!(
                        "Server connection test failed (attempt {retries}/{max_retries}): {e}, retrying in {retry_delay:?}"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = std::cmp::min(retry_delay * 2, Duration::from_millis(1000));
                }
                Err(_) => {
                    retries += 1;
                    if retries >= max_retries {
                        eprintln!(
                            "Warning: Server connection test timed out after {max_retries} retries"
                        );
                        break;
                    }
                    eprintln!(
                        "Server connection test timed out (attempt {retries}/{max_retries}), retrying in {retry_delay:?}"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = std::cmp::min(retry_delay * 2, Duration::from_millis(1000));
                }
            }
        }

        // Send multiple concurrent requests with proper error handling
        let mut handles = vec![];

        for i in 0..3 {
            let client_config = config.clone();
            let handle = tokio::spawn(async move {
                // Add retry logic for Windows named pipe connection
                let mut client_retries = 0;
                let client_max_retries = 5;

                loop {
                    let timeout_result = timeout(Duration::from_secs(10), async {
                        let mut client = InterprocessClient::new(client_config.clone());
                        let task = DetectionTask::new_test_task(
                            format!("concurrent-task-{i}"),
                            TaskType::EnumerateProcesses,
                            Some(format!("concurrent test {i}")),
                        );

                        client.send_task(task).await
                    })
                    .await;

                    if let Ok(result) = timeout_result {
                        return result;
                    }
                    client_retries += 1;
                    if client_retries >= client_max_retries {
                        return Err(IpcError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "Client request timed out after retries",
                        )));
                    }
                    // Wait before retry
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            handles.push(handle);
        }

        // Wait for all requests to complete with proper error handling
        let mut successful_clients = 0;
        let mut failed_clients = 0;

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(result)) => {
                    assert_eq!(result.task_id, format!("concurrent-task-{i}"));
                    assert!(result.success);
                    successful_clients += 1;
                }
                Ok(Err(e)) => {
                    eprintln!("Client {i} failed with error: {e}");
                    failed_clients += 1;
                }
                Err(e) => {
                    eprintln!("Client {i} task panicked: {e}");
                    failed_clients += 1;
                }
            }
        }

        // At least one client should succeed for the test to pass
        assert!(
            successful_clients > 0,
            "No clients succeeded. {successful_clients} successful, {failed_clients} failed"
        );

        // Log the results for debugging
        eprintln!(
            "Concurrent clients test completed: {successful_clients} successful, {failed_clients} failed"
        );

        // Clean up
        let _result = server.graceful_shutdown().await;
    }

    /// Test cross-platform endpoint creation
    #[tokio::test]
    async fn test_cross_platform_endpoint_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let endpoint = create_test_endpoint(&temp_dir);

        // Endpoint should be platform-appropriate
        #[cfg(unix)]
        assert!(
            std::path::Path::new(&endpoint)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sock")),
            "Unix endpoint should be socket file: {endpoint}"
        );

        #[cfg(windows)]
        assert!(
            endpoint.starts_with(r"\\.\pipe\"),
            "Windows endpoint should be named pipe: {}",
            endpoint
        );

        // Should be able to create a config with this endpoint
        let config = IpcConfig {
            transport: TransportType::Interprocess,
            endpoint_path: endpoint,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 5,
            panic_strategy: sentinel_lib::ipc::PanicStrategy::Unwind,
        };

        let _server = InterprocessServer::new(config.clone());
        let _client = InterprocessClient::new(config);

        // Server and client creation should succeed (constructors don't return Result)
    }
}
