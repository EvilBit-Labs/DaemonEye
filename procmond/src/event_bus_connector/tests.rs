//! Unit tests for the event bus connector.

use super::*;
use crate::wal::WalError;
use std::time::SystemTime;
use tempfile::TempDir;

/// Create a test process event with specified PID.
fn create_test_event(pid: u32) -> ProcessEvent {
    ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("test_process_{pid}"),
        executable_path: Some(format!("/usr/bin/test_{pid}")),
        command_line: vec!["test".to_owned(), "--arg".to_owned()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(5.0),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("abc123".to_owned()),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    }
}

#[tokio::test]
async fn test_connector_creation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    assert!(!connector.is_connected());
    assert_eq!(connector.buffer_usage_percent(), 0);
    assert_eq!(connector.buffered_event_count(), 0);
}

#[tokio::test]
async fn test_connect_fails_when_env_not_set() {
    // This test verifies behavior when DAEMONEYE_BROKER_SOCKET is not set.
    // We check by looking up the env var - if it's not set, we expect EnvNotSet.
    // If it IS set (e.g., in CI), we expect a different error (Connection).

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let result = connector.connect().await;

    // Connect should fail either because env var is not set or because
    // there's no broker listening
    assert!(result.is_err());

    match result.unwrap_err() {
        EventBusConnectorError::EnvNotSet(var) => {
            // Expected when env var is not set
            assert!(var.contains(BROKER_SOCKET_ENV));
        }
        EventBusConnectorError::Connection(_) => {
            // Expected when env var IS set but no broker is running
            // This is also a valid test outcome
        }
        other => panic!("Expected EnvNotSet or Connection error, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_publish_while_disconnected() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let event = create_test_event(1234);
    let result = connector.publish(event, ProcessEventType::Start).await;

    // Should succeed by writing to WAL and buffering
    assert!(result.is_ok());
    let sequence = result.unwrap();
    assert_eq!(sequence, 1);

    // Event should be buffered
    assert_eq!(connector.buffered_event_count(), 1);
    // Buffer size should be non-zero (percentage may round to 0 for small events
    // relative to 10MB max buffer)
    assert!(connector.buffer_size_bytes() > 0);
}

#[tokio::test]
async fn test_buffer_overflow_protection() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Set a very small buffer for testing
    connector.max_buffer_size = 500;

    // First event should succeed
    let event1 = create_test_event(1);
    let result1 = connector.publish(event1, ProcessEventType::Start).await;
    assert!(result1.is_ok());

    // Keep adding events until overflow
    let mut overflow_occurred = false;
    for i in 2..=100 {
        let event = create_test_event(i);
        if let Err(EventBusConnectorError::BufferOverflow) =
            connector.publish(event, ProcessEventType::Start).await
        {
            overflow_occurred = true;
            break;
        }
    }

    assert!(overflow_occurred, "Buffer overflow should have occurred");
}

#[tokio::test]
async fn test_backpressure_receiver() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // First call should succeed
    let rx = connector.take_backpressure_receiver();
    assert!(rx.is_some());

    // Second call should return None
    let rx2 = connector.take_backpressure_receiver();
    assert!(rx2.is_none());
}

#[tokio::test]
async fn test_process_event_type_topics() {
    assert_eq!(ProcessEventType::Start.topic(), "events.process.start");
    assert_eq!(ProcessEventType::Stop.topic(), "events.process.stop");
    assert_eq!(ProcessEventType::Modify.topic(), "events.process.modify");
}

#[tokio::test]
async fn test_buffered_event_size_estimation() {
    let event = create_test_event(1234);
    let topic = "events.process.start".to_owned();
    let buffered = BufferedEvent::new(1, event, topic);

    // Size should be reasonable (not zero, not huge)
    assert!(buffered.size_bytes > 50);
    assert!(buffered.size_bytes < 10000);
}

