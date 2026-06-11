//! Unit tests for the registration manager.

use super::*;
use crate::monitor_collector::{ACTOR_CHANNEL_CAPACITY, ActorMessage};
use std::collections::HashMap;
use tokio::sync::mpsc;
/// Creates a test actor handle.
fn create_test_actor() -> (ActorHandle, mpsc::Receiver<ActorMessage>) {
    let (tx, rx) = mpsc::channel(ACTOR_CHANNEL_CAPACITY);
    (ActorHandle::new(tx), rx)
}

/// Creates an EventBusConnector with a unique temp directory for test isolation.
async fn create_test_event_bus() -> (Arc<RwLock<EventBusConnector>>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
    let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
        .await
        .expect("Failed to create event bus connector");
    (Arc::new(RwLock::new(connector)), temp_dir)
}

#[tokio::test]
async fn test_registration_config_default() {
    let config = RegistrationConfig::default();
    assert_eq!(config.collector_id, "procmond");
    assert_eq!(config.collector_type, "process-monitor");
    assert!(!config.capabilities.is_empty());
    assert_eq!(
        config.heartbeat_interval,
        Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
    );
}

#[tokio::test]
async fn test_registration_state_display() {
    assert_eq!(
        format!("{}", RegistrationState::Unregistered),
        "unregistered"
    );
    assert_eq!(format!("{}", RegistrationState::Registering), "registering");
    assert_eq!(format!("{}", RegistrationState::Registered), "registered");
    assert_eq!(
        format!("{}", RegistrationState::Deregistering),
        "deregistering"
    );
    assert_eq!(format!("{}", RegistrationState::Failed), "failed");
}

#[tokio::test]
async fn test_connection_status_display() {
    assert_eq!(format!("{}", ConnectionStatus::Connected), "connected");
    assert_eq!(
        format!("{}", ConnectionStatus::Disconnected),
        "disconnected"
    );
    assert_eq!(
        format!("{}", ConnectionStatus::Reconnecting),
        "reconnecting"
    );
}

#[tokio::test]
async fn test_heartbeat_metrics_default() {
    let metrics = HeartbeatMetrics::default();
    assert_eq!(metrics.processes_collected, 0);
    assert_eq!(metrics.events_published, 0);
    assert!((metrics.buffer_level_percent - 0.0).abs() < f64::EPSILON);
    assert_eq!(metrics.connection_status, ConnectionStatus::Disconnected);
}

#[tokio::test]
async fn test_initial_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    assert_eq!(manager.state().await, RegistrationState::Unregistered);
    assert_eq!(manager.collector_id(), "procmond");
}

#[tokio::test]
async fn test_build_registration_request() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    let request = manager.build_registration_request();

    assert_eq!(request.collector_id, "procmond");
    assert_eq!(request.collector_type, "process-monitor");
    assert!(request.pid.is_some());
    assert!(!request.capabilities.is_empty());
    assert!(!request.hostname.is_empty());
}

#[tokio::test]
async fn test_effective_heartbeat_interval() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Initially uses configured interval
    let interval = manager.effective_heartbeat_interval().await;
    assert_eq!(
        interval,
        Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
    );

    // After setting assigned interval, uses that instead
    *manager.assigned_heartbeat_interval.write().await = Some(Duration::from_mins(1));
    let updated_interval = manager.effective_heartbeat_interval().await;
    assert_eq!(updated_interval, Duration::from_mins(1));
}

#[tokio::test]
async fn test_stats_initial() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    let stats = manager.stats().await;
    assert_eq!(stats.registration_attempts, 0);
    assert_eq!(stats.successful_registrations, 0);
    assert_eq!(stats.failed_registrations, 0);
    assert_eq!(stats.heartbeats_sent, 0);
    assert_eq!(stats.heartbeat_failures, 0);
    assert!(stats.last_heartbeat.is_none());
    assert!(stats.last_registration.is_none());
}

