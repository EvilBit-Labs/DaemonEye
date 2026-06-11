//! Tests for the agent loading state machine.

use crate::broker_manager::{AgentState, BrokerManager};
use crate::collector_config::{CollectorEntry, CollectorsConfig};
use crate::collector_registry::CollectorRegistry;
use daemoneye_eventbus::rpc::{RegistrationProvider, RegistrationRequest};
use daemoneye_lib::config::BrokerConfig;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_agent_state_initial_loading() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let state = manager.agent_state().await;
    assert_eq!(state, AgentState::Loading);
}

#[tokio::test]
async fn test_agent_state_display() {
    assert_eq!(format!("{}", AgentState::Loading), "Loading");
    assert_eq!(format!("{}", AgentState::Ready), "Ready");
    assert_eq!(format!("{}", AgentState::SteadyState), "SteadyState");
    assert_eq!(
        format!(
            "{}",
            AgentState::StartupFailed {
                reason: "timeout".to_owned()
            }
        ),
        "StartupFailed: timeout"
    );
    assert_eq!(format!("{}", AgentState::ShuttingDown), "ShuttingDown");
}

#[tokio::test]
async fn test_agent_state_helper_methods() {
    assert!(AgentState::Loading.accepts_registrations());
    assert!(AgentState::Ready.accepts_registrations());
    assert!(AgentState::SteadyState.accepts_registrations());
    assert!(
        !AgentState::StartupFailed {
            reason: "test".to_owned()
        }
        .accepts_registrations()
    );
    assert!(!AgentState::ShuttingDown.accepts_registrations());

    assert!(AgentState::Loading.is_running());
    assert!(AgentState::Ready.is_running());
    assert!(AgentState::SteadyState.is_running());
    assert!(
        !AgentState::StartupFailed {
            reason: "test".to_owned()
        }
        .is_running()
    );
    assert!(!AgentState::ShuttingDown.is_running());

    assert!(!AgentState::Loading.is_failed());
    assert!(!AgentState::Ready.is_failed());
    assert!(
        AgentState::StartupFailed {
            reason: "test".to_owned()
        }
        .is_failed()
    );
}

#[tokio::test]
async fn test_set_collectors_config() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let collectors_config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1"),
            CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2").with_enabled(false),
        ],
    };

    manager
        .set_collectors_config(collectors_config.clone())
        .await;

    let loaded_config = manager.collectors_config().await;
    assert_eq!(loaded_config.collectors.len(), 2);

    // Only enabled collectors should be in the readiness tracker
    let pending = manager.pending_collectors().await;
    assert_eq!(pending.len(), 1);
    assert!(pending.contains(&"collector-1".to_owned()));
}

#[tokio::test]
async fn test_mark_collector_ready() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Configure with two enabled collectors
    let collectors_config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1"),
            CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2"),
        ],
    };

    manager.set_collectors_config(collectors_config).await;

    // Initially not all ready
    assert!(!manager.all_collectors_ready().await);

    // Mark first collector ready
    let all_ready = manager.mark_collector_ready("collector-1").await;
    assert!(!all_ready);

    // Mark second collector ready
    let all_ready = manager.mark_collector_ready("collector-2").await;
    assert!(all_ready);

    assert!(manager.all_collectors_ready().await);
    assert!(manager.pending_collectors().await.is_empty());
}

#[tokio::test]
async fn test_all_collectors_ready_with_no_collectors() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Empty config means no collectors expected - should be ready by default
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;
    assert!(manager.all_collectors_ready().await);
}

#[tokio::test]
async fn test_transition_to_ready_succeeds() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Configure with no collectors (automatically ready)
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    // Should succeed since all collectors are ready
    let result = manager.transition_to_ready().await;
    assert!(result.is_ok());

    let state = manager.agent_state().await;
    assert_eq!(state, AgentState::Ready);
}

#[tokio::test]
async fn test_transition_to_ready_fails_with_pending_collectors() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Configure with a collector that hasn't reported ready
    let collectors_config = CollectorsConfig {
        collectors: vec![CollectorEntry::new(
            "pending-collector",
            "type-a",
            "/usr/bin/collector",
        )],
    };
    manager.set_collectors_config(collectors_config).await;

    // Should fail since collector hasn't reported ready
    let result = manager.transition_to_ready().await;
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("pending-collector"));

    // State should still be Loading
    let state = manager.agent_state().await;
    assert_eq!(state, AgentState::Loading);
}

#[tokio::test]
async fn test_transition_to_ready_is_idempotent() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    // First transition
    manager.transition_to_ready().await.unwrap();
    assert_eq!(manager.agent_state().await, AgentState::Ready);

    // Second transition should succeed (no-op)
    let result = manager.transition_to_ready().await;
    assert!(result.is_ok());
    assert_eq!(manager.agent_state().await, AgentState::Ready);
}

#[tokio::test]
async fn test_transition_to_steady_state_fails_from_loading() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Try to skip directly to SteadyState from Loading
    let result = manager.transition_to_steady_state().await;
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("must be in Ready state first"));
}

