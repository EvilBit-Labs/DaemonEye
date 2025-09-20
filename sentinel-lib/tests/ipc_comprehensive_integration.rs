//! Comprehensive integration tests for interprocess communication
//!
//! This test suite validates the complete IPC transport behavior including:
//! - Cross-platform local socket functionality
//! - Task distribution and result collection workflows
//! - Error handling and recovery scenarios
//! - Security validation for socket permissions and connection limits

#![allow(
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

use sentinel_lib::ipc::codec::IpcError;
use sentinel_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
use sentinel_lib::ipc::{IpcConfig, TransportType};
use sentinel_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::Barrier;
use tokio::time::{sleep, timeout};

/// Create a test configuration with unique endpoint
fn create_test_config(test_name: &str) -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_test_endpoint(&temp_dir, test_name);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 2000,
        read_timeout_ms: 10000,
        write_timeout_ms: 10000,
        max_connections: 8,
    };

    (config, temp_dir)
}

/// Create platform-specific test endpoint
fn create_test_endpoint(temp_dir: &TempDir, test_name: &str) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join(format!("{}.sock", test_name))
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
        format!(r"\\.\pipe\sentineld\{}-{}", test_name, dir_name)
    }
}

/// Create a test detection task
fn create_test_task(task_id: &str) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("comprehensive integration test".to_owned()),
    }
}

/// Create a test process record
fn create_test_process_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        pid,
        ppid: Some(1),
        name: format!("test_process_{}", pid),
        executable_path: Some(format!("/usr/bin/test_{}", pid)),
        command_line: vec![
            format!("test_{}", pid),
            "--arg".to_owned(),
            "value".to_owned(),
        ],
        start_time: Some(chrono::Utc::now().timestamp()),
        cpu_usage: Some(25.5),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some(format!("hash_{:08x}", pid)),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        collection_time: chrono::Utc::now().timestamp_millis(),
    }
}

/// Test cross-platform transport behavior
#[tokio::test]
async fn test_cross_platform_transport_behavior() {
    let (config, _temp_dir) = create_test_config("cross_platform");

    // Test server creation and startup
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![create_test_process_record(1234)],
            hash_result: None,
        })
    });

    // Start server and verify it binds correctly
    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Test client connection and communication
    let mut client = InterprocessClient::new(config.clone());
    let task = create_test_task("cross_platform_test");

    let result = timeout(Duration::from_secs(10), client.send_task(task))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result.success);
    assert_eq!(result.processes.len(), 1);
    assert_eq!(result.processes.first().map(|p| p.pid), Some(1234));

    // Verify platform-specific endpoint behavior
    #[cfg(unix)]
    {
        // Unix socket should exist as file
        assert!(std::path::Path::new(&config.endpoint_path).exists());

        // Check socket permissions (should be 0600)
        let metadata =
            std::fs::metadata(&config.endpoint_path).expect("Failed to get socket metadata");
        let permissions = metadata.permissions();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = permissions.mode();
            // Socket should have owner read/write only (0600)
            assert_eq!(mode & 0o777, 0o600, "Socket permissions should be 0600");
        }
    }

    #[cfg(windows)]
    {
        // Windows named pipe should be accessible
        assert!(config.endpoint_path.starts_with(r"\\.\pipe\"));
    }

    server.stop();
}

/// Test task distribution and result collection workflows
#[tokio::test]
async fn test_task_distribution_workflows() {
    let (config, _temp_dir) = create_test_config("task_distribution");
    let task_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&task_counter);

    // Start server with task counting handler
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let task_num = counter.fetch_add(1, Ordering::SeqCst);

            // Simulate different task types and processing times
            let processes = match task.task_type {
                x if x == TaskType::EnumerateProcesses as i32 => {
                    vec![
                        create_test_process_record(1000 + task_num),
                        create_test_process_record(2000 + task_num),
                    ]
                }
                x if x == TaskType::CheckProcessHash as i32 => {
                    vec![create_test_process_record(3000 + task_num)]
                }
                _ => vec![],
            };

            // Add processing delay to simulate real work
            sleep(Duration::from_millis(50)).await;

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes,
                hash_result: None,
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Test multiple task types
    let mut client = InterprocessClient::new(config.clone());

    // Test enumerate processes task
    let enum_task = DetectionTask {
        task_id: "enum_test".to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("enumerate test".to_owned()),
    };

    let enum_result = timeout(Duration::from_secs(10), client.send_task(enum_task))
        .await
        .expect("Enumerate task timed out")
        .expect("Enumerate task failed");

    assert!(enum_result.success);
    assert_eq!(enum_result.processes.len(), 2);
    assert_eq!(enum_result.task_id, "enum_test");

    // Test hash check task
    let hash_task = DetectionTask {
        task_id: "hash_test".to_owned(),
        task_type: TaskType::CheckProcessHash.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("hash check test".to_owned()),
    };

    let hash_result = timeout(Duration::from_secs(10), client.send_task(hash_task))
        .await
        .expect("Hash task timed out")
        .expect("Hash task failed");

    assert!(hash_result.success);
    assert_eq!(hash_result.processes.len(), 1);
    assert_eq!(hash_result.task_id, "hash_test");

    // Verify task counter
    assert_eq!(task_counter.load(Ordering::SeqCst), 2);

    server.stop();
}

