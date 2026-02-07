//! Chaos/Resilience Tests for procmond.
//!
//! These tests verify procmond's behavior under adverse conditions, including:
//! - Connection failures and recovery
//! - Backpressure handling
//! - Resource limits and constraints
//! - Concurrent operations
//!
//! # Test Categories
//!
//! 1. **Connection Failures**: Broker unavailability, reconnection, event replay
//! 2. **Backpressure**: Buffer fill, adaptive interval, WAL persistence
//! 3. **Resource Limits**: Memory constraints, CPU limits, WAL rotation
//! 4. **Concurrent Operations**: Multiple RPC requests, shutdown during collection

#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::print_stdout,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::shadow_reuse,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::collapsible_if,
    clippy::integer_division,
    clippy::map_unwrap_or,
    clippy::use_debug,
    clippy::equatable_if_let,
    clippy::needless_pass_by_value,
    clippy::semicolon_outside_block,
    clippy::cast_lossless,
    clippy::single_match_else,
    clippy::shadow_unrelated,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::clone_on_ref_ptr,
    clippy::single_match,
    clippy::pattern_type_mismatch,
    clippy::ignored_unit_patterns
)]

mod common;

use common::{
    create_health_check_request, create_isolated_connector, create_large_event,
    create_mock_health_data, create_test_actor_with_receiver, create_test_event,
};
use daemoneye_eventbus::rpc::RpcStatus;
use procmond::event_bus_connector::{
    BackpressureSignal, EventBusConnector, EventBusConnectorError, ProcessEventType,
};
use procmond::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorMessage};
use procmond::rpc_service::{RpcServiceConfig, RpcServiceHandler};
use procmond::wal::WriteAheadLog;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout};

// ============================================================================
// Test Helpers (unique to chaos tests)
// ============================================================================

/// Creates a test WAL with a small rotation threshold for testing.
async fn create_test_wal(rotation_threshold: u64) -> (WriteAheadLog, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal =
        WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), rotation_threshold)
            .await
            .expect("Failed to create WAL");
    (wal, temp_dir)
}

// ============================================================================
// SECTION 1: Connection Failures (Task 12)
// ============================================================================

/// Test that connector handles broker unavailability gracefully.
#[tokio::test]
async fn test_connection_failure_broker_unavailable() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Verify not connected initially
    assert!(!connector.is_connected());

    // Try to connect - should fail because no broker is running
    let result = connector.connect().await;

    // Should get either EnvNotSet or Connection error
    assert!(result.is_err());

    // Should still be able to publish (events go to buffer/WAL)
    let event = create_test_event(1);
    let result = connector.publish(event, ProcessEventType::Start).await;
    assert!(
        result.is_ok(),
        "Should buffer events when broker unavailable"
    );

    // Verify event is buffered
    assert_eq!(connector.buffered_event_count(), 1);
}

/// Test that events are written to WAL when broker is unavailable.
#[tokio::test]
async fn test_connection_failure_events_persisted_to_wal() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // First connector - publish events while disconnected
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // Publish multiple events
        for i in 1..=10 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Should succeed via WAL");
        }
    }

    // Second connector - verify events survived restart
    {
        let wal = WriteAheadLog::new(wal_path.clone())
            .await
            .expect("Failed to open WAL");

        let events = wal.replay().await.expect("Failed to replay WAL");

        assert_eq!(events.len(), 10, "All 10 events should be in WAL");

        // Verify event content
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.pid, (i + 1) as u32);
        }
    }
}

/// Test that connector attempts reconnection with backoff.
#[tokio::test]
async fn test_connection_failure_reconnection_backoff() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // First connection attempt - expected to fail in test context without broker
    drop(connector.connect().await);

    // Publish while disconnected - this should trigger reconnection attempts internally
    let start = std::time::Instant::now();

    for i in 1..=3 {
        let event = create_test_event(i);
        // We don't care about individual publish results here, just testing reconnection behavior
        drop(connector.publish(event, ProcessEventType::Start).await);
    }

    // Reconnection backoff should not block main operations significantly
    // Events should be processed quickly (buffered, not waiting for connection)
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "Publishing should not be blocked by reconnection attempts, took {:?}",
        elapsed
    );

    // Events should be buffered
    assert_eq!(connector.buffered_event_count(), 3);
}

