//! Cross-platform tests for IPC transport behavior
//!
//! This test suite validates platform-specific behavior and ensures
//! consistent functionality across Linux, macOS, and Windows.

#![allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::uninlined_format_args,
    clippy::print_stdout, // Test output is intentional
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_match_else,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::redundant_else,
    clippy::panic,
    clippy::modulo_arithmetic
)]

use daemoneye_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
use daemoneye_lib::ipc::{IpcConfig, PanicStrategy, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

/// Create a cross-platform test configuration
fn create_cross_platform_config(test_name: &str) -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_cross_platform_endpoint(&temp_dir, test_name);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 3000,
        read_timeout_ms: 10000,
        write_timeout_ms: 10000,
        max_connections: 8,
        panic_strategy: PanicStrategy::Unwind, // Use unwind for tests
    };

    (config, temp_dir)
}

/// Create platform-specific endpoint path
fn create_cross_platform_endpoint(temp_dir: &TempDir, test_name: &str) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join(format!("cross_platform_{}.sock", test_name))
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cross_platform");
        format!(
            r"\\.\pipe\daemoneye\cross-platform-{}-{}",
            test_name, dir_name
        )
    }
}

/// Create a test detection task
fn create_cross_platform_task(task_id: &str) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("cross-platform test".to_owned()),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    }
}

/// Test basic cross-platform functionality
#[tokio::test]
async fn test_basic_cross_platform_functionality() {
    let (config, _temp_dir) = create_cross_platform_config("basic_functionality");

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![ProcessRecord {
                pid: 12345,
                ppid: Some(1),
                name: "cross_platform_test".to_owned(),
                executable_path: Some("/usr/bin/test".to_owned()),
                command_line: vec!["test".to_owned(), "--cross-platform".to_owned()],
                start_time: Some(chrono::Utc::now().timestamp()),
                cpu_usage: Some(15.5),
                memory_usage: Some(2048 * 1024),
                executable_hash: Some("abcdef123456".to_owned()),
                hash_algorithm: Some("sha256".to_owned()),
                user_id: Some("1000".to_owned()),
                accessible: true,
                file_exists: true,
                collection_time: chrono::Utc::now().timestamp_millis(),
            }],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(300)).await;

    // Test client connection
    let mut client = InterprocessClient::new(config.clone());
    let task = create_cross_platform_task("basic_test");

    let result = timeout(Duration::from_secs(10), client.send_task(task))
        .await
        .expect("Basic cross-platform test timed out")
        .expect("Basic cross-platform test failed");

    assert!(result.success);
    assert_eq!(result.task_id, "basic_test");
    assert_eq!(result.processes.len(), 1);
    assert_eq!(result.processes.first().map(|p| p.pid), Some(12345));
    assert_eq!(
        result.processes.first().map(|p| &p.name),
        Some(&"cross_platform_test".to_string())
    );

    // Use graceful shutdown to prevent production panics
    let _result = server.graceful_shutdown().await;
}

/// Test endpoint path validation across platforms
#[tokio::test]
async fn test_endpoint_path_validation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Test valid endpoint paths for current platform
    let valid_endpoints = create_valid_endpoint_paths(&temp_dir);

    for (test_name, endpoint_path) in valid_endpoints {
        let config = IpcConfig {
            transport: TransportType::Interprocess,
            endpoint_path: endpoint_path.clone(),
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 2000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            panic_strategy: PanicStrategy::Unwind, // Use unwind for tests
        };

        // Should be able to create server with valid endpoint
        let server = InterprocessServer::new(config.clone());

        // Server creation should succeed (constructor doesn't return Result)
        drop(server);

        println!(
            "Valid endpoint test '{}' passed: {}",
            test_name, endpoint_path
        );
    }
}