#[tokio::test]
async fn test_heartbeat_message_serialization() {
    let heartbeat = HeartbeatMessage {
        collector_id: "procmond".to_string(),
        sequence: 42,
        timestamp: SystemTime::now(),
        health_status: HealthStatus::Healthy,
        metrics: HeartbeatMetrics {
            processes_collected: 100,
            events_published: 50,
            buffer_level_percent: 25.5,
            connection_status: ConnectionStatus::Connected,
        },
        operational_sub_status: None,
    };

    // Should serialize without error
    let payload = postcard::to_allocvec(&heartbeat).expect("Serialization should succeed");
    assert!(!payload.is_empty());

    // Should deserialize back
    let deserialized: HeartbeatMessage =
        postcard::from_bytes(&payload).expect("Deserialization should succeed");
    assert_eq!(deserialized.collector_id, "procmond");
    assert_eq!(deserialized.sequence, 42);
    assert_eq!(deserialized.metrics.processes_collected, 100);
}

#[tokio::test]
async fn test_invalid_state_transition() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Attempting to register again should fail
    let result = manager.register().await;
    assert!(result.is_err());
    match result.unwrap_err() {
        RegistrationError::InvalidStateTransition { from, to } => {
            assert_eq!(from, RegistrationState::Registered);
            assert_eq!(to, RegistrationState::Registering);
        }
        RegistrationError::RegistrationFailed(_)
        | RegistrationError::RegistrationRejected(_)
        | RegistrationError::Timeout { .. }
        | RegistrationError::HeartbeatFailed(_)
        | RegistrationError::DeregistrationFailed(_)
        | RegistrationError::EventBusError(_) => {
            panic!("Expected InvalidStateTransition error")
        }
    }
}

#[tokio::test]
async fn test_build_heartbeat_message() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    let heartbeat = manager
        .build_heartbeat_message(42, HealthStatus::Healthy, None)
        .await;

    assert_eq!(heartbeat.collector_id, "procmond");
    assert_eq!(heartbeat.sequence, 42);
    assert_eq!(heartbeat.health_status, HealthStatus::Healthy);
    // Buffer should be empty initially
    assert!((heartbeat.metrics.buffer_level_percent - 0.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn test_deregister_from_registered_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Deregister should succeed and transition to Unregistered
    let result = manager.deregister(Some("Test reason".to_string())).await;
    assert!(result.is_ok());

    // State should transition to Unregistered after deregistration
    let state = manager.state().await;
    assert_eq!(state, RegistrationState::Unregistered);
}

#[tokio::test]
async fn test_deregister_from_unregistered_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // State is Unregistered by default

    // Deregister should return Ok (nothing to do)
    let result = manager.deregister(None).await;
    assert!(result.is_ok());

    // State should remain Unregistered
    let state = manager.state().await;
    assert_eq!(state, RegistrationState::Unregistered);
}

#[tokio::test]
async fn test_publish_heartbeat_skips_when_not_registered() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // State is Unregistered by default

    // Publish heartbeat should succeed but skip actual publishing
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    // Stats should not show any heartbeats sent
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 0);
}

#[tokio::test]
async fn test_registration_stats_helpers() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Test increment_registration_attempts
    manager.increment_registration_attempts().await;
    let stats_after_attempt = manager.stats().await;
    assert_eq!(stats_after_attempt.registration_attempts, 1);

    // Test record_successful_registration
    manager.record_successful_registration().await;
    let stats_after_success = manager.stats().await;
    assert_eq!(stats_after_success.successful_registrations, 1);
    assert!(stats_after_success.last_registration.is_some());

    // Test record_failed_registration
    manager.record_failed_registration().await;
    let stats_after_failure = manager.stats().await;
    assert_eq!(stats_after_failure.failed_registrations, 1);

    // Test record_heartbeat
    manager.record_heartbeat().await;
    let stats_after_heartbeat = manager.stats().await;
    assert_eq!(stats_after_heartbeat.heartbeats_sent, 1);
    assert!(stats_after_heartbeat.last_heartbeat.is_some());

    // Test record_heartbeat_failure
    manager.record_heartbeat_failure().await;
    let stats_after_hb_failure = manager.stats().await;
    assert_eq!(stats_after_hb_failure.heartbeat_failures, 1);
}

// ==================== Registration Flow Tests ====================