/// Test socket unavailability handling during publish.
#[tokio::test]
async fn test_connection_failure_socket_unavailable() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Simulate being "connected" but socket becomes unavailable
    // Since we can't actually connect, we test the fallback behavior

    // Publish events - should go to WAL/buffer regardless of connection state
    for i in 1..=5 {
        let event = create_test_event(i);
        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(
            result.is_ok(),
            "Publish should succeed via WAL when socket unavailable"
        );
    }

    // All events should be buffered
    assert_eq!(connector.buffered_event_count(), 5);
    assert!(connector.buffer_size_bytes() > 0);
}

/// Test that events maintain sequence order across connection failures.
#[tokio::test]
async fn test_connection_failure_sequence_ordering() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish events with different types
    let seq1 = connector
        .publish(create_test_event(1), ProcessEventType::Start)
        .await
        .expect("Should succeed");

    let seq2 = connector
        .publish(create_test_event(2), ProcessEventType::Stop)
        .await
        .expect("Should succeed");

    let seq3 = connector
        .publish(create_test_event(3), ProcessEventType::Modify)
        .await
        .expect("Should succeed");

    // Verify sequences are monotonically increasing
    assert!(seq1 < seq2, "Sequence 1 < 2");
    assert!(seq2 < seq3, "Sequence 2 < 3");

    // Drop connector before opening WAL to avoid file lock conflicts
    drop(connector);

    // Verify in WAL
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let entries = wal.replay_entries().await.expect("Failed to replay");
    assert_eq!(entries.len(), 3);

    // Verify sequence order in WAL
    assert_eq!(entries[0].sequence, seq1);
    assert_eq!(entries[1].sequence, seq2);
    assert_eq!(entries[2].sequence, seq3);
}

// ============================================================================
// SECTION 2: Backpressure (Task 13)
// ============================================================================

/// Test that backpressure signal is activated when buffer fills.
#[tokio::test]
async fn test_backpressure_buffer_fill_triggers_activation() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;
    let mut bp_rx = connector
        .take_backpressure_receiver()
        .expect("Should have receiver");

    // Use very large events to fill buffer quickly
    // The buffer is 10MB with 70% high-water mark (7MB)

    let mut activated = false;
    let mut overflow = false;
    for i in 1..=2000 {
        // Create larger events to fill buffer faster
        let event = create_large_event(i, 200);
        match connector.publish(event, ProcessEventType::Start).await {
            Ok(_) => {
                // Check for backpressure signal
                if let Ok(Some(signal)) = timeout(Duration::from_millis(1), bp_rx.recv()).await {
                    if signal == BackpressureSignal::Activated {
                        activated = true;
                        println!(
                            "Backpressure activated at event {}, buffer {}%",
                            i,
                            connector.buffer_usage_percent()
                        );
                        break;
                    }
                }
            }
            Err(EventBusConnectorError::BufferOverflow) => {
                // Hit buffer limit before activation - this is also valid
                overflow = true;
                println!(
                    "Buffer overflow at event {}, buffer {}%",
                    i,
                    connector.buffer_usage_percent()
                );
                break;
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    // Should have hit either activation or overflow
    let usage = connector.buffer_usage_percent();
    assert!(
        activated || overflow || usage >= 70,
        "Should have activated backpressure, hit overflow, or reach 70%+ usage, got {}%",
        usage
    );
}

/// Test that adaptive interval adjustment works with backpressure.
#[tokio::test]
async fn test_backpressure_adaptive_interval_adjustment() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();

    let original_interval = Duration::from_secs(30);

    // Calculate expected new interval (1.5x) simulating backpressure activation
    let expected_interval = Duration::from_millis(
        original_interval
            .as_millis()
            .saturating_mul(3)
            .saturating_div(2) as u64,
    );

    // Manually trigger interval adjustment like backpressure monitor would
    actor_handle
        .adjust_interval(expected_interval)
        .expect("Should send adjustment");

    // Verify message received
    let msg = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Should receive message")
        .expect("Channel should not be closed");

    match msg {
        ActorMessage::AdjustInterval { new_interval } => {
            assert_eq!(new_interval, expected_interval);
            println!(
                "Interval adjusted from {:?} to {:?}",
                original_interval, new_interval
            );
        }
        _ => panic!("Expected AdjustInterval message, got {:?}", msg),
    }
}

/// Test that WAL persistence prevents data loss when buffer is full.
#[tokio::test]
async fn test_backpressure_wal_prevents_data_loss() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish events until buffer overflow
    let mut published_count = 0_u32;
    let mut last_successful_seq = 0_u64;
    for i in 1..=1000 {
        let event = create_large_event(i, 100);
        match connector.publish(event, ProcessEventType::Start).await {
            Ok(seq) => {
                published_count += 1;
                last_successful_seq = seq;
            }
            Err(EventBusConnectorError::BufferOverflow) => {
                println!(
                    "Buffer overflow at event {}, successfully published {} events (last seq: {})",
                    i, published_count, last_successful_seq
                );
                break;
            }
            Err(e) => panic!("Unexpected error at event {}: {:?}", i, e),
        }
    }

    // Drop connector to ensure WAL is flushed
    drop(connector);

    // Verify events are in WAL
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let events = wal.replay().await.expect("Failed to replay WAL");

    // WAL should contain at least as many events as we successfully published
    // (may have more if overflow event was partially written to WAL before buffer check)
    assert!(
        events.len() >= published_count as usize,
        "WAL should contain at least {} published events, got {}",
        published_count,
        events.len()
    );

    println!(
        "Verified {} events persisted to WAL (published: {})",
        events.len(),
        published_count
    );
}

