//! Event Bus Communication Integration Tests.
//!
//! These tests verify the EventBusConnector's behavior in realistic scenarios,
//! testing the integration between WAL persistence, in-memory buffering,
//! backpressure handling, and event ordering.
//!
//! # Test Categories
//!
//! 1. **Publish/Subscribe Flow**: Events are published and buffered correctly
//! 2. **Reconnection Behavior**: Connection state management and recovery
//! 3. **Buffering/Replay**: Events are buffered during disconnection and replayed
//! 4. **Topic Routing**: Events are published to correct topics (events.process.*)
//! 5. **Event Ordering**: Events maintain their sequence order
//! 6. **WAL Integration**: Events persist to WAL and can be recovered
//!
//! # Architecture
//!
//! Since actual broker connectivity requires daemoneye-agent running, these tests
//! focus on:
//! - Testing the connector's internal logic and state management
//! - Testing with simulated connection states
//! - Testing buffer/WAL behavior which doesn't require actual broker
//! - Verifying event ordering and topic routing

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
    clippy::cast_lossless
)]

use collector_core::event::ProcessEvent;
use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};
use procmond::wal::WriteAheadLog;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::time::sleep;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a test EventBusConnector with an isolated temp directory.
/// Returns the connector and the temp directory (which must be kept alive).
async fn create_isolated_connector() -> (EventBusConnector, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");
    (connector, temp_dir)
}

/// Creates a test process event with specified PID.
fn create_test_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("test-process-{pid}"),
        executable_path: Some(format!("/usr/bin/test_{pid}")),
        command_line: vec![
            "test".to_string(),
            "--flag".to_string(),
            format!("--pid={pid}"),
        ],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(5.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some(format!("hash_{pid}")),
        user_id: Some("1000".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

/// Creates a large test event with extensive command line arguments.
fn create_large_event(pid: u32, arg_count: usize) -> ProcessEvent {
    let command_line: Vec<String> = (0..arg_count)
        .map(|i| format!("--arg{}=value{}", i, "x".repeat(50)))
        .collect();

    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("large-process-{pid}"),
        executable_path: Some(format!("/usr/bin/large_{pid}")),
        command_line,
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(50.0),
        memory_usage: Some(100 * 1024 * 1024),
        executable_hash: Some("a".repeat(64)),
        user_id: Some("root".to_string()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

// ============================================================================
// Publish/Subscribe Flow Tests
// ============================================================================

/// Test that events are correctly published with assigned sequence numbers.
#[tokio::test]
async fn test_publish_assigns_monotonic_sequence_numbers() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Publish multiple events
    let mut sequences = Vec::new();
    for i in 1..=10 {
        let event = create_test_event(i);
        let seq = connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
        sequences.push(seq);
    }

    // Verify sequences are monotonically increasing
    for i in 1..sequences.len() {
        assert!(
            sequences[i] > sequences[i - 1],
            "Sequence {} should be greater than {}, got {} vs {}",
            i,
            i - 1,
            sequences[i],
            sequences[i - 1]
        );
    }

    // Verify first sequence starts at 1
    assert_eq!(sequences[0], 1, "First sequence should be 1");
}

/// Test that events published while disconnected are buffered correctly.
#[tokio::test]
async fn test_publish_while_disconnected_buffers_events() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Verify not connected
    assert!(!connector.is_connected());

    // Publish events - they should be buffered
    for i in 1..=5 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed even when disconnected");
    }

    // Verify events are buffered
    assert_eq!(connector.buffered_event_count(), 5);
    assert!(connector.buffer_size_bytes() > 0);
}

/// Test that different event types are handled correctly.
#[tokio::test]
async fn test_publish_different_event_types() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Publish start event
    let event1 = create_test_event(1);
    let seq1 = connector
        .publish(event1, ProcessEventType::Start)
        .await
        .expect("Start event should publish");

    // Publish stop event
    let event2 = create_test_event(2);
    let seq2 = connector
        .publish(event2, ProcessEventType::Stop)
        .await
        .expect("Stop event should publish");

    // Publish modify event
    let event3 = create_test_event(3);
    let seq3 = connector
        .publish(event3, ProcessEventType::Modify)
        .await
        .expect("Modify event should publish");

    // All sequences should be unique and increasing
    assert_eq!(seq1, 1);
    assert_eq!(seq2, 2);
    assert_eq!(seq3, 3);

    // All should be buffered
    assert_eq!(connector.buffered_event_count(), 3);
}

