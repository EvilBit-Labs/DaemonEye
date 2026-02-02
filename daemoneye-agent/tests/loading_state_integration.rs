//! Integration tests for agent loading state coordination.
//!
//! These tests validate the complete loading state machine workflow including:
//! - State transitions (Loading → Ready → SteadyState)
//! - Collector configuration loading
//! - Collector readiness tracking
//! - Startup timeout handling
//! - Begin monitoring broadcast

#![allow(
    clippy::str_to_string,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::let_underscore_must_use,
    clippy::print_stdout
)]

use daemoneye_agent::broker_manager::{AgentState, BrokerManager};
use daemoneye_agent::collector_config::{CollectorEntry, CollectorsConfig};
use daemoneye_eventbus::rpc::{RegistrationProvider, RegistrationRequest};
use daemoneye_lib::config::BrokerConfig;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Setup test broker with loading state enabled
async fn setup_test_broker() -> anyhow::Result<(TempDir, BrokerManager)> {
    let temp_dir = TempDir::new()?;
    let socket_path = temp_dir.path().join("test-broker.sock");

    let config = BrokerConfig {
        enabled: true,
        socket_path: socket_path.to_string_lossy().to_string(),
        max_connections: 100,
        shutdown_timeout_seconds: 10,
        startup_timeout_seconds: 10,
        message_buffer_size: 1000,
        topic_hierarchy: daemoneye_lib::config::TopicHierarchyConfig::default(),
        collector_binaries: std::collections::HashMap::new(),
        config_directory: temp_dir.path().join("configs"),
        process_manager: daemoneye_lib::config::ProcessManagerConfig::default(),
    };

    let manager = BrokerManager::new(config);
    manager.start().await?;

    // Wait for broker to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((temp_dir, manager))
}

/// Create a test collectors configuration
fn create_test_collectors_config(collector_ids: &[&str]) -> CollectorsConfig {
    let collectors = collector_ids
        .iter()
        .map(|id| {
            CollectorEntry::new(id.to_string(), "process", "/usr/bin/test-collector")
                .with_startup_timeout(5)
        })
        .collect();

    CollectorsConfig { collectors }
}

/// Create a registration request for testing
fn create_registration_request(collector_id: &str) -> RegistrationRequest {
    RegistrationRequest {
        collector_id: collector_id.to_string(),
        collector_type: "process".to_string(),
        hostname: "test-host".to_string(),
        version: Some("1.0.0".to_string()),
        pid: Some(12345),
        capabilities: vec!["enumerate".to_string()],
        attributes: HashMap::new(),
        heartbeat_interval_ms: Some(30000),
    }
}