/// Test that no backpressure signal is sent for small buffer usage.
#[tokio::test]
async fn test_backpressure_no_signal_for_low_buffer_usage() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;
    let mut bp_rx = connector
        .take_backpressure_receiver()
        .expect("Should have receiver");

    // The connector doesn't expose direct buffer manipulation, so we verify
    // the signal receiver works correctly by checking no signals pending initially
    assert!(
        bp_rx.try_recv().is_err(),
        "No signals should be pending initially"
    );

    // Publish a few events (not enough to trigger backpressure)
    for i in 1..=5 {
        let event = create_test_event(i);
        // We don't care about individual publish results here, just testing backpressure signals
        drop(connector.publish(event, ProcessEventType::Start).await);
    }

    // Verify no backpressure signals for small buffer usage
    assert!(
        bp_rx.try_recv().is_err(),
        "Should not signal for small buffer usage"
    );

    let usage = connector.buffer_usage_percent();
    println!("Buffer usage after 5 small events: {}%", usage);
    assert!(usage < 70, "Small events should not trigger backpressure");
}

// ============================================================================
// SECTION 3: Resource Limits (Task 14)
// ============================================================================

/// Test that operations complete within reasonable memory constraints.
/// This is a sanity check rather than enforcing exact 100MB limit in tests.
#[tokio::test]
async fn test_resource_limits_memory_budget() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // The connector has a 10MB buffer limit
    // Verify we can't exceed it
    let max_buffer_bytes = 10 * 1024 * 1024;

    let mut total_buffered = 0_usize;
    for i in 1..=5000 {
        let event = create_test_event(i);
        match connector.publish(event, ProcessEventType::Start).await {
            Ok(_) => {
                total_buffered = connector.buffer_size_bytes();
            }
            Err(EventBusConnectorError::BufferOverflow) => {
                // This is expected when buffer is full
                break;
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    // Buffer should not exceed max
    assert!(
        total_buffered <= max_buffer_bytes,
        "Buffer size {} should not exceed max {}",
        total_buffered,
        max_buffer_bytes
    );

    println!(
        "Buffer usage: {} bytes (max: {} bytes)",
        total_buffered, max_buffer_bytes
    );
}

/// Test WAL file rotation when threshold is reached.
#[tokio::test]
async fn test_resource_limits_wal_rotation() {
    // Use a small rotation threshold for testing (1KB)
    let rotation_threshold = 1024_u64;
    let (wal, temp_dir) = create_test_wal(rotation_threshold).await;

    // Write events until rotation should occur
    let mut written = 0_u32;
    for i in 1..=100 {
        let event = create_test_event(i);
        wal.write(event).await.expect("WAL write should succeed");
        written = i;
    }

    // Verify multiple WAL files were created
    let mut wal_files = Vec::new();
    for entry in std::fs::read_dir(temp_dir.path()).expect("Should read dir") {
        let entry = entry.expect("Should read entry");
        let filename = entry.file_name().to_string_lossy().to_string();
        if filename.ends_with(".wal") {
            wal_files.push(filename);
        }
    }

    println!(
        "Created {} WAL files after {} events",
        wal_files.len(),
        written
    );

    // Should have at least 2 files due to rotation
    assert!(
        wal_files.len() >= 2,
        "WAL should rotate when threshold {} bytes is reached",
        rotation_threshold
    );

    // Verify all events can still be replayed
    let events = wal.replay().await.expect("Replay should succeed");
    assert_eq!(events.len(), written as usize);
}

/// Test that WAL rotation prevents disk exhaustion by creating bounded files.
#[tokio::test]
async fn test_resource_limits_wal_bounded_file_size() {
    // Very small threshold to ensure multiple rotations
    let rotation_threshold = 512_u64;
    let (wal, temp_dir) = create_test_wal(rotation_threshold).await;

    // Write many events
    for i in 1..=50 {
        let event = create_test_event(i);
        wal.write(event).await.expect("WAL write should succeed");
    }

    // Check individual file sizes and count WAL files
    let mut wal_file_count = 0;
    // Allow generous overhead: 4x rotation threshold for last partial write
    let max_allowed_size = rotation_threshold * 4;

    for entry in std::fs::read_dir(temp_dir.path()).expect("Should read dir") {
        let entry = entry.expect("Should read entry");
        let metadata = entry.metadata().expect("Should read metadata");
        let filename = entry.file_name().to_string_lossy().to_string();

        if filename.ends_with(".wal") {
            wal_file_count += 1;
            let size = metadata.len();
            println!("WAL file {} size: {} bytes", filename, size);

            // Assert file size is bounded
            assert!(
                size <= max_allowed_size,
                "WAL file {} size {} exceeds max allowed {} (rotation_threshold * 4)",
                filename,
                size,
                max_allowed_size
            );
        }
    }

    // Should have created multiple WAL files due to rotation
    assert!(
        wal_file_count >= 2,
        "Expected at least 2 WAL files from rotation, got {}",
        wal_file_count
    );
}

/// Test CPU-bound operations complete in reasonable time.
#[tokio::test]
async fn test_resource_limits_operation_timing() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Measure time to publish 1000 events
    let start = std::time::Instant::now();

    for i in 1..=1000 {
        let event = create_test_event(i);
        if connector
            .publish(event, ProcessEventType::Start)
            .await
            .is_err()
        {
            // Buffer overflow is acceptable
            break;
        }
    }

    let elapsed = start.elapsed();

    // Should complete in reasonable time (not CPU-bound)
    // 1000 events should take less than 5 seconds even on slow systems
    assert!(
        elapsed < Duration::from_secs(5),
        "Publishing 1000 events took {:?}, should be < 5s",
        elapsed
    );

    println!("Published events in {:?}", elapsed);
}

// ============================================================================
// SECTION 4: Concurrent Operations (Task 15)
// ============================================================================

/// Test multiple concurrent RPC requests are handled correctly.
#[tokio::test]
async fn test_concurrent_multiple_rpc_requests() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let config = RpcServiceConfig {
        collector_id: "test-procmond".to_string(),
        control_topic: "control.collector.procmond".to_string(),
        response_topic_prefix: "response.collector.procmond".to_string(),
        default_timeout: Duration::from_millis(500),
        max_concurrent_requests: 10,
    };

    let handler = Arc::new(RpcServiceHandler::new(
        actor_handle.clone(),
        event_bus,
        config,
    ));

    // Spawn actor responder
    let response_count = Arc::new(AtomicU64::new(0));
    let response_count_clone = response_count.clone();

    let responder = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                ActorMessage::HealthCheck { respond_to } => {
                    respond_to
                        .send(create_mock_health_data())
                        .expect("Response receiver should be waiting");
                    response_count_clone.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    });

    // Send multiple concurrent requests
    let mut handles = Vec::new();
    for i in 0..10 {
        let handler_clone = handler.clone();
        let handle = tokio::spawn(async move {
            let request = create_health_check_request(5);
            let response = handler_clone.handle_request(request).await;
            (i, response.status)
        });
        handles.push(handle);
    }

    // Wait for all requests to complete
    let mut results = Vec::new();
    for handle in handles {
        let result = timeout(Duration::from_secs(2), handle)
            .await
            .expect("Request should complete")
            .expect("Task should not panic");
        results.push(result);
    }

    // All requests should succeed
    for (i, status) in &results {
        assert_eq!(*status, RpcStatus::Success, "Request {} should succeed", i);
    }

    println!("All {} concurrent requests succeeded", results.len());

    // Clean up
    responder.abort();
}

