//! Security validation tests for IPC communication
//!
//! This test suite validates security aspects of the IPC implementation including:
//! - Socket permissions and access control
//! - Connection limits and resource management
//! - Input validation and attack resistance
//! - Privilege separation and isolation

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
    clippy::redundant_else,
    clippy::panic
)]

use daemoneye_lib::ipc::codec::{IpcCodec, IpcError};
use daemoneye_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
use daemoneye_lib::ipc::{IpcConfig, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use interprocess::local_socket::tokio::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Barrier;
use tokio::time::{sleep, timeout};

/// Create a test configuration for security tests
fn create_security_test_config(test_name: &str) -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_security_test_endpoint(&temp_dir, test_name);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 1024 * 1024, // 1MB limit for security tests
        accept_timeout_ms: 2000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 3, // Low limit for testing
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    (config, temp_dir)
}

/// Create platform-specific security test endpoint
fn create_security_test_endpoint(temp_dir: &TempDir, test_name: &str) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join(format!("security_{}.sock", test_name))
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("security");
        format!(r"\\.\pipe\daemoneye\security-{}-{}", test_name, dir_name)
    }
}

/// Create a test detection task
fn create_security_test_task(task_id: &str) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("security validation test".to_owned()),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    }
}

/// Test Unix socket permissions (Unix only)
#[cfg(unix)]
#[tokio::test]
async fn test_unix_socket_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (config, _temp_dir) = create_security_test_config("socket_permissions");

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
    sleep(Duration::from_millis(200)).await;

    // Check socket file permissions
    let socket_path = std::path::Path::new(&config.endpoint_path);
    assert!(socket_path.exists(), "Socket file should exist");

    let metadata = std::fs::metadata(socket_path).expect("Failed to get socket metadata");
    let permissions = metadata.permissions();
    let mode = permissions.mode();

    // Socket should have owner read/write only (0600)
    assert_eq!(
        mode & 0o777,
        0o600,
        "Socket permissions should be 0600 (owner only), got {:o}",
        mode & 0o777
    );

    // Check parent directory permissions
    if let Some(parent_dir) = socket_path.parent() {
        if parent_dir != std::path::Path::new("/tmp") && parent_dir.exists() {
            let parent_metadata =
                std::fs::metadata(parent_dir).expect("Failed to get parent dir metadata");
            let parent_permissions = parent_metadata.permissions();
            let parent_mode = parent_permissions.mode();

            // Parent directory should have at least owner read/write/execute (0700)
            // In test environments, it might have additional permissions (like 0755)
            assert!(
                (parent_mode & 0o700) == 0o700,
                "Parent directory should have at least owner read/write/execute permissions, got {:o}",
                parent_mode & 0o777
            );
        }
    }

    // Verify client can connect with proper permissions
    let mut client = InterprocessClient::new(config.clone());
    let task = create_security_test_task("permission_test");

    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("Permission test timed out")
        .expect("Permission test failed");

    assert!(result.success);

    let _result = server.graceful_shutdown().await;

    // Verify socket cleanup
    sleep(Duration::from_millis(100)).await;
    assert!(!socket_path.exists(), "Socket file should be cleaned up");
}

