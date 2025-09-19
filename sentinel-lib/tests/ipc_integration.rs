//! Integration tests for interprocess IPC transport
//!
//! These tests validate the full client-server communication flow
//! using the new interprocess transport implementation.

mod tests {
    use sentinel_lib::ipc::codec::IpcError;
    use sentinel_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
    use sentinel_lib::ipc::{Crc32Variant, IpcConfig, TransportType};
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
        let temp_dir = TempDir::new().unwrap();

        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 5, // Allow more concurrent connections
            crc32_variant: Crc32Variant::Ieee,
        };

        (config, temp_dir)
    }

    fn create_test_task() -> DetectionTask {
        DetectionTask {
            task_id: "test-task-1".to_string(),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some("integration test".to_string()),
        }
    }

    #[tokio::test]
    async fn test_basic_client_server_communication() {
        let (config, _temp_dir) = create_test_config();

        // Start server
        let mut server = InterprocessServer::new(config.clone()).unwrap();

        // Set up a simple handler that echoes back success
        server.set_handler(|task: DetectionTask| async move {
            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![], // Empty for this test
                hash_result: None,
            })
        });

        // Start the server
        server.start().await.unwrap();

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send task
        let mut client = InterprocessClient::new(config.clone()).unwrap();
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
        server.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_server_error_handling() {
        let (config, _temp_dir) = create_test_config();

        // Start server with handler that always fails
        let mut server = InterprocessServer::new(config.clone()).unwrap();

        server.set_handler(|_task: DetectionTask| async move {
            Err(sentinel_lib::ipc::codec::IpcError::Encode(
                "Test error".to_string(),
            ))
        });

        server.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send task
        let mut client = InterprocessClient::new(config.clone()).unwrap();
        let task = create_test_task();

        // Send task and expect error response
        let result = timeout(Duration::from_secs(10), client.send_task(task))
            .await
            .expect("Client request timed out")
            .expect("Client request failed");

        // Validate error result
        assert!(!result.success);
        assert!(result.error_message.is_some());
        assert!(result.error_message.unwrap().contains("Test error"));

        // Clean up
        server.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_clients() {
        let (config, _temp_dir) = create_test_config();

        // Start server with a handler that includes client identification
        let mut server = InterprocessServer::new(config.clone()).unwrap();

        server.set_handler(|task: DetectionTask| async move {
            // Simulate some work
            tokio::time::sleep(Duration::from_millis(10)).await;

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![],
                hash_result: None,
            })
        });

        server.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send multiple concurrent requests with proper error handling
        let mut handles = vec![];

        for i in 0..3 {
            let config = config.clone();
            let handle = tokio::spawn(async move {
                // Add timeout to prevent hanging
                timeout(Duration::from_secs(10), async {
                    let mut client = InterprocessClient::new(config)?;
                    let task = DetectionTask {
                        task_id: format!("concurrent-task-{}", i),
                        task_type: TaskType::EnumerateProcesses as i32,
                        process_filter: None,
                        hash_check: None,
                        metadata: Some(format!("concurrent test {}", i)),
                    };

                    client.send_task(task).await
                })
                .await
                .map_err(|_| {
                    IpcError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Client request timed out",
                    ))
                })?
            });

            handles.push(handle);
        }

        // Wait for all requests to complete with proper error handling
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(result)) => {
                    assert_eq!(result.task_id, format!("concurrent-task-{}", i));
                    assert!(result.success);
                }
                Ok(Err(e)) => {
                    panic!("Client {} failed with error: {}", i, e);
                }
                Err(e) => {
                    panic!("Client {} task panicked: {}", i, e);
                }
            }
        }

        // Clean up
        server.stop().await.unwrap();
    }

    /// Test cross-platform endpoint creation
    #[tokio::test]
    async fn test_cross_platform_endpoint_creation() {
        let temp_dir = TempDir::new().unwrap();
        let endpoint = create_test_endpoint(&temp_dir);

        // Endpoint should be platform-appropriate
        #[cfg(unix)]
        assert!(
            endpoint.ends_with(".sock"),
            "Unix endpoint should be socket file: {}",
            endpoint
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
            crc32_variant: Crc32Variant::Ieee,
        };

        let server = InterprocessServer::new(config.clone());
        let client = InterprocessClient::new(config);

        assert!(server.is_ok(), "Server creation should succeed");
        assert!(client.is_ok(), "Client creation should succeed");
    }
}