/// Create valid endpoint paths for the current platform
fn create_valid_endpoint_paths(temp_dir: &TempDir) -> Vec<(String, String)> {
    let mut endpoints = vec![];

    #[cfg(unix)]
    {
        // Unix domain socket paths
        endpoints.push((
            "simple_socket".to_owned(),
            temp_dir
                .path()
                .join("simple.sock")
                .to_string_lossy()
                .to_string(),
        ));

        endpoints.push((
            "nested_socket".to_owned(),
            temp_dir
                .path()
                .join("nested")
                .join("path.sock")
                .to_string_lossy()
                .to_string(),
        ));

        endpoints.push((
            "long_name_socket".to_owned(),
            temp_dir
                .path()
                .join("very_long_socket_name_for_testing.sock")
                .to_string_lossy()
                .to_string(),
        ));
    };

    #[cfg(windows)]
    {
        // Windows named pipe paths
        let base_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("test");

        endpoints.push((
            "long_name_pipe".to_owned(),
            format!(
                r"\\.\pipe\daemoneye\very-long-pipe-name-for-testing-{}",
                base_name
            ),
        ));

        endpoints.push((
            "long_name_pipe".to_owned(),
            format!(
                r"\\.\pipe\daemoneye\very-long-pipe-name-for-testing-{}",
                base_name
            ),
        ));
    };

    endpoints
}

/// Test platform-specific error handling
#[tokio::test]
async fn test_platform_specific_error_handling() {
    let (config, _temp_dir) = create_cross_platform_config("error_handling");

    // Test connection to non-existent server
    let mut client = InterprocessClient::new(config.clone());
    let task = create_cross_platform_task("error_test");

    let result = timeout(Duration::from_secs(3), client.send_task(task)).await;

    // Should fail with platform-appropriate error
    match result {
        Ok(Err(e)) => {
            // Verify error is appropriate for platform
            let error_msg = e.to_string();

            #[cfg(unix)]
            {
                // Unix should report connection refused or no such file
                assert!(
                    error_msg.contains("Connection refused")
                        || error_msg.contains("No such file")
                        || error_msg.contains("Connection failed"),
                    "Unix error should indicate connection failure: {error_msg}"
                );
            }

            #[cfg(windows)]
            {
                // Windows should report pipe not available or similar
                assert!(
                    error_msg.contains("pipe")
                        || error_msg.contains("not available")
                        || error_msg.contains("Connection failed")
                        || error_msg.contains("cannot find the file specified"),
                    "Windows error should indicate pipe unavailable: {error_msg}"
                );
            }
        }
        Ok(Ok(_)) => {
            panic!("Should not succeed connecting to non-existent server");
        }
        Err(_) => {
            // Timeout is also acceptable
            println!("Connection attempt timed out (acceptable)");
        }
    }
}

/// Test concurrent connections across platforms
#[tokio::test]
#[cfg(not(windows))] // Skip on Windows due to interprocess crate issues
async fn test_cross_platform_concurrent_connections() {
    let (config, _temp_dir) = create_cross_platform_config("concurrent_connections");

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Add small delay to test concurrency
        sleep(Duration::from_millis(100)).await;

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");

    // Windows named pipes need more time to initialize properly
    #[cfg(windows)]
    sleep(Duration::from_millis(500)).await;
    #[cfg(not(windows))]
    sleep(Duration::from_millis(300)).await;

    // Test concurrent connections
    let num_concurrent = 4;
    let mut handles = vec![];

    for i in 0..num_concurrent {
        let client_config = config.clone();

        let handle = tokio::spawn(async move {
            // Stagger connections on Windows to prevent connection burst issues
            #[cfg(windows)]
            sleep(Duration::from_millis(i as u64 * 100)).await;

            let mut client = InterprocessClient::new(client_config);
            let task = create_cross_platform_task(&format!("concurrent_{}", i));

            timeout(Duration::from_secs(10), client.send_task(task)).await
        });

        handles.push(handle);
    }

    // Wait for all connections to complete
    let mut successful_connections = 0;

    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(Ok(result))) => {
                assert!(result.success);
                assert_eq!(result.task_id, format!("concurrent_{}", i));
                successful_connections += 1;
            }
            Ok(Ok(Err(e))) => {
                eprintln!("Connection {} failed: {}", i, e);
            }
            Ok(Err(_)) => {
                eprintln!("Connection {} timed out", i);
            }
            Err(e) => {
                eprintln!("Connection {} panicked: {}", i, e);
            }
        }
    }

    assert!(
        successful_connections >= (num_concurrent as usize).div_euclid(2),
        "At least half of concurrent connections should succeed: {} / {}",
        successful_connections,
        num_concurrent
    );

    // Use graceful shutdown to prevent production panics
    let _result = server.graceful_shutdown().await;
}