#[tokio::test]
async fn test_buffer_usage_calculation() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    assert_eq!(connector.buffer_usage_percent(), 0);

    // Set small buffer for predictable testing
    connector.max_buffer_size = 1000;

    // Add event to buffer directly for testing
    let event = create_test_event(1);
    let buffered = BufferedEvent::new(1, event, "test".to_owned());
    let event_size = buffered.size_bytes;
    connector.buffer.push_back(buffered);
    connector.buffer_size_bytes = event_size;

    let usage = connector.buffer_usage_percent();
    // Usage should be event_size * 100 / 1000
    let expected = (event_size * 100 / 1000).min(100);
    assert_eq!(usage, expected as u8);
}

#[tokio::test]
async fn test_shutdown_while_disconnected() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Should succeed even when not connected
    let result = connector.shutdown().await;
    assert!(result.is_ok());
    assert!(!connector.is_connected());
}

#[tokio::test]
async fn test_event_conversion() {
    let event = create_test_event(1234);
    let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event, 42);

    assert_eq!(eventbus_event.pid, 1234);
    assert_eq!(eventbus_event.name, "test_process_1234");
    assert_eq!(eventbus_event.ppid, Some(1));
    assert!(eventbus_event.executable_path.is_some());
}

#[tokio::test]
async fn test_wal_persistence_across_connector_instances() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_path = temp_dir.path().to_path_buf();

    // First instance - write some events
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        for i in 1..=5 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Failed to publish");
        }
    } // Connector dropped

    // Second instance - should be able to replay events
    {
        let connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // WAL should have events from first instance
        let events = connector.wal.replay().await.expect("Failed to replay WAL");
        assert_eq!(events.len(), 5);
    }
}

// ============================================================
// Additional tests for comprehensive coverage
// ============================================================

// --- ProcessEventType tests ---

#[test]
fn test_process_event_type_to_type_string() {
    assert_eq!(ProcessEventType::Start.to_type_string(), "start");
    assert_eq!(ProcessEventType::Stop.to_type_string(), "stop");
    assert_eq!(ProcessEventType::Modify.to_type_string(), "modify");
}

#[test]
fn test_process_event_type_from_type_string() {
    assert_eq!(
        ProcessEventType::from_type_string("start").unwrap(),
        ProcessEventType::Start
    );
    assert_eq!(
        ProcessEventType::from_type_string("stop").unwrap(),
        ProcessEventType::Stop
    );
    assert_eq!(
        ProcessEventType::from_type_string("modify").unwrap(),
        ProcessEventType::Modify
    );
}

#[test]
fn test_process_event_type_from_type_string_unknown() {
    // Unknown strings must return an error — never a silent Start default.
    assert!(matches!(
        ProcessEventType::from_type_string("unknown"),
        Err(EventBusConnectorError::UnknownEventType(s)) if s == "unknown"
    ));
    assert!(matches!(
        ProcessEventType::from_type_string(""),
        Err(EventBusConnectorError::UnknownEventType(s)) if s.is_empty()
    ));
    assert!(matches!(
        ProcessEventType::from_type_string("START"),
        Err(EventBusConnectorError::UnknownEventType(s)) if s == "START"
    ));
}

#[test]
fn test_process_event_type_debug_clone_copy() {
    let event_type = ProcessEventType::Start;
    let cloned = event_type;
    assert_eq!(event_type, cloned);
    assert_eq!(format!("{:?}", event_type), "Start");
}

#[test]
fn test_backpressure_signal_debug_clone_copy() {
    let signal = BackpressureSignal::Activated;
    let cloned = signal;
    assert_eq!(signal, cloned);
    assert_eq!(format!("{:?}", signal), "Activated");

    let signal2 = BackpressureSignal::Released;
    assert_eq!(format!("{:?}", signal2), "Released");
}

// --- BufferedEvent tests ---