/// Test that config updates during collection are applied at cycle boundary.
#[tokio::test]
async fn test_concurrent_config_update_during_operation() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();

    // Spawn task to handle the actor message
    let responder = tokio::spawn(async move {
        if let Some(msg) = rx.recv().await {
            match msg {
                ActorMessage::UpdateConfig { respond_to, .. } => {
                    // Simulate validation and acceptance
                    respond_to
                        .send(Ok(()))
                        .expect("Config update response receiver should be waiting");
                    println!("Config update received and validated");
                }
                _ => panic!("Expected UpdateConfig message"),
            }
        }
    });

    // Send a config update using the public API
    let result = actor_handle
        .update_config(procmond::monitor_collector::ProcmondMonitorConfig::default())
        .await;

    assert!(result.is_ok(), "Config update should be accepted");

    // Clean up
    responder.abort();
}

/// Test graceful shutdown waits for current operation to complete.
#[tokio::test]
async fn test_concurrent_shutdown_during_operation() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();

    // Spawn task to handle the actor message
    let responder = tokio::spawn(async move {
        // Wait a bit to simulate "during operation"
        sleep(Duration::from_millis(50)).await;

        if let Some(msg) = rx.recv().await {
            match msg {
                ActorMessage::GracefulShutdown { respond_to } => {
                    // Simulate graceful completion
                    respond_to
                        .send(Ok(()))
                        .expect("Shutdown response receiver should be waiting");
                    println!("Graceful shutdown processed");
                }
                _ => panic!("Expected GracefulShutdown message"),
            }
        }
    });

    // Request graceful shutdown using the public API
    let result = actor_handle.graceful_shutdown().await;

    assert!(result.is_ok(), "Shutdown should complete gracefully");

    // Clean up
    responder.abort();
}