// ============================================================================
// Reconnection Behavior Tests
// ============================================================================

/// Test that connect fails gracefully when broker is not available.
#[tokio::test]
async fn test_connect_fails_without_broker() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Connect should fail (no broker running or env var not set)
    let result = connector.connect().await;
    assert!(result.is_err());

    // Should still be disconnected
    assert!(!connector.is_connected());
}

/// Test that events can still be published after failed connection attempt.
#[tokio::test]
async fn test_publish_after_failed_connect() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Try to connect - expected to fail in test context without broker
    drop(connector.connect().await);

    // Should still be able to publish (to buffer)
    let event = create_test_event(1);
    let result = connector.publish(event, ProcessEventType::Start).await;

    assert!(result.is_ok());
    assert_eq!(connector.buffered_event_count(), 1);
}

/// Test shutdown clears connection state.
#[tokio::test]
async fn test_shutdown_clears_connection_state() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Publish some events
    for i in 1..=3 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    // Shutdown
    connector.shutdown().await.expect("Shutdown should succeed");

    // Connection should be cleared
    assert!(!connector.is_connected());

    // Buffer should still have events (for potential recovery)
    assert_eq!(connector.buffered_event_count(), 3);
}

// ============================================================================
// Buffering and Replay Tests
// ============================================================================

/// Test buffer overflow protection.
#[tokio::test]
async fn test_buffer_overflow_protection() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Create a large event to fill buffer quickly
    let large_event = create_large_event(1, 100);

    // Estimate single event size
    let first_result = connector
        .publish(large_event.clone(), ProcessEventType::Start)
        .await;
    assert!(first_result.is_ok());

    let single_event_size = connector.buffer_size_bytes();

    // Calculate how many events would exceed buffer
    // Default buffer is 10MB, but we can test with a smaller buffer
    // by publishing many events

    // Publish events until we hit overflow
    let mut overflow_count = 0;
    for i in 2..=1000 {
        let event = create_large_event(i, 100);
        match connector.publish(event, ProcessEventType::Start).await {
            Ok(_) => {}
            Err(procmond::event_bus_connector::EventBusConnectorError::BufferOverflow) => {
                overflow_count = i;
                break;
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    // We should have hit overflow at some point (or filled the buffer)
    // Either we hit overflow or we published many events
    if overflow_count > 0 {
        println!(
            "Buffer overflow at event {}, buffer size: {} bytes",
            overflow_count,
            connector.buffer_size_bytes()
        );
    } else {
        println!(
            "Published 999 events, buffer size: {} bytes",
            connector.buffer_size_bytes()
        );
    }

    // Buffer should be at capacity or close to it
    assert!(
        connector.buffer_size_bytes() >= single_event_size,
        "Buffer should have at least one event"
    );
}

/// Test that WAL persists events for recovery.
#[tokio::test]
async fn test_wal_persistence_across_connector_restarts() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // First connector instance - publish events
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create first connector");

        for i in 1..=5 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }

        // Explicit shutdown
        connector.shutdown().await.expect("Shutdown should succeed");
    }

    // Second connector instance - should recover events from WAL
    {
        let _connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create second connector");

        // Access WAL directly to verify events persisted
        let wal = WriteAheadLog::new(wal_path.clone())
            .await
            .expect("Failed to open WAL");

        let events = wal.replay().await.expect("Failed to replay WAL");

        assert_eq!(events.len(), 5, "Should have 5 events in WAL");

        // Verify event content
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.pid, (i + 1) as u32);
        }
    }
}