#[test]
fn test_buffered_event_with_minimal_event() {
    let event = ProcessEvent {
        pid: 1,
        ppid: None,
        name: "min".to_owned(),
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
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let buffered = BufferedEvent::new(1, event, "test".to_owned());

    // Size should still include base overhead
    assert!(buffered.size_bytes >= 64);
    assert_eq!(buffered.sequence, 1);
    assert_eq!(buffered.topic, "test");
}

#[test]
fn test_buffered_event_with_platform_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "key1".to_owned(),
        serde_json::Value::String("value1".to_owned()),
    );
    metadata.insert("key2".to_owned(), serde_json::Value::Number(42.into()));

    let event = ProcessEvent {
        pid: 1,
        ppid: Some(1),
        name: "test".to_owned(),
        executable_path: Some("/bin/test".to_owned()),
        command_line: vec!["test".to_owned()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024),
        executable_hash: Some("hash".to_owned()),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: Some(serde_json::Value::Object(metadata)),
    };

    let buffered = BufferedEvent::new(1, event, "test".to_owned());

    // Size should include metadata string length
    assert!(buffered.size_bytes > 100);
}

#[test]
fn test_buffered_event_debug() {
    let event = create_test_event(123);
    let buffered = BufferedEvent::new(5, event, "topic".to_owned());

    let debug_str = format!("{:?}", buffered);
    assert!(debug_str.contains("BufferedEvent"));
    assert!(debug_str.contains("sequence: 5"));
}

// --- Buffer management tests ---

#[tokio::test]
async fn test_buffer_usage_percent_with_zero_max() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Set zero max buffer (edge case)
    connector.max_buffer_size = 0;

    // Should return 100% when max is zero
    assert_eq!(connector.buffer_usage_percent(), 100);
}

#[tokio::test]
async fn test_buffer_usage_percent_exact_100() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Set buffer size equal to max
    connector.max_buffer_size = 1000;
    connector.buffer_size_bytes = 1000;

    assert_eq!(connector.buffer_usage_percent(), 100);
}

#[tokio::test]
async fn test_buffer_usage_percent_over_100_clamped() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Set buffer size greater than max (shouldn't happen normally, but test clamping)
    connector.max_buffer_size = 1000;
    connector.buffer_size_bytes = 2000;

    // Should be clamped to 100
    assert_eq!(connector.buffer_usage_percent(), 100);
}

// --- Backpressure tests ---

#[tokio::test]
async fn test_backpressure_activation_at_70_percent() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut rx = connector.take_backpressure_receiver().unwrap();

    // Set small buffer for predictable percentages
    connector.max_buffer_size = 1000;

    // Simulate crossing 70% threshold
    connector.check_backpressure(69, 70);

    // Should receive activation signal
    let signal = rx.try_recv();
    assert!(signal.is_ok());
    assert_eq!(signal.unwrap(), BackpressureSignal::Activated);
}

#[tokio::test]
async fn test_backpressure_release_at_50_percent() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut rx = connector.take_backpressure_receiver().unwrap();

    // Simulate crossing below 50% threshold
    connector.check_backpressure(50, 49);

    // Should receive release signal
    let signal = rx.try_recv();
    assert!(signal.is_ok());
    assert_eq!(signal.unwrap(), BackpressureSignal::Released);
}

#[tokio::test]
async fn test_backpressure_no_signal_when_not_crossing_threshold() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut rx = connector.take_backpressure_receiver().unwrap();

    // Stay below 70% - no activation
    connector.check_backpressure(60, 65);
    assert!(rx.try_recv().is_err());

    // Stay above 50% - no release
    connector.check_backpressure(55, 60);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn test_backpressure_signal_with_dropped_receiver() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Take and immediately drop the receiver
    let rx = connector.take_backpressure_receiver().unwrap();
    drop(rx);

    // Should not panic when trying to send signal with dropped receiver
    connector.check_backpressure(69, 70);
    connector.check_backpressure(50, 49);
}