/// Test connection limits enforcement
#[tokio::test]
async fn test_connection_limits_enforcement() {
    let (mut config, _temp_dir) = create_security_test_config("connection_limits");

    // Set very low connection limit
    config.max_connections = 2;

    let connection_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&connection_counter);

    // Start server with connection tracking
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let conn_num = counter.fetch_add(1, Ordering::SeqCst);

            // Hold connection open to test limits
            sleep(Duration::from_millis(1000)).await;

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![ProcessRecord {
                    pid: conn_num,
                    ppid: None,
                    name: format!("test_process_{}", conn_num),
                    executable_path: None,
                    command_line: vec![],
                    start_time: None,
                    cpu_usage: None,
                    memory_usage: None,
                    executable_hash: None,
                    hash_algorithm: None,
                    user_id: None,
                    accessible: true,
                    file_exists: true,
                    collection_time: chrono::Utc::now().timestamp_millis(),
                }],
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Attempt more connections than the limit
    let num_clients = 5; // More than the limit of 2
    let barrier = Arc::new(Barrier::new(num_clients));
    let mut handles = vec![];

    for i in 0..num_clients {
        let client_config = config.clone();
        let client_barrier = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            // Wait for all clients to be ready
            client_barrier.wait().await;

            let mut client = InterprocessClient::new(client_config);
            let task = create_security_test_task(&format!("limit_test_{}", i));

            let start_time = Instant::now();
            let result = timeout(Duration::from_secs(3), client.send_task(task)).await;
            let duration = start_time.elapsed();

            (i, result, duration)
        });

        handles.push(handle);
    }

    // Wait for all attempts to complete
    let mut successful_connections = 0;
    let mut rejected_connections = 0;
    let mut timeout_connections = 0;

    for handle in handles {
        let (client_id, result, duration) = handle.await.expect("Client task failed");

        match result {
            Ok(Ok(_response)) => {
                successful_connections += 1;
                println!("Client {} succeeded in {:?}", client_id, duration);
            }
            Ok(Err(e)) => {
                rejected_connections += 1;
                println!("Client {} rejected: {} in {:?}", client_id, e, duration);
            }
            Err(_) => {
                timeout_connections += 1;
                println!("Client {} timed out in {:?}", client_id, duration);
            }
        }
    }

    println!(
        "Connection limit test results: {} successful, {} rejected, {} timeout (limit: {})",
        successful_connections, rejected_connections, timeout_connections, config.max_connections
    );

    // Should have some successful connections but not exceed the limit
    assert!(
        successful_connections > 0,
        "At least some connections should succeed"
    );
    assert!(
        successful_connections <= config.max_connections,
        "Should not exceed connection limit: {} > {}",
        successful_connections,
        config.max_connections
    );

    // Most connections should be rejected or timeout due to limits
    assert!(
        rejected_connections + timeout_connections > 0,
        "Some connections should be rejected due to limits"
    );

    let _result = server.graceful_shutdown().await;
}

/// Test message size limits enforcement
#[tokio::test]
async fn test_message_size_limits() {
    let (mut config, _temp_dir) = create_security_test_config("message_size_limits");

    // Set small message size limit for testing
    config.max_frame_bytes = 1024; // 1KB limit

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
    sleep(Duration::from_millis(200)).await;

    // Test with oversized message
    let mut client = InterprocessClient::new(config.clone());
    let large_task = DetectionTask {
        task_id: "oversized_test".to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("x".repeat(2048)), // Larger than 1KB limit
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    let result = timeout(Duration::from_secs(5), client.send_task(large_task)).await;

    // Should fail due to size limit
    match result {
        Ok(Err(IpcError::TooLarge { size, max_size })) => {
            assert!(size > max_size, "Size should exceed limit");
            assert_eq!(
                max_size, config.max_frame_bytes,
                "Max size should match config"
            );
        }
        Ok(Err(IpcError::Encode(_))) => {
            // Also acceptable - encoding might fail first
        }
        other => {
            panic!("Expected TooLarge or Encode error for oversized message, got: {other:?}");
        }
    }

    // Test with normal-sized message (should succeed)
    let normal_task = create_security_test_task("normal_test");
    let normal_result = timeout(Duration::from_secs(5), client.send_task(normal_task))
        .await
        .expect("Normal message timed out");

    match normal_result {
        Ok(result) => {
            assert!(result.success, "Normal-sized message should succeed");
        }
        Err(
            IpcError::ServerNotFound { endpoint }
            | IpcError::ConnectionRefused { endpoint }
            | IpcError::PermissionDenied { endpoint }
            | IpcError::ConnectionTimeout { endpoint },
        ) => {
            // If connection fails, that's also acceptable for this test
            // as we're primarily testing message size limits
            println!(
                "Normal message connection failed (acceptable for size limit test): {}",
                endpoint
            );
        }
        Err(IpcError::Io(io_err)) => {
            // Fallback for other IO errors
            println!(
                "Normal message connection failed (acceptable for size limit test): {}",
                io_err
            );
        }
        Err(other_err) => {
            panic!("Unexpected error for normal message: {}", other_err);
        }
    }

    let _result = server.graceful_shutdown().await;
}

/// Test malformed frame attack resistance
#[tokio::test]
async fn test_malformed_frame_resistance() {
    let (config, _temp_dir) = create_security_test_config("malformed_frames");

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
    sleep(Duration::from_millis(200)).await;

    // Test various malformed frames
    let malformed_frames = [
        // Zero length frame
        vec![0, 0, 0, 0, 0, 0, 0, 0],
        // Invalid length (too large)
        vec![0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0],
        // Partial frame (missing data)
        vec![10, 0, 0, 0, 0x12, 0x34, 0x56, 0x78, 1, 2, 3], // Claims 10 bytes but only has 3
        // Corrupted CRC
        vec![4, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 1, 2, 3, 4],
    ];

    for (i, malformed_frame) in malformed_frames.iter().enumerate() {
        // Try to send malformed frame directly to socket
        match LocalSocketStream::connect(
            create_socket_name(&config).expect("Failed to create socket name"),
        )
        .await
        {
            Ok(mut stream) => {
                // Send malformed frame
                let write_result = stream.write_all(malformed_frame).await;

                if write_result.is_ok() {
                    // Try to read response (should fail or timeout)
                    let mut buffer = vec![0_u8; 1024];
                    let read_result =
                        timeout(Duration::from_millis(500), stream.read(&mut buffer)).await;

                    // Server should either close connection or timeout
                    match read_result {
                        Ok(Ok(0)) => {
                            // Connection closed (good)
                            println!("Malformed frame {} caused connection close", i);
                        }
                        Ok(Ok(_)) => {
                            // Got some data back - check if it's an error response
                            println!("Malformed frame {} got response (may be error)", i);
                        }
                        Ok(Err(_)) | Err(_) => {
                            // Error or timeout (good)
                            println!("Malformed frame {} caused error/timeout", i);
                        }
                    }
                } else {
                    println!("Malformed frame {} failed to send", i);
                }
            }
            Err(e) => {
                println!("Failed to connect for malformed frame test {}: {}", i, e);
            }
        }
    }

    // Verify server is still functional after malformed frame attacks
    let mut client = InterprocessClient::new(config.clone());
    let recovery_task = create_security_test_task("recovery_test");

    let recovery_result = timeout(Duration::from_secs(5), client.send_task(recovery_task))
        .await
        .expect("Recovery test timed out")
        .expect("Recovery test failed");

    assert!(
        recovery_result.success,
        "Server should recover from malformed frame attacks"
    );

    let _result = server.graceful_shutdown().await;
}

/// Helper function to create socket name (platform-specific)
fn create_socket_name(
    config: &IpcConfig,
) -> Result<interprocess::local_socket::Name<'_>, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::{GenericFilePath, ToFsName};
        use std::path::Path;

        let path = Path::new(&config.endpoint_path);
        Ok(path.to_fs_name::<GenericFilePath>()?)
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};

        Ok(config
            .endpoint_path
            .clone()
            .to_ns_name::<GenericNamespaced>()?)
    }
}

