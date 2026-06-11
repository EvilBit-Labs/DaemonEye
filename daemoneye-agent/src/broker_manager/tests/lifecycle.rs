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