#[tokio::test]
async fn test_backpressure_integration_with_publish() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut rx = connector.take_backpressure_receiver().unwrap();

    // Calculate event size to set appropriate buffer limit
    let test_event = create_test_event(1);
    let event_size = BufferedEvent::estimate_size(&test_event, "events.process.start");

    // Set buffer such that 2 events = ~70% (so 3 events crosses threshold)
    // If 70% = 2 events, then 100% = 2/0.7 = ~2.86 events
    // So max_buffer_size = event_size * 3 should mean 2 events = 66%, 3 events = 100%
    // For 2 events to be exactly 70%, max = 2 * event_size / 0.7 = 2.86 * event_size
    let max_buffer = (event_size * 100) / 70 * 2 + 1; // About 2.86 events
    connector.max_buffer_size = max_buffer;

    // Publish events until we cross 70%
    let mut activation_received = false;
    for i in 1..=10 {
        let event = create_test_event(i);
        let result = connector.publish(event, ProcessEventType::Start).await;

        if result.is_err() {
            break; // Buffer overflow
        }

        // Check for backpressure signal
        if let Ok(signal) = rx.try_recv() {
            if signal == BackpressureSignal::Activated {
                activation_received = true;
                break;
            }
        }
    }

    // Should have received activation signal or exceeded threshold
    let usage = connector.buffer_usage_percent();
    assert!(
        activation_received || usage >= 70,
        "Expected activation signal or usage >= 70%, got: activation={}, usage={}%",
        activation_received,
        usage
    );
}

// --- Publish with different event types ---

#[tokio::test]
async fn test_publish_stop_event() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let event = create_test_event(1);
    let result = connector.publish(event, ProcessEventType::Stop).await;

    assert!(result.is_ok());
    assert_eq!(connector.buffered_event_count(), 1);

    // Check that the buffered event has the correct topic
    let buffered = &connector.buffer[0];
    assert_eq!(buffered.topic, "events.process.stop");
}

#[tokio::test]
async fn test_publish_modify_event() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let event = create_test_event(1);
    let result = connector.publish(event, ProcessEventType::Modify).await;

    assert!(result.is_ok());
    assert_eq!(connector.buffered_event_count(), 1);

    // Check that the buffered event has the correct topic
    let buffered = &connector.buffer[0];
    assert_eq!(buffered.topic, "events.process.modify");
}

// --- Sequence numbering tests ---

#[tokio::test]
async fn test_publish_sequence_numbering() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    for i in 1..=5 {
        let event = create_test_event(i);
        let seq = connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");

        assert_eq!(seq, u64::from(i));
    }
}

// --- add_to_buffer tests ---

#[tokio::test]
async fn test_add_to_buffer_tracks_size() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    connector.max_buffer_size = 10000;

    let event1 = create_test_event(1);
    let buffered1 = BufferedEvent::new(1, event1, "test".to_owned());
    let size1 = buffered1.size_bytes;

    connector.add_to_buffer(buffered1).expect("Should succeed");

    assert_eq!(connector.buffer_size_bytes, size1);
    assert_eq!(connector.buffered_event_count(), 1);

    let event2 = create_test_event(2);
    let buffered2 = BufferedEvent::new(2, event2, "test".to_owned());
    let size2 = buffered2.size_bytes;

    connector.add_to_buffer(buffered2).expect("Should succeed");

    assert_eq!(connector.buffer_size_bytes, size1 + size2);
    assert_eq!(connector.buffered_event_count(), 2);
}

#[tokio::test]
async fn test_add_to_buffer_rejects_when_full() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Create event and measure its size
    let event = create_test_event(1);
    let size = BufferedEvent::estimate_size(&event, "test");

    // Set max to slightly less than one event
    connector.max_buffer_size = size - 1;

    let buffered = BufferedEvent::new(1, event, "test".to_owned());
    let result = connector.add_to_buffer(buffered);

    assert!(matches!(
        result,
        Err(EventBusConnectorError::BufferOverflow)
    ));
}

// --- try_reconnect tests ---