#[tokio::test]
async fn test_agent_starts_in_loading_state() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Verify agent starts in Loading state
    let state = manager.agent_state().await;
    assert!(
        matches!(state, AgentState::Loading),
        "Agent should start in Loading state, got {:?}",
        state
    );

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_set_collectors_config_sets_expected() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set collectors configuration with 2 collectors
    let config = create_test_collectors_config(&["collector-a", "collector-b"]);
    manager.set_collectors_config(config).await;

    // Verify we're still in Loading state (no collectors have registered yet)
    let state = manager.agent_state().await;
    assert!(matches!(state, AgentState::Loading));

    // Transition to Ready should fail because collectors haven't registered
    let result = manager.transition_to_ready().await;
    assert!(
        result.is_err(),
        "Should not be able to transition to Ready with pending collectors"
    );

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_registration_marks_collector_ready() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set up expected collectors
    let config = create_test_collectors_config(&["collector-a"]);
    manager.set_collectors_config(config).await;

    // Simulate collector registration
    let request = create_registration_request("collector-a");
    let response = manager.register_collector(request).await?;
    assert!(response.accepted, "Registration should be accepted");

    // Now transition to Ready should succeed
    let result = manager.transition_to_ready().await;
    assert!(
        result.is_ok(),
        "Should be able to transition to Ready after all collectors registered"
    );

    let state = manager.agent_state().await;
    assert!(matches!(state, AgentState::Ready));

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_multiple_collectors_registration_and_ready() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set up expected collectors
    let config = create_test_collectors_config(&["collector-a", "collector-b", "collector-c"]);
    manager.set_collectors_config(config).await;

    // Register collectors one by one
    for id in &["collector-a", "collector-b"] {
        let request = create_registration_request(id);
        let _ = manager.register_collector(request).await?;
    }

    // Transition should still fail (collector-c not registered)
    assert!(manager.transition_to_ready().await.is_err());

    // Register the last collector
    let request = create_registration_request("collector-c");
    let _ = manager.register_collector(request).await?;

    // Now transition should succeed
    assert!(manager.transition_to_ready().await.is_ok());
    assert!(matches!(manager.agent_state().await, AgentState::Ready));

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_wait_for_collectors_ready_immediate_with_no_collectors() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Empty config means no collectors expected
    let config = CollectorsConfig::default();
    manager.set_collectors_config(config).await;

    // Wait should return immediately with true (all ready)
    let result = manager
        .wait_for_collectors_ready(Duration::from_secs(1), Duration::from_millis(100))
        .await?;

    assert!(result, "Should return true when no collectors expected");

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_wait_for_collectors_ready_timeout() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set up expected collector that won't register
    let config = create_test_collectors_config(&["never-registers"]);
    manager.set_collectors_config(config).await;

    // Wait should timeout and return false
    let result = manager
        .wait_for_collectors_ready(Duration::from_millis(200), Duration::from_millis(50))
        .await?;

    assert!(!result, "Should return false on timeout");

    // State should be StartupFailed
    let state = manager.agent_state().await;
    assert!(
        matches!(state, AgentState::StartupFailed { .. }),
        "Agent should be in StartupFailed state after timeout, got {:?}",
        state
    );

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_wait_for_collectors_ready_success_before_timeout() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set up expected collector
    let config = create_test_collectors_config(&["quick-collector"]);
    manager.set_collectors_config(config).await;

    // Wrap manager in Arc for sharing with spawned task
    let manager_arc = Arc::new(manager);
    let manager_clone = Arc::clone(&manager_arc);

    // Spawn a task to register collector after a short delay
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let request = create_registration_request("quick-collector");
        let _ = manager_clone.register_collector(request).await;
    });

    // Wait should succeed before timeout
    let result = manager_arc
        .wait_for_collectors_ready(Duration::from_secs(2), Duration::from_millis(25))
        .await?;

    assert!(
        result,
        "Should return true when collector registers in time"
    );

    // State should still be Loading (wait doesn't transition)
    let state = manager_arc.agent_state().await;
    assert!(
        matches!(state, AgentState::Loading),
        "State should still be Loading after wait, got {:?}",
        state
    );

    // Shutdown can be called on Arc<BrokerManager> since it takes &self
    manager_arc.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_full_loading_state_lifecycle() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // 1. Start in Loading state
    assert!(matches!(manager.agent_state().await, AgentState::Loading));

    // 2. Set collectors config
    let config = create_test_collectors_config(&["procmond"]);
    manager.set_collectors_config(config).await;

    // 3. Register collector
    let request = create_registration_request("procmond");
    let _ = manager.register_collector(request).await?;

    // 4. Transition to Ready
    manager.transition_to_ready().await?;
    assert!(matches!(manager.agent_state().await, AgentState::Ready));

    // 5. Drop privileges (stub)
    manager.drop_privileges().await?;

    // 6. Broadcast begin monitoring
    manager.broadcast_begin_monitoring().await?;

    // 7. Transition to SteadyState
    manager.transition_to_steady_state().await?;
    assert!(matches!(
        manager.agent_state().await,
        AgentState::SteadyState
    ));

    // 8. Transition to ShuttingDown for graceful shutdown
    manager.transition_to_shutting_down().await;
    assert!(matches!(
        manager.agent_state().await,
        AgentState::ShuttingDown
    ));

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_startup_timeout_from_config() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set collectors config with specific timeout
    let config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("collector-a", "process", "/usr/bin/test").with_startup_timeout(30),
            CollectorEntry::new("collector-b", "network", "/usr/bin/test").with_startup_timeout(45),
        ],
    };
    manager.set_collectors_config(config).await;

    // Get startup timeout should return the max (45 seconds)
    let timeout = manager.get_startup_timeout().await;
    assert_eq!(timeout.as_secs(), 45);

    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn test_disabled_collectors_not_expected() -> anyhow::Result<()> {
    let (_temp_dir, manager) = setup_test_broker().await?;

    // Set collectors config with one enabled and one disabled
    let config = CollectorsConfig {
        collectors: vec![
            CollectorEntry::new("enabled-collector", "process", "/usr/bin/test")
                .with_startup_timeout(30),
            CollectorEntry::new("disabled-collector", "network", "/usr/bin/test")
                .with_enabled(false)
                .with_startup_timeout(30),
        ],
    };
    manager.set_collectors_config(config).await;

    // Only register the enabled collector
    let request = create_registration_request("enabled-collector");
    let _ = manager.register_collector(request).await?;

    // Should be able to transition to Ready (disabled collector not expected)
    manager.transition_to_ready().await?;
    assert!(matches!(manager.agent_state().await, AgentState::Ready));

    manager.shutdown().await?;
    Ok(())
}
