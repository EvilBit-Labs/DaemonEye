//! Tests for broker lifecycle, health, and registration provider behavior.

use super::sample_registration_request;
use crate::broker_manager::{BrokerHealth, BrokerManager};
use crate::collector_registry::CollectorRegistry;
use daemoneye_eventbus::rpc::{DeregistrationRequest, RegistrationProvider};
use daemoneye_lib::config::BrokerConfig;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_broker_manager_creation() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let health = manager.health_status().await;
    assert_eq!(health, BrokerHealth::Stopped);
    assert!(!manager.is_running().await);
}

#[tokio::test]
async fn test_broker_manager_disabled() {
    let config = BrokerConfig {
        enabled: false,
        ..Default::default()
    };

    let manager = BrokerManager::new(config);
    let result = manager.start().await;

    assert!(result.is_ok());
    let health = manager.health_status().await;
    assert_eq!(health, BrokerHealth::Stopped);
}

#[tokio::test]
async fn test_broker_manager_socket_path() {
    let config = BrokerConfig {
        socket_path: "/tmp/test-broker.sock".to_owned(),
        ..Default::default()
    };

    let manager = BrokerManager::new(config);
    assert_eq!(manager.socket_path(), "/tmp/test-broker.sock");
}

#[tokio::test]
async fn test_broker_manager_statistics_when_stopped() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let stats = manager.statistics().await;
    assert!(stats.is_none());
}

#[tokio::test]
async fn test_registration_provider_delegates_to_registry() {
    let manager = BrokerManager::new(BrokerConfig::default());

    {
        let mut guard = manager.collector_registry.write().await;
        *guard = Some(Arc::new(CollectorRegistry::default()))
    };

    let request = sample_registration_request();
    let response = manager
        .register_collector(request.clone())
        .await
        .expect("registration succeeds");
    assert!(response.accepted);
    assert_eq!(response.collector_id, request.collector_id);

    manager
        .update_heartbeat(&request.collector_id)
        .await
        .expect("heartbeat updated");

    manager
        .deregister_collector(DeregistrationRequest {
            collector_id: request.collector_id,
            reason: Some("test".to_owned()),
            force: false,
        })
        .await
        .expect("deregistration succeeds");
}

#[tokio::test]
async fn test_broker_manager_health_check_when_stopped() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let health = manager.health_check().await;
    assert_eq!(health, BrokerHealth::Stopped);
}

#[tokio::test]
async fn test_broker_manager_wait_for_healthy_timeout() {
    let config = BrokerConfig::default();
    let manager = BrokerManager::new(config);

    let result = manager.wait_for_healthy(Duration::from_millis(100)).await;
    assert!(result.is_err());
}

/// Start a `BrokerManager` backed by a real broker on a tempdir socket.
///
/// Returns the started manager and the `TempDir` guard (which must be kept alive
/// for the lifetime of the manager so the socket/config paths remain valid).
async fn started_manager() -> (BrokerManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("broker.sock");
    let config = BrokerConfig {
        socket_path: socket_path.to_string_lossy().into_owned(),
        config_directory: dir.path().to_path_buf(),
        ..Default::default()
    };
    let manager = BrokerManager::new(config);
    manager.start().await.expect("broker starts");
    (manager, dir)
}

#[tokio::test]
async fn test_create_rpc_client_is_deduplicated() {
    let (manager, _dir) = started_manager().await;

    // Two creations for the same collector_id must converge on a single shared
    // client; the second must not overwrite (and leak) the first.
    let first = manager
        .create_rpc_client("collector-x")
        .await
        .expect("first client created");
    let second = manager
        .create_rpc_client("collector-x")
        .await
        .expect("second client reuses existing");

    assert!(
        Arc::ptr_eq(&first, &second),
        "concurrent creations must return the same client"
    );
    assert_eq!(
        manager.list_registered_collector_ids().await.len(),
        1,
        "only one client should be stored for the collector"
    );

    manager.shutdown().await.expect("broker shuts down");
}

#[tokio::test]
async fn test_health_check_recovers_from_unhealthy() {
    let (manager, _dir) = started_manager().await;

    // A live broker with no collectors should report healthy.
    assert_eq!(manager.health_check().await, BrokerHealth::Healthy);

    // Simulate a transient failure that cached an Unhealthy status.
    *manager.health_status.write().await = BrokerHealth::Unhealthy("transient failure".to_owned());

    // The next health check must re-probe and recover to Healthy rather than
    // staying stuck on the cached Unhealthy state forever.
    assert_eq!(manager.health_check().await, BrokerHealth::Healthy);
    assert_eq!(manager.health_status().await, BrokerHealth::Healthy);

    manager.shutdown().await.expect("broker shuts down");
}