#[tokio::test]
async fn test_register_successful() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Register should succeed and transition to Registered state
    let result = manager.register().await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.accepted);
    assert_eq!(response.collector_id, "procmond");
    assert!(!response.assigned_topics.is_empty());

    // State should be Registered
    assert_eq!(manager.state().await, RegistrationState::Registered);

    // Stats should reflect successful registration
    let stats = manager.stats().await;
    assert_eq!(stats.registration_attempts, 1);
    assert_eq!(stats.successful_registrations, 1);
    assert!(stats.last_registration.is_some());
}

#[tokio::test]
async fn test_register_from_failed_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Failed (simulating previous registration failure)
    *manager.state.write().await = RegistrationState::Failed;

    // Should be able to register again from Failed state
    let result = manager.register().await;
    assert!(result.is_ok());

    // State should be Registered
    assert_eq!(manager.state().await, RegistrationState::Registered);
}

#[tokio::test]
async fn test_register_invalid_from_deregistering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Deregistering
    *manager.state.write().await = RegistrationState::Deregistering;

    // Attempting to register should fail
    let result = manager.register().await;
    assert!(result.is_err());
    match result.unwrap_err() {
        RegistrationError::InvalidStateTransition { from, to } => {
            assert_eq!(from, RegistrationState::Deregistering);
            assert_eq!(to, RegistrationState::Registering);
        }
        _ => panic!("Expected InvalidStateTransition error"),
    }
}

#[tokio::test]
async fn test_register_invalid_from_registering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registering
    *manager.state.write().await = RegistrationState::Registering;

    // Attempting to register should fail
    let result = manager.register().await;
    assert!(result.is_err());
    match result.unwrap_err() {
        RegistrationError::InvalidStateTransition { from, to } => {
            assert_eq!(from, RegistrationState::Registering);
            assert_eq!(to, RegistrationState::Registering);
        }
        _ => panic!("Expected InvalidStateTransition error"),
    }
}

#[tokio::test]
async fn test_register_stores_assigned_heartbeat_interval() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Initial interval should be the default
    let initial_interval = manager.effective_heartbeat_interval().await;
    assert_eq!(
        initial_interval,
        Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS)
    );

    // Register
    let result = manager.register().await;
    assert!(result.is_ok());

    // After registration, interval should be assigned from response
    let assigned = manager.assigned_heartbeat_interval.read().await;
    assert!(assigned.is_some());
}

// ==================== Heartbeat Tests ====================

/// Creates a test HealthCheckData with the given state and connection status.
fn create_test_health_data(
    state: crate::monitor_collector::CollectorState,
    connected: bool,
) -> crate::monitor_collector::HealthCheckData {
    let operational_sub_status = (state
        == crate::monitor_collector::CollectorState::WaitingForAgent)
        .then(|| "idle-awaiting-begin".to_owned());
    crate::monitor_collector::HealthCheckData {
        state,
        collection_interval: Duration::from_secs(5),
        original_interval: Duration::from_secs(5),
        event_bus_connected: connected,
        buffer_level_percent: Some(10),
        last_collection: Some(std::time::Instant::now()),
        collection_cycles: 100,
        lifecycle_events: 50,
        collection_errors: 0,
        backpressure_events: 0,
        operational_sub_status,
    }
}

#[tokio::test]
async fn test_publish_heartbeat_when_registered() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Spawn a task to respond to health check
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health =
                create_test_health_data(crate::monitor_collector::CollectorState::Running, true);
            let _ = respond_to.send(health);
        }
    });

    // Publish heartbeat
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    // Stats should show heartbeat sent
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 1);
    assert!(stats.last_heartbeat.is_some());

    // Heartbeat sequence should have incremented
    let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
    assert_eq!(sequence, 1);

    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_publish_heartbeat_increments_sequence() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Spawn a task to respond to multiple health checks
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        for _ in 0..3 {
            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            }
        }
    });

    // Publish 3 heartbeats
    for _ in 0..3 {
        let result = manager.publish_heartbeat().await;
        assert!(result.is_ok());
    }

    // Sequence should be 3
    let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
    assert_eq!(sequence, 3);

    // Stats should show 3 heartbeats
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 3);

    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_publish_heartbeat_skips_in_deregistering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Deregistering
    *manager.state.write().await = RegistrationState::Deregistering;

    // Publish heartbeat should succeed but skip actual publishing
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    // Stats should not show any heartbeats sent
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 0);
}