#[tokio::test]
async fn test_try_reconnect_without_socket_config() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Without calling connect(), there's no socket_config
    let result = connector.try_reconnect().await;

    // Should return Ok(false) when no socket config
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[tokio::test]
async fn test_try_reconnect_backoff_skips_early_attempts() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Manually set up reconnection state
    connector.socket_config = Some(SocketConfig {
        unix_path: "/nonexistent/socket".to_owned(),
        windows_pipe: DEFAULT_WINDOWS_PIPE.to_owned(),
        connection_limit: 1,
        #[cfg(target_os = "freebsd")]
        freebsd_path: None,
        auth_token: None,
        per_client_byte_limit: MAX_BUFFER_SIZE,
        rate_limit_config: None,
        correlation_config: None,
    });
    connector.last_reconnect_attempt = Some(std::time::Instant::now());
    connector.reconnect_attempts = 1;

    // Immediate retry should be skipped due to backoff
    let result = connector.try_reconnect().await;

    // Should return Ok(false) because backoff period hasn't elapsed
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[tokio::test]
async fn test_try_reconnect_increments_attempts() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Set up socket config with invalid path
    connector.socket_config = Some(SocketConfig {
        unix_path: "/nonexistent/socket/path/that/wont/work".to_owned(),
        windows_pipe: DEFAULT_WINDOWS_PIPE.to_owned(),
        connection_limit: 1,
        #[cfg(target_os = "freebsd")]
        freebsd_path: None,
        auth_token: None,
        per_client_byte_limit: MAX_BUFFER_SIZE,
        rate_limit_config: None,
        correlation_config: None,
    });

    assert_eq!(connector.reconnect_attempts, 0);

    // First attempt should increment counter
    let _ = connector.try_reconnect().await;

    assert_eq!(connector.reconnect_attempts, 1);
    assert!(connector.last_reconnect_attempt.is_some());
}

// --- replay_wal tests ---

#[tokio::test]
async fn test_replay_wal_empty() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Replay on empty WAL should return 0
    let result = connector.replay_wal().await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[tokio::test]
async fn test_replay_wal_with_events_while_disconnected() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Write some events while disconnected (they go to buffer)
    for i in 1..=3 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");
    }

    // Replay WAL while still disconnected - events should be buffered
    let replayed = connector.replay_wal().await.expect("Failed to replay");

    // Since we're disconnected, events should be buffered, not replayed to broker
    // The replay count might be 0 (nothing published) + buffer flush (0 since disconnected)
    assert_eq!(replayed, 0);
}

#[tokio::test]
async fn test_replay_wal_preserves_event_types() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_path = temp_dir.path().to_path_buf();

    // First instance - write events with different types
    {
        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let event1 = create_test_event(1);
        connector
            .publish(event1, ProcessEventType::Start)
            .await
            .expect("Failed to publish");

        let event2 = create_test_event(2);
        connector
            .publish(event2, ProcessEventType::Stop)
            .await
            .expect("Failed to publish");

        let event3 = create_test_event(3);
        connector
            .publish(event3, ProcessEventType::Modify)
            .await
            .expect("Failed to publish");
    }

    // Second instance - verify event types are preserved in WAL
    {
        let connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        let entries = connector
            .wal
            .replay_entries()
            .await
            .expect("Failed to replay");
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].event_type.as_deref(), Some("start"));
        assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
        assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
    }
}

// --- flush_buffer tests ---

#[tokio::test]
async fn test_flush_buffer_when_empty() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Pretend we're connected
    connector.connected = true;

    // Flush empty buffer should return 0
    let flushed = connector.flush_buffer().await;
    assert_eq!(flushed, 0);
}

#[tokio::test]
async fn test_flush_buffer_when_disconnected() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Add events to buffer
    let event = create_test_event(1);
    let buffered = BufferedEvent::new(1, event, "test".to_owned());
    connector.buffer.push_back(buffered);
    connector.buffer_size_bytes = 100;

    // Flush while disconnected should return 0
    let flushed = connector.flush_buffer().await;
    assert_eq!(flushed, 0);
    assert_eq!(connector.buffered_event_count(), 1); // Event still in buffer
}

// --- Error type tests ---

#[test]
fn test_error_display() {
    let wal_err = EventBusConnectorError::Wal(WalError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "test",
    )));
    assert!(format!("{}", wal_err).contains("WAL error"));

    let eventbus_err = EventBusConnectorError::EventBus("test error".to_owned());
    assert!(format!("{}", eventbus_err).contains("EventBus error"));

    let conn_err = EventBusConnectorError::Connection("test conn".to_owned());
    assert!(format!("{}", conn_err).contains("Connection failed"));

    let overflow_err = EventBusConnectorError::BufferOverflow;
    assert!(format!("{}", overflow_err).contains("Buffer overflow"));

    let env_err = EventBusConnectorError::EnvNotSet("TEST_VAR".to_owned());
    assert!(format!("{}", env_err).contains("Environment variable not set"));

    let ser_err = EventBusConnectorError::Serialization("test ser".to_owned());
    assert!(format!("{}", ser_err).contains("Serialization error"));
}