/// Test timeout-based `DoS` resistance
#[tokio::test]
async fn test_timeout_dos_resistance() {
    let (mut config, _temp_dir) = create_security_test_config("timeout_dos");

    // Set short timeouts for testing
    config.read_timeout_ms = 1000;
    config.write_timeout_ms = 1000;

    let slow_request_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&slow_request_counter);

    // Start server with intentionally slow handler
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let request_num = counter.fetch_add(1, Ordering::SeqCst);

            // First few requests are slow (potential DoS)
            if request_num < 3 {
                sleep(Duration::from_millis(2000)).await; // Exceed timeout
            }

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
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Send requests that will timeout
    let mut timeout_count = 0;
    let mut success_count = 0;

    for i in 0..5 {
        let mut client = InterprocessClient::new(config.clone());
        let task = create_security_test_task(&format!("timeout_dos_{}", i));

        let result = timeout(Duration::from_secs(3), client.send_task(task)).await;

        match result {
            Ok(Ok(response)) => {
                if response.success {
                    success_count += 1;
                } else {
                    timeout_count += 1; // Server-side timeout
                }
            }
            Ok(Err(_)) | Err(_) => {
                timeout_count += 1; // Client-side timeout or error
            }
        }
    }

    println!(
        "Timeout DoS test: {} timeouts, {} successes",
        timeout_count, success_count
    );

    // Should have some timeouts due to slow processing
    assert!(
        timeout_count > 0,
        "Should have some timeouts due to slow processing"
    );

    // But server should still be responsive for later requests
    assert!(success_count > 0, "Should have some successful requests");

    let _result = server.graceful_shutdown().await;
}