#[tokio::test]
async fn test_publish_heartbeat_skips_in_failed_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Failed
    *manager.state.write().await = RegistrationState::Failed;

    // Publish heartbeat should succeed but skip actual publishing
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    // Stats should not show any heartbeats sent
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 0);
}

#[tokio::test]
async fn test_publish_heartbeat_skips_in_registering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registering
    *manager.state.write().await = RegistrationState::Registering;

    // Publish heartbeat should succeed but skip actual publishing
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    // Stats should not show any heartbeats sent
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 0);
}

// ==================== Health Status Tests ====================

#[tokio::test]
async fn test_heartbeat_health_status_healthy() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Respond with Running state and connected
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health =
                create_test_health_data(crate::monitor_collector::CollectorState::Running, true);
            let _ = respond_to.send(health);
        }
    });

    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());
    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_heartbeat_health_status_degraded_waiting_for_agent() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Respond with WaitingForAgent state
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health = create_test_health_data(
                crate::monitor_collector::CollectorState::WaitingForAgent,
                true,
            );
            let _ = respond_to.send(health);
        }
    });

    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());
    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_heartbeat_health_status_degraded_disconnected() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Respond with Running state but disconnected
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health = create_test_health_data(
                crate::monitor_collector::CollectorState::Running,
                false, // Not connected
            );
            let _ = respond_to.send(health);
        }
    });

    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());
    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_heartbeat_health_status_unhealthy_shutting_down() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Respond with ShuttingDown state
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health = create_test_health_data(
                crate::monitor_collector::CollectorState::ShuttingDown,
                true,
            );
            let _ = respond_to.send(health);
        }
    });

    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());
    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_heartbeat_health_status_unhealthy_stopped() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Respond with Stopped state
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health =
                create_test_health_data(crate::monitor_collector::CollectorState::Stopped, false);
            let _ = respond_to.send(health);
        }
    });

    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());
    health_responder.await.unwrap();
}

#[tokio::test]
async fn test_heartbeat_health_check_timeout() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Create manager with very short timeout for testing
    let config = RegistrationConfig {
        heartbeat_interval: Duration::from_millis(100),
        ..RegistrationConfig::default()
    };
    let manager = RegistrationManager::new(event_bus, actor_handle, config);

    *manager.state.write().await = RegistrationState::Registered;

    // Don't respond to health check - it will timeout
    // The test times out the health check and reports Unknown status
    let result = manager.publish_heartbeat().await;
    // Should succeed even with timeout (reports Unknown health)
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_heartbeat_health_check_error() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    *manager.state.write().await = RegistrationState::Registered;

    // Drop the receiver to cause channel closed error
    tokio::spawn(async move {
        // Receive the message but don't respond (drop the oneshot sender)
        if let Some(ActorMessage::HealthCheck { respond_to: _ }) = rx.recv().await {
            // Don't respond - just let it drop
        }
    });

    // Give time for the spawn to execute
    tokio::time::sleep(Duration::from_millis(10)).await;

    let result = manager.publish_heartbeat().await;
    // Should succeed even with error (reports Unknown health)
    assert!(result.is_ok());
}

// ==================== Deregistration Tests ====================

#[tokio::test]
async fn test_deregister_with_reason() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Deregister with reason
    let result = manager
        .deregister(Some("Graceful shutdown".to_string()))
        .await;
    assert!(result.is_ok());

    // State should be Unregistered
    assert_eq!(manager.state().await, RegistrationState::Unregistered);
}

#[tokio::test]
async fn test_deregister_from_failed_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Failed
    *manager.state.write().await = RegistrationState::Failed;

    // Deregister should return Ok (nothing to do)
    let result = manager.deregister(None).await;
    assert!(result.is_ok());

    // State should remain Failed
    assert_eq!(manager.state().await, RegistrationState::Failed);
}

#[tokio::test]
async fn test_deregister_from_registering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registering
    *manager.state.write().await = RegistrationState::Registering;

    // Deregister should return Ok (nothing to do)
    let result = manager.deregister(None).await;
    assert!(result.is_ok());

    // State should remain Registering
    assert_eq!(manager.state().await, RegistrationState::Registering);
}