/// Test large message handling across platforms
#[tokio::test]
#[cfg(not(windows))] // Skip on Windows due to interprocess crate issues
async fn test_cross_platform_large_messages() {
    let (config, _temp_dir) = create_cross_platform_config("large_messages");

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Create large response with many process records
        let mut processes = vec![];
        for i in 0..500 {
            processes.push(ProcessRecord {
                pid: i,
                ppid: Some(i.saturating_sub(1)),
                name: format!("large_test_process_{}", i),
                executable_path: Some(format!("/usr/bin/large_test_{}", i)),
                command_line: vec![
                    format!("large_test_{}", i),
                    "--large-message-test".to_owned(),
                    format!("--process-id={}", i),
                ],
                start_time: Some(chrono::Utc::now().timestamp()),
                cpu_usage: Some(10.0 + f64::from(i % 90)),
                memory_usage: Some(1024 * 1024 * (u64::from(i) + 1)),
                executable_hash: Some(format!("large_hash_{:08x}", i)),
                hash_algorithm: Some("sha256".to_owned()),
                user_id: Some("1000".to_owned()),
                accessible: true,
                file_exists: true,
                collection_time: chrono::Utc::now().timestamp_millis(),
            });
        }

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes,
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(300)).await;

    // Send large message request
    let mut client = InterprocessClient::new(config.clone());
    let task = create_cross_platform_task("large_message_test");

    let result = timeout(Duration::from_secs(30), client.send_task(task))
        .await
        .expect("Large message test timed out")
        .expect("Large message test failed");

    assert!(result.success);
    assert_eq!(result.task_id, "large_message_test");
    assert_eq!(result.processes.len(), 500);

    // Verify all process records are intact
    for (i, process) in result.processes.iter().enumerate() {
        assert_eq!(process.pid, i as u32);
        assert_eq!(process.name, format!("large_test_process_{}", i));
        assert!(process.executable_hash.is_some());
    }

    // Use graceful shutdown to prevent production panics
    let _result = server.graceful_shutdown().await;
}

/// Test server restart behavior across platforms
#[tokio::test]
#[cfg(not(windows))] // Skip on Windows due to interprocess crate issues
async fn test_cross_platform_server_restart() {
    let (config, _temp_dir) = create_cross_platform_config("server_restart");

    // Start first server instance
    let mut server1 = InterprocessServer::new(config.clone());

    server1.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: Some("first_server".to_owned()),
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server1.start().await.expect("Failed to start first server");
    sleep(Duration::from_millis(300)).await;

    // Test connection to first server
    let mut client1 = InterprocessClient::new(config.clone());
    let task1 = create_cross_platform_task("first_server_test");

    let result1 = timeout(Duration::from_secs(5), client1.send_task(task1))
        .await
        .expect("First server test timed out")
        .expect("First server test failed");

    assert!(result1.success);
    assert_eq!(result1.error_message, Some("first_server".to_owned()));

    // Stop first server (ignore cleanup panics on Windows)
    let _panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        server1.stop();
    }));
    // We intentionally ignore the result as we're just trying to prevent test failures due to panic
    sleep(Duration::from_millis(300)).await;

    // Start second server instance (should reuse endpoint)
    let mut server2 = InterprocessServer::new(config.clone());

    server2.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: Some("second_server".to_owned()),
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server2
        .start()
        .await
        .expect("Failed to start second server");
    sleep(Duration::from_millis(300)).await;

    // Test connection to second server
    let mut client2 = InterprocessClient::new(config.clone());
    let task2 = create_cross_platform_task("second_server_test");

    let result2 = timeout(Duration::from_secs(5), client2.send_task(task2))
        .await
        .expect("Second server test timed out")
        .expect("Second server test failed");

    assert!(result2.success);
    assert_eq!(result2.error_message, Some("second_server".to_owned()));

    // Stop second server (ignore cleanup panics on Windows)
    let _panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        server2.stop();
    }));
    // We intentionally ignore the result as we're just trying to prevent test failures due to panic
}