/// Test concurrent task distribution
#[tokio::test]
async fn test_concurrent_task_distribution() {
    let (config, _temp_dir) = create_test_config("concurrent_tasks");
    let task_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&task_counter);

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let task_num = counter.fetch_add(1, Ordering::SeqCst);

            // Simulate processing time
            sleep(Duration::from_millis(100)).await;

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![create_test_process_record(task_num)],
                hash_result: None,
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Send multiple concurrent tasks
    let num_tasks = 5;
    let barrier = Arc::new(Barrier::new(num_tasks));
    let mut handles = vec![];

    for i in 0..num_tasks {
        let client_config = config.clone();
        let task_barrier = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            // Wait for all tasks to be ready
            task_barrier.wait().await;

            let mut client = InterprocessClient::new(client_config);
            let task = create_test_task(&format!("concurrent_task_{}", i));

            let start_time = Instant::now();
            let result = timeout(Duration::from_secs(15), client.send_task(task))
                .await
                .expect("Concurrent task timed out")
                .expect("Concurrent task failed");
            let duration = start_time.elapsed();

            (i, result, duration)
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    let mut results = vec![];
    for handle in handles {
        let (task_id, result, duration) = handle.await.expect("Task handle failed");
        results.push((task_id, result, duration));
    }

    // Verify all tasks completed successfully
    assert_eq!(results.len(), num_tasks);
    for (task_id, result, duration) in results {
        assert!(result.success, "Task {} failed", task_id);
        assert_eq!(result.task_id, format!("concurrent_task_{}", task_id));
        assert!(!result.processes.is_empty());

        // Verify reasonable processing time (should be concurrent, not sequential)
        assert!(
            duration < Duration::from_secs(5),
            "Task {} took too long: {:?}",
            task_id,
            duration
        );
    }

    // Verify all tasks were processed
    assert_eq!(task_counter.load(Ordering::SeqCst), num_tasks as u32);

    server.stop();
}

/// Test error handling and recovery scenarios
#[tokio::test]
async fn test_error_handling_and_recovery() {
    let (config, _temp_dir) = create_test_config("error_handling");
    let error_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&error_counter);

    // Start server with error-prone handler
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let error_count = counter.fetch_add(1, Ordering::SeqCst);

            // Fail first few requests, then succeed
            if error_count < 3 {
                return Err(IpcError::Encode(format!(
                    "Simulated error #{}",
                    error_count + 1
                )));
            }

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![create_test_process_record(1000)],
                hash_result: None,
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Test error responses
    let mut client = InterprocessClient::new(config.clone());

    // First request should fail
    let task1 = create_test_task("error_test_1");
    let result1 = timeout(Duration::from_secs(10), client.send_task(task1))
        .await
        .expect("Error test 1 timed out")
        .expect("Error test 1 failed to get response");

    assert!(!result1.success);
    assert!(result1.error_message.is_some());
    assert!(
        result1
            .error_message
            .as_ref()
            .is_some_and(|msg| msg.contains("Simulated error #1"))
    );

    // Second request should also fail
    let task2 = create_test_task("error_test_2");
    let result2 = timeout(Duration::from_secs(10), client.send_task(task2))
        .await
        .expect("Error test 2 timed out")
        .expect("Error test 2 failed to get response");

    assert!(!result2.success);
    assert!(result2.error_message.is_some());
    assert!(
        result2
            .error_message
            .as_ref()
            .is_some_and(|msg| msg.contains("Simulated error #2"))
    );

    // Third request should also fail
    let task3 = create_test_task("error_test_3");
    let result3 = timeout(Duration::from_secs(10), client.send_task(task3))
        .await
        .expect("Error test 3 timed out")
        .expect("Error test 3 failed to get response");

    assert!(!result3.success);
    assert!(result3.error_message.is_some());
    assert!(
        result3
            .error_message
            .as_ref()
            .is_some_and(|msg| msg.contains("Simulated error #3"))
    );

    // Fourth request should succeed (recovery)
    let task4 = create_test_task("recovery_test");
    let result4 = timeout(Duration::from_secs(10), client.send_task(task4))
        .await
        .expect("Recovery test timed out")
        .expect("Recovery test failed");

    assert!(result4.success);
    assert!(result4.error_message.is_none());
    assert_eq!(result4.processes.len(), 1);

    server.stop();
}