#[test]
fn test_error_debug() {
    let err = EventBusConnectorError::BufferOverflow;
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("BufferOverflow"));
}

// --- Event conversion tests ---

#[test]
fn test_convert_to_eventbus_event_with_all_fields() {
    let event = ProcessEvent {
        pid: 1234,
        ppid: Some(1),
        name: "full_test".to_owned(),
        executable_path: Some("/usr/bin/full".to_owned()),
        command_line: vec!["full".to_owned(), "--opt1".to_owned(), "--opt2".to_owned()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(50.5),
        memory_usage: Some(1024 * 1024 * 100),
        executable_hash: Some("sha256:abc".to_owned()),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("root".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event, 42);

    assert_eq!(eventbus_event.pid, 1234);
    assert_eq!(eventbus_event.name, "full_test");
    assert_eq!(eventbus_event.ppid, Some(1));
    assert_eq!(
        eventbus_event.executable_path,
        Some("/usr/bin/full".to_owned())
    );
    // Command line is joined with spaces
    assert_eq!(
        eventbus_event.command_line,
        Some("full --opt1 --opt2".to_owned())
    );
    assert!(eventbus_event.start_time.is_some());
    // The durable WAL sequence is stamped into metadata as source_seq (T3 · U6).
    assert_eq!(daemoneye_eventbus::source_seq_of(&eventbus_event), 42);
}

#[test]
fn test_convert_to_eventbus_event_minimal() {
    let event = ProcessEvent {
        pid: 1,
        ppid: None,
        name: "min".to_owned(),
        executable_path: None,
        command_line: vec![],
        start_time: None,
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        hash_algorithm: None,
        user_id: None,
        accessible: false,
        file_exists: false,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event, 42);

    assert_eq!(eventbus_event.pid, 1);
    assert_eq!(eventbus_event.name, "min");
    assert_eq!(eventbus_event.ppid, None);
    assert_eq!(eventbus_event.executable_path, None);
    assert_eq!(eventbus_event.command_line, Some("".to_owned()));
    assert_eq!(eventbus_event.start_time, None);
}

// --- Concurrent operations tests ---

#[tokio::test]
async fn test_multiple_publishes_preserve_order() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Publish multiple events
    for i in 1..=10 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");
    }

    // Verify order is preserved in buffer
    for (idx, buffered) in connector.buffer.iter().enumerate() {
        let expected_pid = (idx + 1) as u32;
        assert_eq!(buffered.event.pid, expected_pid);
        assert_eq!(buffered.sequence, (idx + 1) as u64);
    }
}

// --- Large event handling tests ---

#[tokio::test]
async fn test_large_command_line_event() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Create event with very long command line
    let long_args: Vec<String> = (0..1000).map(|i| format!("--arg{i}=value{i}")).collect();
    let event = ProcessEvent {
        pid: 1,
        ppid: Some(1),
        name: "large_cmd".to_owned(),
        executable_path: Some("/usr/bin/large".to_owned()),
        command_line: long_args,
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024),
        executable_hash: Some("hash".to_owned()),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let result = connector.publish(event, ProcessEventType::Start).await;
    assert!(result.is_ok());

    // Size should reflect the large command line
    assert!(connector.buffer_size_bytes > 10000);
}

#[tokio::test]
async fn test_event_near_buffer_limit() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Create a normal event and measure its size
    let test_event = create_test_event(1);
    let event_size = BufferedEvent::estimate_size(&test_event, "events.process.start");

    // Set buffer to exactly 2 events + 1 byte margin
    connector.max_buffer_size = event_size * 2 + 1;

    // First two events should succeed
    let event1 = create_test_event(1);
    assert!(
        connector
            .publish(event1, ProcessEventType::Start)
            .await
            .is_ok()
    );

    let event2 = create_test_event(2);
    assert!(
        connector
            .publish(event2, ProcessEventType::Start)
            .await
            .is_ok()
    );

    // Third event should overflow
    let event3 = create_test_event(3);
    let result = connector.publish(event3, ProcessEventType::Start).await;
    assert!(matches!(
        result,
        Err(EventBusConnectorError::BufferOverflow)
    ));
}