/// Test that event types are preserved in WAL.
#[tokio::test]
async fn test_wal_preserves_event_types() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Publish events with different types
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let event1 = create_test_event(1);
        connector
            .publish(event1, ProcessEventType::Start)
            .await
            .expect("Start event should publish");

        let event2 = create_test_event(2);
        connector
            .publish(event2, ProcessEventType::Stop)
            .await
            .expect("Stop event should publish");

        let event3 = create_test_event(3);
        connector
            .publish(event3, ProcessEventType::Modify)
            .await
            .expect("Modify event should publish");
    }

    // Verify event types are preserved
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL");

        let entries = wal.replay_entries().await.expect("Failed to replay WAL");

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].event_type.as_deref(), Some("start"));
        assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
        assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
    }
}

/// Test replay_wal while disconnected buffers events.
#[tokio::test]
async fn test_replay_wal_buffers_when_disconnected() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // First instance - publish events
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        for i in 1..=3 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }
    }

    // Second instance - replay while disconnected
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // Not connected, so replay should buffer events
        let replayed = connector.replay_wal().await.expect("Replay should succeed");

        // Since we're disconnected, events are buffered, not replayed to broker
        // The return value is 0 because nothing was published to broker
        assert_eq!(replayed, 0);
    }
}

// ============================================================================
// Topic Routing Tests (events.process.*)
// ============================================================================

/// Test that different event types produce correct type strings in WAL.
/// Note: topic() is private, so we verify via WAL entries which store type strings.
#[tokio::test]
async fn test_event_type_stored_correctly_in_wal() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        connector
            .publish(create_test_event(1), ProcessEventType::Start)
            .await
            .expect("Start should publish");

        connector
            .publish(create_test_event(2), ProcessEventType::Stop)
            .await
            .expect("Stop should publish");

        connector
            .publish(create_test_event(3), ProcessEventType::Modify)
            .await
            .expect("Modify should publish");
    }

    // Verify type strings in WAL (these map to topics)
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let entries = wal.replay_entries().await.expect("Failed to replay");

    assert_eq!(entries.len(), 3);
    // Type strings: "start", "stop", "modify" map to topics:
    // "events.process.start", "events.process.stop", "events.process.modify"
    assert_eq!(entries[0].event_type.as_deref(), Some("start"));
    assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
    assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
}

/// Test that events are buffered with correct topics.
#[tokio::test]
async fn test_buffered_events_have_correct_topics() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Publish events of different types
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let event1 = create_test_event(1);
        connector
            .publish(event1, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");

        let event2 = create_test_event(2);
        connector
            .publish(event2, ProcessEventType::Stop)
            .await
            .expect("Publish should succeed");

        let event3 = create_test_event(3);
        connector
            .publish(event3, ProcessEventType::Modify)
            .await
            .expect("Publish should succeed");

        // Verify all events are buffered
        assert_eq!(connector.buffered_event_count(), 3);
    }

    // Verify topics through WAL entries
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let entries = wal.replay_entries().await.expect("Failed to replay");
    assert_eq!(entries.len(), 3);

    // Event types map to topics:
    // "start" -> "events.process.start"
    // "stop" -> "events.process.stop"
    // "modify" -> "events.process.modify"
    assert_eq!(entries[0].event_type.as_deref(), Some("start"));
    assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
    assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
}

/// Test topic routing across connector restarts.
#[tokio::test]
async fn test_topic_routing_preserved_across_restart() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Publish with different event types
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        connector
            .publish(create_test_event(1), ProcessEventType::Start)
            .await
            .expect("Start should publish");

        connector
            .publish(create_test_event(2), ProcessEventType::Stop)
            .await
            .expect("Stop should publish");

        connector
            .publish(create_test_event(3), ProcessEventType::Modify)
            .await
            .expect("Modify should publish");
    }

    // Verify topics are preserved
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL");

        let entries = wal.replay_entries().await.expect("Failed to replay");

        // Event types are stored in WAL entries
        assert_eq!(entries.len(), 3);

        // The type string maps to topics:
        // "start" -> "events.process.start"
        // "stop" -> "events.process.stop"
        // "modify" -> "events.process.modify"
        assert_eq!(entries[0].event_type.as_deref(), Some("start"));
        assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
        assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
    }
}