#[tokio::test]
async fn test_mark_startup_failed() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Configure with a collector that won't report ready
    let collectors_config = CollectorsConfig {
        collectors: vec![CollectorEntry::new(
            "slow-collector",
            "type-a",
            "/usr/bin/collector",
        )],
    };
    manager.set_collectors_config(collectors_config).await;

    // Mark startup as failed
    manager
        .mark_startup_failed("Timeout waiting for collectors".to_owned())
        .await;

    let state = manager.agent_state().await;
    match state {
        AgentState::StartupFailed { reason } => {
            assert!(reason.contains("Timeout"));
        }
        _ => panic!("Expected StartupFailed state, got {state:?}"),
    }
}

#[tokio::test]
async fn test_mark_startup_failed_only_from_loading() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    // Transition to Ready first
    manager.transition_to_ready().await.unwrap();
    assert_eq!(manager.agent_state().await, AgentState::Ready);

    // Try to mark as failed - should be ignored since not in Loading state
    manager
        .mark_startup_failed("This should be ignored".to_owned())
        .await;

    // State should still be Ready
    assert_eq!(manager.agent_state().await, AgentState::Ready);
}

#[tokio::test]
async fn test_transition_to_shutting_down() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    // Transition through states
    manager.transition_to_ready().await.unwrap();
    manager.transition_to_shutting_down().await;

    assert_eq!(manager.agent_state().await, AgentState::ShuttingDown);
}

#[tokio::test]
async fn test_full_state_machine_lifecycle() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Step 1: Start in Loading
    assert_eq!(manager.agent_state().await, AgentState::Loading);

    // Step 2: Configure collectors
    let collectors_config = CollectorsConfig {
        collectors: vec![CollectorEntry::new(
            "test-collector",
            "type-a",
            "/usr/bin/collector",
        )],
    };
    manager.set_collectors_config(collectors_config).await;
    assert!(!manager.all_collectors_ready().await);

    // Step 3: Mark collector as ready
    manager.mark_collector_ready("test-collector").await;
    assert!(manager.all_collectors_ready().await);

    // Step 4: Transition to Ready
    manager.transition_to_ready().await.unwrap();
    assert_eq!(manager.agent_state().await, AgentState::Ready);

    // Step 5: Shutdown
    manager.transition_to_shutting_down().await;
    assert_eq!(manager.agent_state().await, AgentState::ShuttingDown);
}

#[tokio::test]
async fn test_registration_marks_collector_ready() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Initialize registry
    {
        let mut guard = manager.collector_registry.write().await;
        *guard = Some(Arc::new(CollectorRegistry::default()));
    }

    // Configure expected collectors
    let collectors_config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("collector-a", "type-a", "/usr/bin/collector-a"),
            CollectorEntry::new("collector-b", "type-b", "/usr/bin/collector-b"),
        ],
    };
    manager.set_collectors_config(collectors_config).await;

    // Initially not all ready
    assert!(!manager.all_collectors_ready().await);
    assert_eq!(manager.pending_collectors().await.len(), 2);

    // Register first collector - should mark it as ready
    let request_a = RegistrationRequest {
        collector_id: "collector-a".to_owned(),
        collector_type: "type-a".to_owned(),
        hostname: "localhost".to_owned(),
        version: Some("1.0.0".to_owned()),
        pid: Some(1234),
        capabilities: vec![],
        attributes: std::collections::HashMap::new(),
        heartbeat_interval_ms: None,
    };

    let response = manager
        .register_collector(request_a)
        .await
        .expect("registration succeeds");
    assert!(response.accepted);

    // First collector should be marked ready
    assert_eq!(manager.pending_collectors().await.len(), 1);
    assert!(!manager.all_collectors_ready().await);

    // Register second collector - should mark it as ready and complete readiness
    let request_b = RegistrationRequest {
        collector_id: "collector-b".to_owned(),
        collector_type: "type-b".to_owned(),
        hostname: "localhost".to_owned(),
        version: Some("1.0.0".to_owned()),
        pid: Some(5678),
        capabilities: vec![],
        attributes: std::collections::HashMap::new(),
        heartbeat_interval_ms: None,
    };

    let response = manager
        .register_collector(request_b)
        .await
        .expect("registration succeeds");
    assert!(response.accepted);

    // Both collectors should now be ready
    assert!(manager.pending_collectors().await.is_empty());
    assert!(manager.all_collectors_ready().await);

    // Should be able to transition to Ready state now
    manager
        .transition_to_ready()
        .await
        .expect("transition succeeds");
    assert_eq!(manager.agent_state().await, AgentState::Ready);
}