// --- Connection state tests ---

#[tokio::test]
async fn test_is_connected_initial_state() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    assert!(!connector.is_connected());
}

#[tokio::test]
async fn test_buffer_size_bytes_initial_state() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    assert_eq!(connector.buffer_size_bytes(), 0);
}

// --- Shutdown tests ---

#[tokio::test]
async fn test_shutdown_clears_connected_flag() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Manually set connected to true
    connector.connected = true;

    connector.shutdown().await.expect("Shutdown failed");

    assert!(!connector.is_connected());
}

#[tokio::test]
async fn test_shutdown_with_buffered_events() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Publish some events (they will be buffered since disconnected)
    for i in 1..=5 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");
    }

    assert_eq!(connector.buffered_event_count(), 5);

    // Shutdown should succeed with events in buffer
    connector.shutdown().await.expect("Shutdown failed");

    // Buffer is preserved for potential recovery
    assert_eq!(connector.buffered_event_count(), 5);
}

// --- Size estimation accuracy tests ---

#[test]
fn test_estimate_size_consistency() {
    let event = create_test_event(123);
    let topic = "events.process.start";

    // Multiple calls should return the same size
    let size1 = BufferedEvent::estimate_size(&event, topic);
    let size2 = BufferedEvent::estimate_size(&event, topic);
    let size3 = BufferedEvent::estimate_size(&event, topic);

    assert_eq!(size1, size2);
    assert_eq!(size2, size3);
}

#[test]
fn test_estimate_size_increases_with_content() {
    let small_event = ProcessEvent {
        pid: 1,
        ppid: None,
        name: "a".to_owned(),
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
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let large_event = ProcessEvent {
        pid: 1,
        ppid: Some(1),
        name: "a".repeat(100),
        executable_path: Some("b".repeat(200)),
        command_line: vec!["c".repeat(50); 10],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.0),
        memory_usage: Some(1024),
        executable_hash: Some("d".repeat(64)),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("e".repeat(32)),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    };

    let small_size = BufferedEvent::estimate_size(&small_event, "t");
    let large_size = BufferedEvent::estimate_size(&large_event, "t");

    assert!(large_size > small_size);
}

// --- WAL integration tests ---

#[tokio::test]
async fn test_wal_write_before_buffer() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish an event
    let event = create_test_event(42);
    let seq = connector
        .publish(event, ProcessEventType::Start)
        .await
        .expect("Failed to publish");

    assert_eq!(seq, 1);

    // Verify event is in WAL (even though also in buffer)
    let wal_events = connector.wal.replay().await.expect("Failed to replay WAL");
    assert_eq!(wal_events.len(), 1);
    assert_eq!(wal_events[0].pid, 42);

    // Also verify it's in buffer
    assert_eq!(connector.buffered_event_count(), 1);
    assert_eq!(connector.buffer[0].event.pid, 42);
}

// --- WAL-write-before-buffer ordering test ---

#[tokio::test]
async fn test_wal_write_before_buffer_ordering() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_path = temp_dir.path().to_path_buf();

    let mut connector = EventBusConnector::new(wal_path.clone())
        .await
        .expect("Failed to create connector");

    // Publish multiple events while disconnected
    for i in 1..=3 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");
    }

    // Verify WAL contains all events (written first, before buffering)
    let wal_entries = connector
        .wal
        .replay_entries()
        .await
        .expect("Failed to replay WAL");
    assert_eq!(wal_entries.len(), 3, "WAL should have all events");

    // Verify buffer also has the same events
    assert_eq!(connector.buffered_event_count(), 3);

    // Verify WAL sequences match buffer sequences (same ordering)
    for (wal_entry, buffered) in wal_entries.iter().zip(connector.buffer.iter()) {
        assert_eq!(wal_entry.sequence, buffered.sequence);
        assert_eq!(wal_entry.event.pid, buffered.event.pid);
    }
}