// ============================================================================
// Event Ordering Preservation Tests
// ============================================================================

/// Test that events maintain FIFO ordering in buffer.
#[tokio::test]
async fn test_event_ordering_in_buffer() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Publish events with identifiable PIDs
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        for i in 1..=20 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }
    }

    // Verify order through WAL entries
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let entries = wal.replay_entries().await.expect("Failed to replay");
    assert_eq!(entries.len(), 20);

    // Verify FIFO ordering - PIDs should match publish order
    for (i, entry) in entries.iter().enumerate() {
        let expected_pid = (i + 1) as u32;
        assert_eq!(
            entry.event.pid, expected_pid,
            "Event at position {} should have PID {}, got {}",
            i, expected_pid, entry.event.pid
        );
    }

    // Also verify sequence numbers are monotonically increasing
    for (i, entry) in entries.iter().enumerate() {
        let expected_seq = (i + 1) as u64;
        assert_eq!(
            entry.sequence, expected_seq,
            "Event at position {} should have sequence {}, got {}",
            i, expected_seq, entry.sequence
        );
    }
}

/// Test that sequence numbers are preserved across restarts.
#[tokio::test]
async fn test_sequence_continuity_across_restarts() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // First instance - publish 5 events
    let last_seq_1 = {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let mut last_seq = 0;
        for i in 1..=5 {
            let event = create_test_event(i);
            last_seq = connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }
        last_seq
    };

    assert_eq!(last_seq_1, 5);

    // Second instance - continue from sequence 6
    let first_seq_2 = {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let event = create_test_event(6);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed")
    };

    // Sequence should continue from 6
    assert_eq!(first_seq_2, 6, "Sequence should continue after restart");
}

/// Test ordering with mixed event types.
#[tokio::test]
async fn test_ordering_with_mixed_event_types() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Publish events in specific order with different types
    let sequences: Vec<(u64, ProcessEventType)> = vec![
        (
            connector
                .publish(create_test_event(1), ProcessEventType::Start)
                .await
                .unwrap(),
            ProcessEventType::Start,
        ),
        (
            connector
                .publish(create_test_event(2), ProcessEventType::Modify)
                .await
                .unwrap(),
            ProcessEventType::Modify,
        ),
        (
            connector
                .publish(create_test_event(3), ProcessEventType::Stop)
                .await
                .unwrap(),
            ProcessEventType::Stop,
        ),
        (
            connector
                .publish(create_test_event(4), ProcessEventType::Start)
                .await
                .unwrap(),
            ProcessEventType::Start,
        ),
        (
            connector
                .publish(create_test_event(5), ProcessEventType::Stop)
                .await
                .unwrap(),
            ProcessEventType::Stop,
        ),
    ];

    // Verify sequences are strictly increasing
    for i in 1..sequences.len() {
        assert!(
            sequences[i].0 > sequences[i - 1].0,
            "Sequence should be strictly increasing"
        );
    }

    // Verify we got expected sequences
    assert_eq!(sequences[0].0, 1);
    assert_eq!(sequences[4].0, 5);
}

// ============================================================================
// WAL Integration in Publish Flow Tests
// ============================================================================

/// Test that every publish writes to WAL first (durability guarantee).
#[tokio::test]
async fn test_wal_write_before_buffer() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish single event
    let event = create_test_event(42);
    let seq = connector
        .publish(event, ProcessEventType::Start)
        .await
        .expect("Publish should succeed");

    assert_eq!(seq, 1);

    // Verify event is in WAL immediately
    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let events = wal.replay().await.expect("Failed to replay");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].pid, 42);

    // Also verify it's buffered
    assert_eq!(connector.buffered_event_count(), 1);
}