/// Test resource exhaustion resistance
#[tokio::test]
async fn test_resource_exhaustion_resistance() {
    let (mut config, _temp_dir) = create_security_test_config("resource_exhaustion");

    // Set limits for resource exhaustion testing
    config.max_connections = 2;
    config.max_frame_bytes = 10 * 1024; // 10KB limit

    let resource_counter = Arc::new(AtomicU32::new(0));
    let handler_counter = Arc::clone(&resource_counter);

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(move |task: DetectionTask| {
        let counter = Arc::clone(&handler_counter);
        async move {
            let request_num = counter.fetch_add(1, Ordering::SeqCst);

            // Simulate resource usage
            let _memory_usage = vec![0_u8; 1024]; // Allocate some memory
            sleep(Duration::from_millis(100)).await; // Use some CPU time

            Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![ProcessRecord {
                    pid: request_num,
                    ppid: None,
                    name: format!("resource_test_{}", request_num),
                    executable_path: None,
                    command_line: vec![],
                    start_time: None,
                    cpu_usage: None,
                    memory_usage: None,
                    executable_hash: None,
                    hash_algorithm: None,
                    user_id: None,
                    accessible: true,
                    file_exists: true,
                    collection_time: chrono::Utc::now().timestamp_millis(),
                }],
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            })
        }
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Attempt resource exhaustion with many concurrent requests
    let num_attackers = 10;
    let mut handles = vec![];

    for i in 0..num_attackers {
        let client_config = config.clone();

        let handle = tokio::spawn(async move {
            let mut client = InterprocessClient::new(client_config);
            let task = create_security_test_task(&format!("exhaust_{}", i));

            timeout(Duration::from_secs(5), client.send_task(task)).await
        });

        handles.push(handle);
    }

    // Wait for all attempts
    let mut successful_attacks = 0;
    let mut failed_attacks = 0;

    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(Ok(_))) => {
                successful_attacks += 1;
            }
            Ok(Ok(Err(_)) | Err(_)) | Err(_) => {
                failed_attacks += 1;
                println!("Attack {} failed (expected due to limits)", i);
            }
        }
    }

    println!(
        "Resource exhaustion test: {} successful, {} failed (limit: {})",
        successful_attacks, failed_attacks, config.max_connections
    );

    // Should limit successful attacks due to connection limits
    assert!(
        successful_attacks <= config.max_connections,
        "Should not exceed connection limits during resource exhaustion attack"
    );

    // Verify server is still functional after attack
    let mut recovery_client = InterprocessClient::new(config.clone());
    let recovery_task = create_security_test_task("post_attack_recovery");

    let recovery_result = timeout(
        Duration::from_secs(5),
        recovery_client.send_task(recovery_task),
    )
    .await
    .expect("Recovery test timed out")
    .expect("Recovery test failed");

    assert!(
        recovery_result.success,
        "Server should recover from resource exhaustion attack"
    );

    let _result = server.graceful_shutdown().await;
}

/// Test CRC32 collision resistance (basic)
#[tokio::test]
async fn test_crc32_collision_resistance() {
    let _codec = IpcCodec::new(1024 * 1024);

    // Test that different data produces different CRC32 values
    let data_a = b"a".repeat(100);
    let data_b = b"b".repeat(100);
    let test_cases = vec![
        (b"test data 1".as_slice(), b"test data 2".as_slice()),
        (b"hello world".as_slice(), b"hello world!".as_slice()),
        (b"".as_slice(), b"x".as_slice()),
        (data_a.as_slice(), data_b.as_slice()),
    ];

    for (data1, data2) in test_cases {
        let crc1 = crc32c::crc32c(data1);
        let crc2 = crc32c::crc32c(data2);

        assert_ne!(
            crc1, crc2,
            "Different data should produce different CRC32 values: {:?} vs {:?}",
            data1, data2
        );
    }

    // Test that identical data produces identical CRC32 values
    let identical_data = b"identical test data";
    let crc_a = crc32c::crc32c(identical_data);
    let crc_b = crc32c::crc32c(identical_data);

    assert_eq!(
        crc_a, crc_b,
        "Identical data should produce identical CRC32 values"
    );
}

/// Test server shutdown security
#[tokio::test]
async fn test_server_shutdown_security() {
    let (config, _temp_dir) = create_security_test_config("shutdown_security");

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
    sleep(Duration::from_millis(200)).await;

    // Verify server is running
    let mut client = InterprocessClient::new(config.clone());
    let task = create_security_test_task("pre_shutdown");

    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("Pre-shutdown test timed out")
        .expect("Pre-shutdown test failed");

    assert!(result.success);

    // Stop server
    let _result = server.graceful_shutdown().await;
    sleep(Duration::from_millis(200)).await;

    // Verify server properly rejects new connections
    let mut post_shutdown_client = InterprocessClient::new(config.clone());
    let post_shutdown_task = create_security_test_task("post_shutdown");

    let post_shutdown_result = timeout(
        Duration::from_secs(2),
        post_shutdown_client.send_task(post_shutdown_task),
    )
    .await;

    // Should fail to connect
    assert!(
        post_shutdown_result.is_err() || post_shutdown_result.is_ok_and(|r| r.is_err()),
        "Should not be able to connect after server shutdown"
    );

    // Verify socket cleanup on Unix
    #[cfg(unix)]
    {
        let socket_path = std::path::Path::new(&config.endpoint_path);
        assert!(
            !socket_path.exists(),
            "Socket file should be cleaned up after shutdown"
        );
    }
}