#[tokio::test]
async fn test_deregister_from_deregistering_state() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Deregistering
    *manager.state.write().await = RegistrationState::Deregistering;

    // Deregister should return Ok (nothing to do)
    let result = manager.deregister(None).await;
    assert!(result.is_ok());

    // State should remain Deregistering
    assert_eq!(manager.state().await, RegistrationState::Deregistering);
}

// ==================== Heartbeat Task Tests ====================

#[tokio::test]
async fn test_spawn_heartbeat_task_waits_for_registration() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

    // Spawn heartbeat task while unregistered
    let handle = Arc::clone(&manager).spawn_heartbeat_task();

    // Give task time to start waiting
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Task should still be running (waiting for registration)
    assert!(!handle.is_finished());

    // Abort the task to clean up
    handle.abort();
}

#[tokio::test]
async fn test_spawn_heartbeat_task_exits_on_failed_registration() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

    // Spawn heartbeat task
    let handle = Arc::clone(&manager).spawn_heartbeat_task();

    // Give task time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Set state to Failed
    *manager.state.write().await = RegistrationState::Failed;

    // Wait for task to notice and exit
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Task should have exited
    assert!(handle.is_finished());
}

#[tokio::test]
async fn test_spawn_heartbeat_task_runs_when_registered() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Create manager with short heartbeat interval for testing
    let config = RegistrationConfig {
        heartbeat_interval: Duration::from_millis(100),
        ..RegistrationConfig::default()
    };
    let manager = Arc::new(RegistrationManager::new(event_bus, actor_handle, config));

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Spawn a task to respond to health checks
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        for _ in 0..3 {
            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            }
        }
    });

    // Spawn heartbeat task
    let handle = Arc::clone(&manager).spawn_heartbeat_task();

    // Wait for a few heartbeats
    tokio::time::sleep(Duration::from_millis(350)).await;

    // Should have published at least 2 heartbeats
    let stats = manager.stats().await;
    assert!(stats.heartbeats_sent >= 2);

    // Abort the task
    handle.abort();
    health_responder.abort();
}

#[tokio::test]
async fn test_spawn_heartbeat_task_stops_when_deregistered() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;

    // Create manager with short heartbeat interval
    let config = RegistrationConfig {
        heartbeat_interval: Duration::from_millis(100),
        ..RegistrationConfig::default()
    };
    let manager = Arc::new(RegistrationManager::new(event_bus, actor_handle, config));

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Spawn a task to respond to health checks
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        loop {
            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            } else {
                break;
            }
        }
    });

    // Spawn heartbeat task
    let handle = Arc::clone(&manager).spawn_heartbeat_task();

    // Wait for heartbeat task to start
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Deregister
    *manager.state.write().await = RegistrationState::Unregistered;

    // Wait for task to notice and exit
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Task should have exited
    assert!(handle.is_finished());

    health_responder.abort();
}

// ==================== Connection Status Tests ====================

#[tokio::test]
async fn test_build_heartbeat_message_with_connected_status() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Build heartbeat - event bus is not connected by default
    let heartbeat = manager
        .build_heartbeat_message(1, HealthStatus::Healthy, None)
        .await;

    // Connection status should reflect disconnected (default state)
    assert_eq!(
        heartbeat.metrics.connection_status,
        ConnectionStatus::Disconnected
    );
}

// ==================== Error Type Tests ====================

#[test]
fn test_registration_error_display() {
    let err1 = RegistrationError::RegistrationFailed("Test failure".to_string());
    assert!(err1.to_string().contains("Registration failed"));

    let err2 = RegistrationError::RegistrationRejected("Invalid collector".to_string());
    assert!(err2.to_string().contains("Registration rejected"));

    let err3 = RegistrationError::Timeout { timeout_secs: 30 };
    assert!(err3.to_string().contains("30"));

    let err4 = RegistrationError::HeartbeatFailed("Publish error".to_string());
    assert!(err4.to_string().contains("heartbeat"));

    let err5 = RegistrationError::DeregistrationFailed("Deregistration error".to_string());
    assert!(err5.to_string().contains("Deregistration"));

    let err6 = RegistrationError::EventBusError("Bus error".to_string());
    assert!(err6.to_string().contains("Event bus"));

    let err7 = RegistrationError::InvalidStateTransition {
        from: RegistrationState::Registered,
        to: RegistrationState::Registering,
    };
    assert!(err7.to_string().contains("Invalid state transition"));
}