// --- Buffer overflow graceful handling test ---

#[tokio::test]
async fn test_buffer_overflow_returns_error_gracefully() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    // Calculate event size to set precise buffer limit
    let sample_event = create_test_event(1);
    let event_size = BufferedEvent::estimate_size(&sample_event, "events.process.start");

    // Allow exactly 2 events, reject the third
    connector.max_buffer_size = event_size * 2;

    // First two events should succeed
    let event1 = create_test_event(1);
    assert!(
        connector
            .publish(event1, ProcessEventType::Start)
            .await
            .is_ok()
    );
    let event2 = create_test_event(2);
    assert!(
        connector
            .publish(event2, ProcessEventType::Start)
            .await
            .is_ok()
    );

    // Third event should fail with BufferOverflow
    let event3 = create_test_event(3);
    let result = connector.publish(event3, ProcessEventType::Start).await;
    assert!(
        matches!(result, Err(EventBusConnectorError::BufferOverflow)),
        "Expected BufferOverflow, got: {result:?}"
    );

    // Verify WAL still has all 3 events (WAL write happens before buffer)
    let wal_events = connector.wal.replay().await.expect("Failed to replay WAL");
    assert_eq!(
        wal_events.len(),
        3,
        "WAL should persist event even when buffer overflows"
    );

    // Buffer should only have the first 2 events
    assert_eq!(connector.buffered_event_count(), 2);
}

// --- Backpressure trigger and release combined test ---

#[tokio::test]
async fn test_backpressure_trigger_and_release_cycle() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let mut rx = connector.take_backpressure_receiver().unwrap();

    // Calculate event size for precise thresholds
    let sample_event = create_test_event(1);
    let event_size = BufferedEvent::estimate_size(&sample_event, "events.process.start");

    // Set buffer so that:
    //   3 events = 60% (below threshold)
    //   4 events = 80% (above 70% high-water mark)
    //   2 events = 40% (below 50% low-water mark)
    // max = 5 * event_size => each event = 20%
    connector.max_buffer_size = event_size * 5;

    // Fill to 60% (3 events) - no backpressure signal
    for i in 1..=3 {
        let event = create_test_event(i);
        connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");
    }
    assert!(rx.try_recv().is_err(), "No signal expected below 70%");

    // Add 4th event to cross 70% threshold (usage = 80%)
    let event4 = create_test_event(4);
    connector
        .publish(event4, ProcessEventType::Start)
        .await
        .expect("Failed to publish");

    // Should receive Activated signal
    let signal = rx
        .try_recv()
        .expect("Expected backpressure Activated signal");
    assert_eq!(signal, BackpressureSignal::Activated);

    // Drain buffer below 50% by removing events manually
    // Remove 3 events: from 4 events (80%) down to 1 event (20%) which is below 50%
    let previous_usage = connector.buffer_usage_percent();
    for _ in 0..3 {
        if let Some(removed) = connector.buffer.pop_front() {
            connector.buffer_size_bytes = connector
                .buffer_size_bytes
                .saturating_sub(removed.size_bytes);
        }
    }
    let current_usage = connector.buffer_usage_percent();

    // Manually trigger backpressure check for the drain
    connector.check_backpressure(previous_usage, current_usage);

    // Should receive Released signal
    let released_signal = rx
        .try_recv()
        .expect("Expected backpressure Released signal");
    assert_eq!(released_signal, BackpressureSignal::Released);
}

// --- Client ID generation test ---

#[tokio::test]
async fn test_client_id_is_unique() {
    let temp_dir1 = TempDir::new().expect("Failed to create temp dir");
    let temp_dir2 = TempDir::new().expect("Failed to create temp dir");

    let connector1 = EventBusConnector::new(temp_dir1.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    let connector2 = EventBusConnector::new(temp_dir2.path().to_path_buf())
        .await
        .expect("Failed to create connector");

    assert_ne!(connector1.client_id, connector2.client_id);
    assert!(connector1.client_id.starts_with("procmond-"));
    assert!(connector2.client_id.starts_with("procmond-"));
}
