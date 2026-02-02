//! Integration tests for heartbeat detection and recovery functionality.
//!
//! These tests verify the heartbeat monitoring, missed heartbeat detection,
//! and escalating recovery action infrastructure.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::semicolon_outside_block,
    clippy::semicolon_inside_block
)]

use daemoneye_agent::collector_registry::{
    CollectorRegistry, HeartbeatStatus, MAX_MISSED_HEARTBEATS,
};
use daemoneye_agent::recovery::{
    CollectorRecoveryState, MAX_RECOVERY_ATTEMPTS, RecoveryAction, RecoveryResult,
};
use daemoneye_eventbus::rpc::RegistrationRequest;
use std::collections::HashMap;
use std::time::Duration;

fn create_registration_request(collector_id: &str) -> RegistrationRequest {
    RegistrationRequest {
        collector_id: collector_id.to_string(),
        collector_type: "process".to_string(),
        hostname: "test-host".to_string(),
        version: Some("1.0.0".to_string()),
        pid: Some(12345),
        capabilities: vec!["process".to_string()],
        attributes: HashMap::new(),
        heartbeat_interval_ms: Some(1000), // 1 second heartbeat
    }
}

// ------------------------------------------------
// HeartbeatStatus Tests
// ------------------------------------------------

#[test]
fn test_heartbeat_status_needs_recovery() {
    // Healthy status does not need recovery
    assert!(!HeartbeatStatus::Healthy.needs_recovery());

    // Degraded status does not need recovery
    assert!(!HeartbeatStatus::Degraded { missed_count: 1 }.needs_recovery());
    assert!(!HeartbeatStatus::Degraded { missed_count: 2 }.needs_recovery());

    // Failed status needs recovery
    assert!(
        HeartbeatStatus::Failed {
            missed_count: 3,
            time_since_last: Duration::from_secs(90),
        }
        .needs_recovery()
    );
}

#[test]
fn test_heartbeat_status_is_healthy() {
    assert!(HeartbeatStatus::Healthy.is_healthy());
    assert!(!HeartbeatStatus::Degraded { missed_count: 1 }.is_healthy());
    assert!(
        !HeartbeatStatus::Failed {
            missed_count: 3,
            time_since_last: Duration::from_secs(90),
        }
        .is_healthy()
    );
}

// ------------------------------------------------
// CollectorRegistry Heartbeat Tests
// ------------------------------------------------

#[tokio::test]
async fn test_registry_update_heartbeat_resets_missed_count() {
    let registry = CollectorRegistry::default();

    // Register a collector
    let request = create_registration_request("heartbeat-test");
    registry
        .register(request)
        .await
        .expect("registration succeeds");

    // Update heartbeat should succeed
    registry
        .update_heartbeat("heartbeat-test")
        .await
        .expect("heartbeat update succeeds");

    // Get the collector record to verify
    let record = registry
        .get("heartbeat-test")
        .await
        .expect("collector exists");
    assert_eq!(record.missed_heartbeats, 0);
}