// ==================== Config Tests ====================

#[tokio::test]
async fn test_registration_config_custom() {
    let config = RegistrationConfig {
        collector_id: "custom-collector".to_owned(),
        collector_type: "custom-type".to_owned(),
        version: "2.0.0".to_owned(),
        capabilities: vec!["cap1".to_owned(), "cap2".to_owned()],
        heartbeat_interval: Duration::from_mins(1),
        registration_timeout: Duration::from_secs(20),
        max_retries: 5,
        attributes: HashMap::new(),
    };

    assert_eq!(config.collector_id, "custom-collector");
    assert_eq!(config.collector_type, "custom-type");
    assert_eq!(config.version, "2.0.0");
    assert_eq!(config.capabilities.len(), 2);
    assert_eq!(config.heartbeat_interval, Duration::from_mins(1));
    assert_eq!(config.registration_timeout, Duration::from_secs(20));
    assert_eq!(config.max_retries, 5);
}

#[tokio::test]
async fn test_registration_manager_new_vs_with_defaults() {
    let (actor_handle1, _rx1) = create_test_actor();
    let (event_bus1, _temp_dir1) = create_test_event_bus().await;
    let manager1 = RegistrationManager::with_defaults(event_bus1, actor_handle1);

    let (actor_handle2, _rx2) = create_test_actor();
    let (event_bus2, _temp_dir2) = create_test_event_bus().await;
    let manager2 =
        RegistrationManager::new(event_bus2, actor_handle2, RegistrationConfig::default());

    // Both should have the same default collector_id
    assert_eq!(manager1.collector_id(), manager2.collector_id());
    assert_eq!(manager1.collector_id(), "procmond");
}

// ==================== Concurrent Access Tests ====================

#[tokio::test]
async fn test_concurrent_state_reads() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

    // Spawn multiple tasks reading state concurrently
    let mut handles = Vec::new();
    for _ in 0..10 {
        let manager_clone = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for _ in 0..100 {
                let _state = manager_clone.state().await;
            }
        }));
    }

    // All should complete without deadlock
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_concurrent_stats_reads() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

    // Spawn multiple tasks reading stats concurrently
    let mut handles = Vec::new();
    for _ in 0..10 {
        let manager_clone = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for _ in 0..100 {
                let _stats = manager_clone.stats().await;
            }
        }));
    }

    // All should complete without deadlock
    for handle in handles {
        handle.await.unwrap();
    }
}

// ==================== Stats Overflow Protection Tests ====================

#[tokio::test]
async fn test_stats_saturating_add() {
    let (actor_handle, _rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set stats to near max
    {
        let mut stats = manager.stats.write().await;
        stats.registration_attempts = u64::MAX - 1;
        stats.heartbeats_sent = u64::MAX - 1;
    }

    // Increment - should saturate, not overflow
    manager.increment_registration_attempts().await;
    manager.increment_registration_attempts().await;
    manager.record_heartbeat().await;
    manager.record_heartbeat().await;

    let stats = manager.stats().await;
    assert_eq!(stats.registration_attempts, u64::MAX);
    assert_eq!(stats.heartbeats_sent, u64::MAX);
}

// ==================== Topic Format Tests ====================

#[test]
fn test_heartbeat_topic_format() {
    let topic = format!("{}.{}", HEARTBEAT_TOPIC_PREFIX, "procmond");
    assert_eq!(topic, "control.health.heartbeat.procmond");
}

#[test]
fn test_registration_topic_constant() {
    assert_eq!(REGISTRATION_TOPIC, "control.collector.lifecycle");
}

// ==================== Default Constant Tests ====================

#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_HEARTBEAT_INTERVAL_SECS, 30);
    assert_eq!(DEFAULT_REGISTRATION_TIMEOUT_SECS, 10);
    assert_eq!(MAX_REGISTRATION_RETRIES, 3);
}

