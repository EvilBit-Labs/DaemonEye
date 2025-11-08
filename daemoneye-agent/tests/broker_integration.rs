//! Integration tests for embedded broker functionality

use daemoneye_agent::BrokerManager;
use daemoneye_lib::config::BrokerConfig;
use std::time::Duration;
use tempfile::{NamedTempFile, TempDir};

#[tokio::test]
async fn test_broker_lifecycle() {
    // Create temporary paths for socket and config
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let socket_path = temp_file.path().to_string_lossy().to_string();
    let _config_dir = TempDir::new().expect("Failed to create temp directory");

    let config_dir = TempDir::new().expect("Failed to create temp directory");
    let config_directory = config_dir.path().join("collector-configs");
    std::fs::create_dir_all(&config_directory).expect("Failed to create config directory");
    let _config_dir = config_dir;

    // Create broker configuration
    let config = BrokerConfig {
        socket_path,
        startup_timeout_seconds: 5,
        shutdown_timeout_seconds: 5,
        config_directory,
        ..Default::default()
    };

    // Create broker manager
    let broker_manager = BrokerManager::new(config);

    // Test initial state
    assert_eq!(
        broker_manager.health_status().await,
        daemoneye_agent::BrokerHealth::Stopped
    );

    // Start the broker
    let start_result = broker_manager.start().await;
    assert!(
        start_result.is_ok(),
        "Failed to start broker: {:?}",
        start_result
    );

    // Wait for broker to become healthy
    let wait_result = broker_manager
        .wait_for_healthy(Duration::from_secs(10))
        .await;
    assert!(
        wait_result.is_ok(),
        "Broker failed to become healthy: {:?}",
        wait_result
    );

    // Verify broker is healthy
    assert_eq!(
        broker_manager.health_status().await,
        daemoneye_agent::BrokerHealth::Healthy
    );

    // Get statistics
    let stats = broker_manager.statistics().await;
    assert!(
        stats.is_some(),
        "Should have statistics when broker is running"
    );

    let stats = stats.unwrap();
    assert_eq!(stats.messages_published, 0);
    assert_eq!(stats.active_subscribers, 0);

    // Shutdown the broker
    let shutdown_result = broker_manager.shutdown().await;
    assert!(
        shutdown_result.is_ok(),
        "Failed to shutdown broker: {:?}",
        shutdown_result
    );

    // Verify broker is stopped
    assert_eq!(
        broker_manager.health_status().await,
        daemoneye_agent::BrokerHealth::Stopped
    );
}

#[tokio::test]
async fn test_broker_disabled() {
    let config_dir = TempDir::new().expect("Failed to create temp directory");
    let config_directory = config_dir.path().join("collector-configs");
    std::fs::create_dir_all(&config_directory).expect("Failed to create config directory");
    let _config_dir = config_dir;

    let config = BrokerConfig {
        enabled: false,
        config_directory,
        ..Default::default()
    };

    let broker_manager = BrokerManager::new(config);

    // Start should succeed but broker should remain stopped
    let start_result = broker_manager.start().await;
    assert!(start_result.is_ok());

    assert_eq!(
        broker_manager.health_status().await,
        daemoneye_agent::BrokerHealth::Stopped
    );

    // Statistics should be None when disabled
    let stats = broker_manager.statistics().await;
    assert!(stats.is_none());
}

#[tokio::test]
async fn test_broker_health_monitoring() {
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let socket_path = temp_file.path().to_string_lossy().to_string();

    let config_dir = TempDir::new().expect("Failed to create temp directory");
    let config_directory = config_dir.path().join("collector-configs");
    std::fs::create_dir_all(&config_directory).expect("Failed to create config directory");
    let _config_dir = config_dir;

    let config = BrokerConfig {
        socket_path,
        config_directory,
        ..Default::default()
    };

    let broker_manager = BrokerManager::new(config);

    // Health check when stopped
    let health = broker_manager.health_check().await;
    assert_eq!(health, daemoneye_agent::BrokerHealth::Stopped);

    // Start broker
    broker_manager
        .start()
        .await
        .expect("Failed to start broker");
    broker_manager
        .wait_for_healthy(Duration::from_secs(5))
        .await
        .expect("Broker failed to become healthy");

    // Health check when running
    let health = broker_manager.health_check().await;
    assert_eq!(health, daemoneye_agent::BrokerHealth::Healthy);

    // Shutdown
    broker_manager
        .shutdown()
        .await
        .expect("Failed to shutdown broker");

    // Health check after shutdown
    let health = broker_manager.health_check().await;
    assert_eq!(health, daemoneye_agent::BrokerHealth::Stopped);
}