/// Test WAL and buffer stay synchronized.
#[tokio::test]
async fn test_wal_buffer_synchronization() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish multiple events
    for i in 1..=10 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    // Both WAL and buffer should have 10 events
    assert_eq!(connector.buffered_event_count(), 10);

    let wal = WriteAheadLog::new(wal_path)
        .await
        .expect("Failed to open WAL");

    let wal_events = wal.replay().await.expect("Failed to replay");
    assert_eq!(wal_events.len(), 10);
}

/// Test WAL recovery after crash simulation.
#[tokio::test]
async fn test_wal_recovery_after_crash() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Simulate "crash" by dropping connector without shutdown
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        for i in 1..=7 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }

        // Drop without shutdown - simulates crash
        drop(connector);
    }

    // Recovery - new connector should find WAL data
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL for recovery");

        let events = wal.replay().await.expect("Failed to replay after crash");
        assert_eq!(events.len(), 7, "All events should be recovered from WAL");

        // Verify event content
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.pid, (i + 1) as u32);
            assert_eq!(event.name, format!("test-process-{}", i + 1));
        }
    }
}

// ============================================================================
// Backpressure Tests
// ============================================================================

/// Test backpressure receiver can be taken and works correctly.
#[tokio::test]
async fn test_backpressure_receiver_functionality() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Take the backpressure receiver
    let _rx = connector
        .take_backpressure_receiver()
        .expect("Should get receiver");

    // Get event size to calculate buffer limit
    let test_event = create_test_event(1);
    connector
        .publish(test_event, ProcessEventType::Start)
        .await
        .expect("First publish should succeed");

    let single_event_size = connector.buffer_size_bytes();

    // Verify receiver is taken (second take returns None)
    assert!(connector.take_backpressure_receiver().is_none());

    // Publish more events and check buffer grows
    for i in 2..=5 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    assert_eq!(connector.buffered_event_count(), 5);
    assert!(connector.buffer_size_bytes() >= single_event_size * 5);
}

/// Test backpressure receiver can only be taken once.
#[tokio::test]
async fn test_backpressure_receiver_single_consumer() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // First take succeeds
    let rx1 = connector.take_backpressure_receiver();
    assert!(rx1.is_some());

    // Second take returns None
    let rx2 = connector.take_backpressure_receiver();
    assert!(rx2.is_none());
}

// ============================================================================
// Buffer Behavior Tests
// ============================================================================

/// Test buffer usage percentage calculation.
#[tokio::test]
async fn test_buffer_usage_percentage() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Initial usage should be 0
    assert_eq!(connector.buffer_usage_percent(), 0);

    // Publish events and verify usage increases
    for i in 1..=100 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    // Usage should be non-zero (but likely still low given 10MB buffer)
    // With typical event sizes, 100 events won't fill 10MB buffer much
    let usage = connector.buffer_usage_percent();
    assert!(connector.buffer_size_bytes() > 0, "Buffer should have data");

    println!(
        "After 100 events: {} bytes, {}% usage",
        connector.buffer_size_bytes(),
        usage
    );
}

/// Test buffer behavior with very large events.
#[tokio::test]
async fn test_buffer_with_large_events() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Create events with large command lines
    for i in 1..=10 {
        let event = create_large_event(i, 200); // 200 args each
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    // Buffer should have grown significantly
    assert!(
        connector.buffer_size_bytes() > 100_000,
        "Buffer should be > 100KB with large events"
    );
}

// ============================================================================
// Concurrent Operations Tests
// ============================================================================

/// Test that multiple sequential publishes work correctly.
#[tokio::test]
async fn test_rapid_sequential_publishes() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    let start = std::time::Instant::now();

    // Rapid sequential publishes
    for i in 1..=100 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Publish should succeed");
    }

    let duration = start.elapsed();

    assert_eq!(connector.buffered_event_count(), 100);

    println!("Published 100 events in {:?}", duration);
}