#[tokio::test]
async fn test_wait_for_collectors_ready_success() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Initialize registry
    {
        let mut guard = manager.collector_registry.write().await;
        *guard = Some(Arc::new(CollectorRegistry::default()));
    }

    // Configure with a single collector
    let collectors_config = CollectorsConfig {
        collectors: vec![CollectorEntry::new(
            "test-collector",
            "type-a",
            "/usr/bin/collector",
        )],
    };
    manager.set_collectors_config(collectors_config).await;

    // Spawn a task that will register the collector after a short delay
    let manager_clone = Arc::new(manager);
    let manager_for_task = Arc::clone(&manager_clone);

    let registration_task = tokio::spawn(async move {
        // Wait a bit then register
        tokio::time::sleep(Duration::from_millis(50)).await;

        let request = RegistrationRequest {
            collector_id: "test-collector".to_owned(),
            collector_type: "type-a".to_owned(),
            hostname: "localhost".to_owned(),
            version: None,
            pid: None,
            capabilities: vec![],
            attributes: std::collections::HashMap::new(),
            heartbeat_interval_ms: None,
        };

        manager_for_task
            .register_collector(request)
            .await
            .expect("registration succeeds");
    });

    // Wait for collectors with a generous timeout
    let result = manager_clone
        .wait_for_collectors_ready(Duration::from_secs(5), Duration::from_millis(10))
        .await
        .expect("wait succeeds");

    assert!(result, "All collectors should be ready");
    assert!(manager_clone.all_collectors_ready().await);

    // Clean up
    registration_task.await.unwrap();
}

#[tokio::test]
async fn test_wait_for_collectors_ready_timeout() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // Configure with a collector that will never register
    let collectors_config = CollectorsConfig {
        collectors: vec![CollectorEntry::new(
            "never-registers",
            "type-a",
            "/usr/bin/collector",
        )],
    };
    manager.set_collectors_config(collectors_config).await;

    // Wait with a short timeout
    let result = manager
        .wait_for_collectors_ready(Duration::from_millis(100), Duration::from_millis(20))
        .await
        .expect("wait succeeds but returns false");

    assert!(!result, "Should timeout and return false");

    // Agent state should be StartupFailed
    let state = manager.agent_state().await;
    match state {
        AgentState::StartupFailed { reason } => {
            assert!(reason.contains("never-registers"));
            assert!(reason.contains("Timeout"));
        }
        _ => panic!("Expected StartupFailed state, got {state:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_collectors_ready_no_collectors() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // No collectors configured - should succeed immediately
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    let result = manager
        .wait_for_collectors_ready(Duration::from_secs(1), Duration::from_millis(10))
        .await
        .expect("wait succeeds");

    assert!(result, "Should succeed immediately with no collectors");
}

#[tokio::test]
async fn test_get_startup_timeout() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // No collectors - default timeout
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;
    assert_eq!(manager.get_startup_timeout().await, Duration::from_mins(1));

    // With collectors, use max timeout
    let collectors_config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("collector-1", "type-a", "/usr/bin/collector1")
                .with_startup_timeout(30),
            CollectorEntry::new("collector-2", "type-b", "/usr/bin/collector2")
                .with_startup_timeout(90),
            CollectorEntry::new("collector-3", "type-c", "/usr/bin/collector3")
                .with_startup_timeout(45)
                .with_enabled(false), // Disabled - should be ignored
        ],
    };
    manager.set_collectors_config(collectors_config).await;

    // Should return 90 (max of enabled collectors)
    assert_eq!(manager.get_startup_timeout().await, Duration::from_secs(90));
}

#[tokio::test]
async fn test_drop_privileges_stub() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // The stub should succeed without doing anything
    let result = manager.drop_privileges().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_broadcast_begin_monitoring_errors_when_broker_unavailable() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    // No broker was started, so the broadcast must fail rather than silently
    // succeed. A silent success would falsely signal that collectors received
    // the "begin monitoring" command.
    let result = manager.broadcast_begin_monitoring().await;
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("broker not available"));
}

#[test]
fn test_mark_ready_ignores_unexpected_collector() {
    use crate::broker_manager::state::CollectorReadinessTracker;
    use std::collections::HashSet;

    let expected: HashSet<String> = std::iter::once("collector-1".to_owned()).collect();
    let mut tracker = CollectorReadinessTracker::new(expected);

    // Marking an unexpected collector must not grow the ready set.
    tracker.mark_ready("unexpected-collector");
    assert_eq!(tracker.ready_count(), 0);
    assert!(!tracker.all_ready());

    // Marking the expected collector works as normal.
    tracker.mark_ready("collector-1");
    assert_eq!(tracker.ready_count(), 1);
    assert!(tracker.all_ready());
}

#[tokio::test]
async fn test_transition_to_steady_state_rolls_back_when_broker_unavailable() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);
    manager
        .set_collectors_config(CollectorsConfig::default())
        .await;

    // Reach Ready (no collectors expected, so this is immediate).
    manager.transition_to_ready().await.unwrap();
    assert_eq!(manager.agent_state().await, AgentState::Ready);

    // The broker was never started, so broadcasting "begin monitoring" must
    // fail and the state machine must roll back from SteadyState to Ready.
    let result = manager.transition_to_steady_state().await;
    assert!(result.is_err());
    assert_eq!(manager.agent_state().await, AgentState::Ready);
}