/// Test connection limits and security validation
#[tokio::test]
async fn test_connection_limits_and_security() {
    let (mut config, _temp_dir) = create_test_config("connection_limits");

    // Set a low connection limit for testing
    config.max_connections = 2;

    let connection_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&connection_counter);

    // Start server with connection tracking
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let conn_num = counter.fetch_add(1, Ordering::SeqCst);

            // Hold connection open for a while to test limits
            sleep(Duration::from_millis(500)).await;

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![create_test_process_record(conn_num)],
                hash_result: None,
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Test connection limit enforcement
    let num_clients = 4; // More than the limit of 2
    let mut handles = vec![];

    for i in 0..num_clients {
        let client_config = config.clone();

        let handle = tokio::spawn(async move {
            let mut client = InterprocessClient::new(client_config);
            let task = create_test_task(&format!("limit_test_{}", i));

            // Some connections may be rejected due to limits
            timeout(Duration::from_secs(10), client.send_task(task)).await
        });

        handles.push(handle);
    }

    // Wait for all attempts to complete
    let mut successful_connections = 0;
    let mut failed_connections = 0;

    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(Ok(result))) => {
                assert!(result.success);
                successful_connections += 1;
            }
            Ok(Ok(Err(_))) => {
                // Connection failed (expected due to limits)
                failed_connections += 1;
            }
            Ok(Err(_)) => {
                // Timeout (also expected due to limits)
                failed_connections += 1;
            }
            Err(e) => {
                eprintln!("Task {} panicked: {}", i, e);
                failed_connections += 1;
            }
        }
    }

    // Should have some successful connections but not all due to limits
    assert!(successful_connections > 0, "No connections succeeded");
    assert!(
        successful_connections <= config.max_connections,
        "Too many connections succeeded: {} > {}",
        successful_connections,
        config.max_connections
    );

    eprintln!(
        "Connection limit test: {} successful, {} failed (limit: {})",
        successful_connections, failed_connections, config.max_connections
    );

    server.stop();
}

/// Test server shutdown and cleanup
#[tokio::test]
async fn test_server_shutdown_and_cleanup() {
    let (config, _temp_dir) = create_test_config("shutdown_cleanup");

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

    // Verify server is running
    let mut client = InterprocessClient::new(config.clone());
    let task = create_test_task("shutdown_test");

    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("Pre-shutdown test timed out")
        .expect("Pre-shutdown test failed");

    assert!(result.success);

    // Stop server
    server.stop();

    // Give server time to clean up
    sleep(Duration::from_millis(200)).await;

    // Verify server is no longer accepting connections
    let mut client2 = InterprocessClient::new(config.clone());
    let task2 = create_test_task("post_shutdown_test");

    let result2 = timeout(Duration::from_secs(2), client2.send_task(task2)).await;

    // Should fail to connect or timeout
    assert!(
        result2.is_err() || result2.is_ok_and(|r| r.is_err()),
        "Client should not be able to connect after server shutdown"
    );

    // Verify socket cleanup on Unix
    #[cfg(unix)]
    {
        // Socket file should be removed
        assert!(
            !std::path::Path::new(&config.endpoint_path).exists(),
            "Socket file should be cleaned up after server shutdown"
        );
    }
}

/// Test large message handling
#[tokio::test]
async fn test_large_message_handling() {
    let (config, _temp_dir) = create_test_config("large_messages");

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Create a large response with many process records
        let mut processes = vec![];
        for i in 0..1000 {
            processes.push(create_test_process_record(i));
        }

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes,
            hash_result: None,
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Send task and receive large response
    let mut client = InterprocessClient::new(config.clone());
    let task = create_test_task("large_message_test");

    let result = timeout(Duration::from_secs(30), client.send_task(task))
        .await
        .expect("Large message test timed out")
        .expect("Large message test failed");

    assert!(result.success);
    assert_eq!(result.processes.len(), 1000);

    // Verify all process records are intact
    for (i, process) in result.processes.iter().enumerate() {
        assert_eq!(process.pid, i as u32);
        assert_eq!(process.name, format!("test_process_{}", i));
    }

    server.stop();
}

/// Test timeout handling
#[tokio::test]
async fn test_timeout_handling() {
    let (mut config, _temp_dir) = create_test_config("timeout_handling");

    // Set short timeouts for testing
    config.read_timeout_ms = 1000;
    config.write_timeout_ms = 1000;

    // Start server with slow handler
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Simulate slow processing that exceeds timeout
        sleep(Duration::from_millis(2000)).await;

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

    // Send task that should timeout
    let mut client = InterprocessClient::new(config.clone());
    let task = create_test_task("timeout_test");

    let result = timeout(Duration::from_secs(5), client.send_task(task)).await;

    // Should timeout or receive error response
    match result {
        Ok(Ok(response)) => {
            // If we get a response, it should indicate an error
            assert!(!response.success || response.error_message.is_some());
        }
        Ok(Err(_)) | Err(_) => {
            // Expected: IPC error due to timeout or outer timeout
        }
    }

    server.stop();
}