#[tokio::test]
async fn test_registry_update_heartbeat_unknown_collector() {
    let registry = CollectorRegistry::default();

    // Update heartbeat for non-existent collector should fail
    let result = registry.update_heartbeat("unknown-collector").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_registry_heartbeat_status_healthy() {
    let registry = CollectorRegistry::default();

    // Register a collector
    let request = create_registration_request("healthy-collector");
    registry
        .register(request)
        .await
        .expect("registration succeeds");

    // Immediately after registration, status should be healthy
    let status = registry.heartbeat_status("healthy-collector").await;
    assert!(status.is_some());
    assert!(matches!(status.unwrap(), HeartbeatStatus::Healthy));
}

#[tokio::test]
async fn test_registry_heartbeat_status_unknown_collector() {
    let registry = CollectorRegistry::default();

    // Status for non-existent collector should be None
    let status = registry.heartbeat_status("unknown-collector").await;
    assert!(status.is_none());
}

#[tokio::test]
async fn test_registry_reset_missed_heartbeats() {
    let registry = CollectorRegistry::default();

    // Register a collector
    let request = create_registration_request("reset-test");
    registry
        .register(request)
        .await
        .expect("registration succeeds");

    // Reset missed heartbeats should succeed
    registry
        .reset_missed_heartbeats("reset-test")
        .await
        .expect("reset succeeds");

    // Verify it's still at 0
    let record = registry.get("reset-test").await.expect("collector exists");
    assert_eq!(record.missed_heartbeats, 0);
}

// ------------------------------------------------
// RecoveryAction Tests
// ------------------------------------------------

#[test]
fn test_recovery_action_escalation_chain() {
    // Full escalation chain
    let action = RecoveryAction::HealthCheck;
    let action = action.escalate().expect("can escalate from HealthCheck");
    assert_eq!(action, RecoveryAction::GracefulShutdown);

    let action = action
        .escalate()
        .expect("can escalate from GracefulShutdown");
    assert_eq!(action, RecoveryAction::ForceKill);

    let action = action.escalate().expect("can escalate from ForceKill");
    assert_eq!(action, RecoveryAction::Restart);

    // Cannot escalate from Restart
    assert!(action.escalate().is_none());
}

#[test]
fn test_recovery_action_from_degraded_status() {
    // Low missed count - no action needed
    let status = HeartbeatStatus::Degraded { missed_count: 1 };
    assert!(RecoveryAction::from_heartbeat_status(&status).is_none());

    // Higher missed count - trigger health check
    let status = HeartbeatStatus::Degraded { missed_count: 2 };
    assert_eq!(
        RecoveryAction::from_heartbeat_status(&status),
        Some(RecoveryAction::HealthCheck)
    );

    let status = HeartbeatStatus::Degraded { missed_count: 3 };
    assert_eq!(
        RecoveryAction::from_heartbeat_status(&status),
        Some(RecoveryAction::HealthCheck)
    );
}

#[test]
fn test_recovery_action_from_failed_status() {
    let status = HeartbeatStatus::Failed {
        missed_count: MAX_MISSED_HEARTBEATS,
        time_since_last: Duration::from_secs(120),
    };

    // Failed status should trigger graceful shutdown (skip health check)
    assert_eq!(
        RecoveryAction::from_heartbeat_status(&status),
        Some(RecoveryAction::GracefulShutdown)
    );
}

// ------------------------------------------------
// CollectorRecoveryState Tests
// ------------------------------------------------

#[test]
fn test_recovery_state_lifecycle() {
    let mut state = CollectorRecoveryState::new("test-collector".to_string());

    // Initial state
    assert!(!state.in_progress);
    assert!(state.current_action.is_none());
    assert_eq!(state.attempt_count, 0);
    assert!(!state.is_exhausted());

    // Start recovery
    state.start_recovery(RecoveryAction::HealthCheck);
    assert!(state.in_progress);
    assert_eq!(state.current_action, Some(RecoveryAction::HealthCheck));
    assert_eq!(state.attempt_count, 1);

    // Complete recovery successfully
    state.complete_recovery(RecoveryResult::Success {
        action: RecoveryAction::HealthCheck,
        message: "OK".to_string(),
    });
    assert!(!state.in_progress);
    assert!(matches!(
        state.last_result,
        Some(RecoveryResult::Success { .. })
    ));

    // Reset state
    state.reset();
    assert!(!state.in_progress);
    assert!(state.current_action.is_none());
    assert_eq!(state.attempt_count, 0);
    assert!(state.last_result.is_none());
}

#[test]
fn test_recovery_state_exhaustion() {
    let mut state = CollectorRecoveryState::new("exhausted-collector".to_string());

    // Simulate multiple recovery attempts
    for i in 0..MAX_RECOVERY_ATTEMPTS {
        assert!(
            !state.is_exhausted(),
            "Should not be exhausted at attempt {i}"
        );
        state.start_recovery(RecoveryAction::HealthCheck);
        state.complete_recovery(RecoveryResult::Failed {
            action: RecoveryAction::HealthCheck,
            error: "Test failure".to_string(),
            should_escalate: true,
        });
    }

    // Should now be exhausted
    assert!(state.is_exhausted());
}

// ------------------------------------------------
// Integration Tests - Heartbeat + Recovery Workflow
// ------------------------------------------------

#[tokio::test]
async fn test_heartbeat_to_recovery_workflow() {
    let registry = CollectorRegistry::default();

    // Register collector with short heartbeat interval
    let mut request = create_registration_request("workflow-test");
    request.heartbeat_interval_ms = Some(100); // 100ms heartbeat
    registry
        .register(request)
        .await
        .expect("registration succeeds");

    // Initially healthy
    let status = registry.heartbeat_status("workflow-test").await;
    assert!(matches!(status, Some(HeartbeatStatus::Healthy)));
    assert!(RecoveryAction::from_heartbeat_status(&status.unwrap()).is_none());

    // Simulate sending heartbeat
    registry
        .update_heartbeat("workflow-test")
        .await
        .expect("heartbeat update succeeds");

    // Status should still be healthy
    let status = registry.heartbeat_status("workflow-test").await;
    assert!(matches!(status, Some(HeartbeatStatus::Healthy)));
}

#[tokio::test]
async fn test_multiple_collectors_heartbeat_tracking() {
    let registry = CollectorRegistry::default();

    // Register multiple collectors
    let collectors = vec!["collector-a", "collector-b", "collector-c"];
    for collector_id in &collectors {
        let request = create_registration_request(collector_id);
        registry
            .register(request)
            .await
            .expect("registration succeeds");
    }

    // Update heartbeats for all
    for collector_id in &collectors {
        registry
            .update_heartbeat(collector_id)
            .await
            .expect("heartbeat update succeeds");
    }

    // All should be healthy
    for collector_id in &collectors {
        let status = registry.heartbeat_status(collector_id).await;
        assert!(
            matches!(status, Some(HeartbeatStatus::Healthy)),
            "Collector {collector_id} should be healthy"
        );
    }
}

#[tokio::test]
async fn test_list_collector_ids() {
    let registry = CollectorRegistry::default();

    // Register multiple collectors
    let expected_ids = vec!["list-test-a", "list-test-b"];
    for collector_id in &expected_ids {
        let request = create_registration_request(collector_id);
        registry
            .register(request)
            .await
            .expect("registration succeeds");
    }

    // List should contain all registered collectors
    let ids = registry.list_collector_ids().await;
    assert_eq!(ids.len(), expected_ids.len());
    for expected_id in &expected_ids {
        assert!(
            ids.contains(&expected_id.to_string()),
            "Should contain {expected_id}"
        );
    }
}

#[test]
fn test_max_missed_heartbeats_constant() {
    // Verify the constant value matches expectations
    assert_eq!(MAX_MISSED_HEARTBEATS, 3);
}

#[test]
fn test_max_recovery_attempts_constant() {
    // Verify the constant value matches expectations
    assert_eq!(MAX_RECOVERY_ATTEMPTS, 3);
}

// ------------------------------------------------
// Recovery Result Tests
// ------------------------------------------------

#[test]
fn test_recovery_result_success() {
    let result = RecoveryResult::Success {
        action: RecoveryAction::HealthCheck,
        message: "Collector recovered".to_string(),
    };

    if let RecoveryResult::Success { action, message } = result {
        assert_eq!(action, RecoveryAction::HealthCheck);
        assert_eq!(message, "Collector recovered");
    } else {
        panic!("Expected Success result");
    }
}

#[test]
fn test_recovery_result_failed() {
    let result = RecoveryResult::Failed {
        action: RecoveryAction::GracefulShutdown,
        error: "Connection refused".to_string(),
        should_escalate: true,
    };

    if let RecoveryResult::Failed {
        action,
        error,
        should_escalate,
    } = result
    {
        assert_eq!(action, RecoveryAction::GracefulShutdown);
        assert_eq!(error, "Connection refused");
        assert!(should_escalate);
    } else {
        panic!("Expected Failed result");
    }
}

#[test]
fn test_recovery_result_skipped() {
    let result = RecoveryResult::Skipped {
        reason: "Collector already healthy".to_string(),
    };

    if let RecoveryResult::Skipped { reason } = result {
        assert_eq!(reason, "Collector already healthy");
    } else {
        panic!("Expected Skipped result");
    }
}

#[test]
fn test_recovery_result_exhausted() {
    let result = RecoveryResult::Exhausted {
        attempts: 3,
        last_action: RecoveryAction::Restart,
    };

    if let RecoveryResult::Exhausted {
        attempts,
        last_action,
    } = result
    {
        assert_eq!(attempts, 3);
        assert_eq!(last_action, RecoveryAction::Restart);
    } else {
        panic!("Expected Exhausted result");
    }
}