/// Test that BeginMonitoring transitions state correctly.
#[tokio::test]
async fn test_concurrent_begin_monitoring_state_transition() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();

    // Send BeginMonitoring
    actor_handle
        .begin_monitoring()
        .expect("Should send begin monitoring");

    // Verify message received
    let msg = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Should receive message")
        .expect("Channel open");

    match msg {
        ActorMessage::BeginMonitoring => {
            println!("BeginMonitoring received");
        }
        _ => panic!("Expected BeginMonitoring message"),
    }
}

/// Test multiple interval adjustments are handled correctly.
#[tokio::test]
async fn test_concurrent_interval_adjustments() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();

    // Send multiple rapid interval adjustments (simulating backpressure fluctuation)
    let intervals = vec![
        Duration::from_secs(30),
        Duration::from_secs(45),
        Duration::from_secs(60),
        Duration::from_secs(30), // Back to original
    ];

    for interval in &intervals {
        actor_handle
            .adjust_interval(*interval)
            .expect("Should send adjustment");
    }

    // Verify all messages received in order
    let mut received_intervals = Vec::new();
    for _ in &intervals {
        let msg = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("Should receive message")
            .expect("Channel open");

        if let ActorMessage::AdjustInterval { new_interval } = msg {
            received_intervals.push(new_interval);
        }
    }

    assert_eq!(
        received_intervals, intervals,
        "All interval adjustments should be received in order"
    );

    println!(
        "Processed {} interval adjustments in correct order",
        intervals.len()
    );
}