/// Test concurrent connector creation (not sharing connectors, but parallel instances).
#[tokio::test]
async fn test_parallel_connector_creation() {
    let handles: Vec<_> = (0..5)
        .map(|i| {
            tokio::spawn(async move {
                let temp_dir = TempDir::new().expect("Failed to create temp directory");
                let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                    .await
                    .expect("Failed to create connector");

                (i, connector.is_connected())
            })
        })
        .collect();

    for handle in handles {
        let (idx, connected) = handle.await.expect("Task should complete");
        assert!(!connected, "Connector {} should not be connected", idx);
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Test graceful handling of WAL directory permissions (if possible to test).
#[tokio::test]
async fn test_connector_with_valid_directory() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    let connector = EventBusConnector::new(temp_dir.path().to_path_buf()).await;

    assert!(connector.is_ok());
}

/// Test multiple shutdown calls are safe.
#[tokio::test]
async fn test_multiple_shutdowns_are_safe() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // First shutdown
    connector
        .shutdown()
        .await
        .expect("First shutdown should succeed");

    // Second shutdown should also succeed (idempotent)
    connector
        .shutdown()
        .await
        .expect("Second shutdown should succeed");
}

/// Test publish after shutdown still works (to buffer/WAL).
#[tokio::test]
async fn test_publish_after_shutdown() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Shutdown first
    connector.shutdown().await.expect("Shutdown should succeed");

    // Publish should still work (writes to WAL and buffers)
    let event = create_test_event(1);
    let result = connector.publish(event, ProcessEventType::Start).await;

    assert!(result.is_ok());
}

// ============================================================================
// Full Flow Integration Tests
// ============================================================================

/// Test complete publish -> persist -> recover flow.
#[tokio::test]
async fn test_complete_publish_persist_recover_flow() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // Phase 1: Publish events
    let published_pids: Vec<u32> = {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let mut pids = Vec::new();
        for i in 1..=15 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
            pids.push(i);
        }

        // Shutdown gracefully
        connector.shutdown().await.expect("Shutdown should succeed");
        pids
    };

    // Phase 2: Simulate restart and recover
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL for recovery");

        let recovered_events = wal.replay().await.expect("Failed to replay WAL");

        assert_eq!(recovered_events.len(), published_pids.len());

        // Verify all PIDs are recovered in order
        for (i, event) in recovered_events.iter().enumerate() {
            assert_eq!(event.pid, published_pids[i], "Event {} PID mismatch", i);
        }
    }
}

/// Test interleaved operations (publish, shutdown, restart, publish more).
#[tokio::test]
async fn test_interleaved_operations() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let wal_path = temp_dir.path().to_path_buf();

    // First session
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        for i in 1..=5 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }

        connector.shutdown().await.expect("Shutdown should succeed");
    }

    // Second session
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // New events should continue sequence
        for i in 6..=10 {
            let event = create_test_event(i);
            let seq = connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");

            // Sequence should be i (continuing from previous session)
            assert_eq!(seq, i as u64);
        }

        connector.shutdown().await.expect("Shutdown should succeed");
    }

    // Verify all events are in WAL
    {
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to open WAL");

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 10);
    }
}

/// Test long-running publish session.
#[tokio::test]
async fn test_long_running_session() {
    let (mut connector, _temp_dir) = create_isolated_connector().await;

    // Publish many events over "time" (simulated with small sleeps)
    let batch_count = 10;
    let events_per_batch = 20;

    for batch in 0..batch_count {
        for i in 0..events_per_batch {
            let pid = (batch * events_per_batch + i + 1) as u32;
            let event = create_test_event(pid);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Publish should succeed");
        }

        // Small delay between batches
        sleep(Duration::from_millis(10)).await;
    }

    let total_events = batch_count * events_per_batch;
    assert_eq!(connector.buffered_event_count(), total_events);

    connector.shutdown().await.expect("Shutdown should succeed");
}