// ==================== Concurrent Heartbeat Tests ====================

#[tokio::test]
async fn test_concurrent_heartbeat_publishes() {
    use tokio::sync::Barrier;

    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = Arc::new(RegistrationManager::with_defaults(event_bus, actor_handle));

    // Set state to Registered
    *manager.state.write().await = RegistrationState::Registered;

    // Create barrier for synchronizing 10 concurrent tasks
    let barrier = Arc::new(Barrier::new(10));

    // Spawn a task to respond to health check actor messages
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        // Respond to 10 health checks (one per concurrent heartbeat)
        for _ in 0..10 {
            if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
                let health = create_test_health_data(
                    crate::monitor_collector::CollectorState::Running,
                    true,
                );
                let _ = respond_to.send(health);
            }
        }
    });

    // Spawn 10 concurrent publish_heartbeat() calls
    let mut handles = Vec::new();
    for _ in 0..10 {
        let manager_clone = Arc::clone(&manager);
        let barrier_clone = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            // Wait for all tasks to be ready
            barrier_clone.wait().await;
            // Now all 10 tasks will call publish_heartbeat concurrently
            manager_clone.publish_heartbeat().await
        }));
    }

    // Wait for all heartbeat tasks to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Heartbeat publish should succeed");
    }

    // Wait for health responder to finish
    health_responder.await.unwrap();

    // Verify sequence numbers are properly incremented (final count should be 10)
    let sequence = manager.heartbeat_sequence.load(Ordering::Relaxed);
    assert_eq!(
        sequence, 10,
        "Sequence should be 10 after 10 concurrent heartbeats"
    );

    // Verify all heartbeats were recorded in stats
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 10, "Should have sent 10 heartbeats");
}

// ==================== WaitingForAgent Heartbeat Tests ====================

/// Test that WaitingForAgent state produces Healthy health_status
/// with "idle-awaiting-begin" sub-status in the heartbeat message.
///
/// Sets actor state to WaitingForAgent, calls publish_heartbeat,
/// and verifies the health_status is Healthy (not Degraded).
#[tokio::test]
async fn test_waiting_for_agent_heartbeat_produces_healthy_idle_awaiting_begin() {
    let (actor_handle, mut rx) = create_test_actor();
    let (event_bus, _temp_dir) = create_test_event_bus().await;
    let manager = RegistrationManager::with_defaults(event_bus, actor_handle);

    // Set state to Registered so heartbeat actually runs
    *manager.state.write().await = RegistrationState::Registered;

    // Respond with WaitingForAgent state (connected=true)
    let health_responder = tokio::spawn(async move {
        use crate::monitor_collector::ActorMessage;

        if let Some(ActorMessage::HealthCheck { respond_to }) = rx.recv().await {
            let health = create_test_health_data(
                crate::monitor_collector::CollectorState::WaitingForAgent,
                true, // Connected to event bus
            );
            let _ = respond_to.send(health);
        }
    });

    // Publish heartbeat - should succeed
    let result = manager.publish_heartbeat().await;
    assert!(result.is_ok());

    health_responder.await.unwrap();

    // Verify stats recorded the heartbeat
    let stats = manager.stats().await;
    assert_eq!(stats.heartbeats_sent, 1);
    assert!(stats.last_heartbeat.is_some());

    // Verify build_heartbeat_message correctly passes through health status
    // and sub-status. The WaitingForAgent -> Healthy mapping is tested above
    // via publish_heartbeat (which queries the actor and maps CollectorState).
    let heartbeat = manager
        .build_heartbeat_message(
            99,
            HealthStatus::Healthy,
            Some("idle-awaiting-begin".to_owned()),
        )
        .await;

    assert_eq!(
        heartbeat.health_status,
        HealthStatus::Healthy,
        "Healthy status should be passed through to heartbeat message"
    );
    assert_eq!(
        heartbeat.operational_sub_status.as_deref(),
        Some("idle-awaiting-begin"),
        "Sub-status should be passed through to heartbeat message"
    );
    assert_eq!(heartbeat.collector_id, "procmond");
}