/// Test that channel backpressure on actor channel is handled.
#[tokio::test]
async fn test_concurrent_actor_channel_backpressure() {
    let (actor_handle, _rx) = create_test_actor_with_receiver();
    // Note: _rx is not consumed, so channel will fill up

    // Try to fill the channel (capacity is ACTOR_CHANNEL_CAPACITY = 100)
    let mut sent = 0_u32;
    let mut failed = 0_u32;

    for _ in 0..150 {
        match actor_handle.begin_monitoring() {
            Ok(_) => sent += 1,
            Err(_) => failed += 1,
        }
    }

    println!(
        "Sent {} messages, {} failed due to full channel",
        sent, failed
    );

    // Channel should have hit capacity
    assert!(failed > 0, "Should have some failures when channel is full");
    assert_eq!(
        sent, ACTOR_CHANNEL_CAPACITY as u32,
        "Should have sent exactly channel capacity messages"
    );
}

/// Test RPC handler correctly tracks statistics under concurrent load.
#[tokio::test]
async fn test_concurrent_rpc_stats_tracking() {
    let (actor_handle, mut rx) = create_test_actor_with_receiver();
    let (connector, _temp_dir) = create_isolated_connector().await;
    let event_bus = Arc::new(RwLock::new(connector));

    let handler = RpcServiceHandler::with_defaults(actor_handle, event_bus);

    // Spawn responder
    let responder = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let ActorMessage::HealthCheck { respond_to } = msg {
                respond_to
                    .send(create_mock_health_data())
                    .expect("Response receiver should be waiting");
            }
        }
    });

    // Send multiple requests
    for _ in 0..5 {
        let request = create_health_check_request(5);
        // We don't care about response, just testing stats tracking
        drop(handler.handle_request(request).await);
    }

    // Check stats
    let stats = handler.stats().await;
    assert_eq!(
        stats.requests_received, 5,
        "Should track 5 received requests"
    );
    assert!(
        stats.health_checks >= 5,
        "Should track at least 5 health checks"
    );

    println!("Stats: {:?}", stats);

    responder.abort();
}

// ============================================================================
// Integration Tests (Multiple Categories)
// ============================================================================

/// Integration test: Connection failure + backpressure + WAL persistence.
#[tokio::test]
async fn test_integration_connection_failure_with_wal_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Phase 1: Publish events while disconnected
    let events_written;
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // Attempt connection - expected to fail in test context without broker
        drop(connector.connect().await);

        // Publish events
        for i in 1..=20 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Should buffer");
        }

        events_written = connector.buffered_event_count();
        println!("Phase 1: Buffered {} events", events_written);

        connector.shutdown().await.expect("Shutdown should succeed");
    }

    // Phase 2: Verify WAL persistence across restart
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL");

        let events = wal.replay().await.expect("Replay should succeed");
        assert_eq!(events.len(), 20, "All 20 events should be in WAL");

        println!("Phase 2: Recovered {} events from WAL", events.len());
    }
}

/// Integration test: Concurrent operations with backpressure.
#[tokio::test]
async fn test_integration_concurrent_operations_with_backpressure() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;
    let mut bp_rx = connector
        .take_backpressure_receiver()
        .expect("Should have receiver");

    // Spawn concurrent publishers
    let connector = Arc::new(tokio::sync::Mutex::new(connector));
    let mut handles = Vec::new();

    for batch in 0..5 {
        let connector_clone = connector.clone();
        let handle = tokio::spawn(async move {
            let mut published = 0;
            for i in 1..=20 {
                let event = create_test_event(batch * 100 + i);
                let mut conn = connector_clone.lock().await;
                if conn.publish(event, ProcessEventType::Start).await.is_ok() {
                    published += 1;
                }
            }
            published
        });
        handles.push(handle);
    }

    // Monitor backpressure while publishing
    let bp_task = tokio::spawn(async move {
        let mut signals = Vec::new();
        while let Ok(Some(signal)) = timeout(Duration::from_millis(100), bp_rx.recv()).await {
            signals.push(signal);
        }
        signals
    });

    // Wait for publishers
    let mut total_published = 0;
    for handle in handles {
        total_published += handle.await.expect("Task should complete");
    }

    // Get backpressure signals
    let signals = bp_task.await.expect("BP task should complete");

    println!(
        "Published {} events total, received {} backpressure signals",
        total_published,
        signals.len()
    );

    // Verify some events were published
    assert!(total_published > 0, "Should have published some events");
}