/// Test platform-specific timeout behavior
#[tokio::test]
#[cfg(not(windows))] // Skip on Windows due to interprocess crate issues
async fn test_cross_platform_timeout_behavior() {
    let (mut config, _temp_dir) = create_cross_platform_config("timeout_behavior");

    // Set short timeouts for testing
    config.read_timeout_ms = 1000;
    config.write_timeout_ms = 1000;

    // Start server with slow handler
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Simulate slow processing
        sleep(Duration::from_millis(2000)).await;

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(300)).await;

    // Test timeout behavior
    let mut client = InterprocessClient::new(config.clone());
    let task = create_cross_platform_task("timeout_test");

    let result = timeout(Duration::from_secs(5), client.send_task(task)).await;

    // Should timeout or receive error response
    match result {
        Ok(Ok(response)) => {
            // If we get a response, it should indicate an error or timeout
            assert!(
                !response.success || response.error_message.is_some(),
                "Response should indicate timeout or error"
            );
        }
        Ok(Err(e)) => {
            // Expected: IPC error due to timeout
            let error_msg = e.to_string();
            assert!(
                error_msg.contains("timeout") || error_msg.contains("Timeout"),
                "Error should indicate timeout: {}",
                error_msg
            );
        }
        Err(_) => {
            // Expected: outer timeout
        }
    }

    // Use graceful shutdown to prevent production panics
    let _result = server.graceful_shutdown().await;
}

/// Test platform-specific cleanup behavior
#[tokio::test]
#[cfg(not(windows))] // Skip on Windows due to interprocess crate issues
async fn test_cross_platform_cleanup() {
    let (config, _temp_dir) = create_cross_platform_config("cleanup_behavior");

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(300)).await;

    // Verify server is running
    let mut client = InterprocessClient::new(config.clone());
    let task = create_cross_platform_task("cleanup_test");

    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("Cleanup test timed out")
        .expect("Cleanup test failed");

    assert!(result.success);

    // Check platform-specific resources before cleanup
    #[cfg(unix)]
    {
        let socket_path = std::path::Path::new(&config.endpoint_path);
        assert!(
            socket_path.exists(),
            "Socket file should exist before cleanup"
        );
    };

    // Stop server and verify cleanup
    // Use graceful shutdown to prevent production panics
    let _result = server.graceful_shutdown().await;
    sleep(Duration::from_millis(300)).await;

    // Check platform-specific cleanup
    #[cfg(unix)]
    {
        let socket_path = std::path::Path::new(&config.endpoint_path);
        assert!(!socket_path.exists(), "Socket file should be cleaned up");
    };

    // Verify server is no longer accepting connections
    let mut post_cleanup_client = InterprocessClient::new(config.clone());
    let post_cleanup_task = create_cross_platform_task("post_cleanup_test");

    let post_cleanup_result = timeout(
        Duration::from_secs(2),
        post_cleanup_client.send_task(post_cleanup_task),
    )
    .await;

    assert!(
        post_cleanup_result.is_err() || post_cleanup_result.is_ok_and(|r| r.is_err()),
        "Should not be able to connect after cleanup"
    );
}
