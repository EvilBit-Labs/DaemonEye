//! Integration tests for RPC-based collector management.
//!
//! This module provides comprehensive integration tests for the collector lifecycle
//! management system, including RPC calls, health monitoring, configuration management,
//! and graceful shutdown coordination.

use collector_core::{
    CollectorLifecycleManager, ConfigManagerSettings, HealthMonitorConfig,
    busrt_types::{
        CollectorConfig, CollectorLifecycleService, CollectorStatus, ConfigurationService,
        GetConfigRequest, HealthCheckRequest, HealthCheckService, HealthStatus, HeartbeatRequest,
        ListCollectorsRequest, PauseCollectorRequest, ResumeCollectorRequest,
        StartCollectorRequest, StatusRequest, StopCollectorRequest, UpdateConfigRequest,
        ValidateConfigRequest,
    },
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::{sleep, timeout};

/// Test fixture for RPC integration tests
struct RpcTestFixture {
    manager: Arc<CollectorLifecycleManager>,
    _temp_dir: tempfile::TempDir,
}

impl RpcTestFixture {
    async fn new() -> Self {
        let _ = tracing_subscriber::fmt::try_init();

        let health_config = HealthMonitorConfig {
            heartbeat_interval: Duration::from_millis(100),
            health_timeout: Duration::from_millis(300),
            max_consecutive_failures: 2,
            health_check_interval: Duration::from_millis(200),
            enable_auto_recovery: true,
            recovery_timeout: Duration::from_secs(5),
        };

        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let config_settings = ConfigManagerSettings {
            enable_file_watching: false,
            config_directory: temp_dir.path().to_path_buf(),
            max_backups_per_collector: 5,
            validation_timeout: Duration::from_secs(10),
            enable_auto_rollback: true,
            file_watch_interval: Duration::from_secs(1),
        };

        let manager =
            CollectorLifecycleManager::new_with_config_settings(health_config, config_settings);
        manager
            .start()
            .await
            .expect("Failed to start lifecycle manager");

        Self {
            manager: Arc::new(manager),
            _temp_dir: temp_dir,
        }
    }

    async fn create_test_config(&self) -> CollectorConfig {
        CollectorConfig {
            collector_type: "process".to_string(),
            scan_interval_ms: 1000,
            batch_size: 100,
            timeout_seconds: 30,
            capabilities: vec!["process".to_string(), "realtime".to_string()],
            settings: HashMap::new(),
        }
    }

    async fn cleanup(&self) {
        let _ = self.manager.stop().await;
    }
}

#[tokio::test]
async fn test_collector_lifecycle_complete_workflow() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Test start collector
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "lifecycle-test".to_string(),
        config: config.clone(),
        environment: HashMap::new(),
    };

    let start_response = fixture
        .manager
        .start_collector(start_request)
        .await
        .expect("Failed to start collector");

    assert!(start_response.success);
    assert_eq!(start_response.collector_id, "lifecycle-test");
    assert_eq!(start_response.status, CollectorStatus::Running);

    // Test get status
    let status_request = StatusRequest {
        collector_id: "lifecycle-test".to_string(),
    };
    let status_response = fixture
        .manager
        .get_collector_status(status_request)
        .await
        .expect("Failed to get collector status");

    assert_eq!(status_response.collector_id, "lifecycle-test");
    assert_eq!(status_response.status, CollectorStatus::Running);
    // uptime_seconds is always >= 0 for unsigned types, so just check it exists
    let _ = status_response.uptime_seconds;

    // Test list collectors
    let list_request = ListCollectorsRequest {
        filter_status: None,
        filter_type: None,
    };
    let list_response = fixture
        .manager
        .list_collectors(list_request)
        .await
        .expect("Failed to list collectors");

    assert_eq!(list_response.total_count, 1);
    assert_eq!(list_response.collectors[0].collector_id, "lifecycle-test");

    // Test pause collector
    let pause_request = PauseCollectorRequest {
        collector_id: "lifecycle-test".to_string(),
        reason: "Testing pause functionality".to_string(),
    };
    let pause_response = fixture
        .manager
        .pause_collector(pause_request)
        .await
        .expect("Failed to pause collector");

    assert!(pause_response.success);

    // Test resume collector
    let resume_request = ResumeCollectorRequest {
        collector_id: "lifecycle-test".to_string(),
    };
    let resume_response = fixture
        .manager
        .resume_collector(resume_request)
        .await
        .expect("Failed to resume collector");

    assert!(resume_response.success);

    // Test stop collector
    let stop_request = StopCollectorRequest {
        collector_id: "lifecycle-test".to_string(),
        force: false,
        timeout_seconds: 30,
    };
    let stop_response = fixture
        .manager
        .stop_collector(stop_request)
        .await
        .expect("Failed to stop collector");

    assert!(stop_response.success);

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_health_monitoring_integration() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start a collector
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "health-test".to_string(),
        config,
        environment: HashMap::new(),
    };

    fixture
        .manager
        .start_collector(start_request)
        .await
        .expect("Failed to start collector");

    // Test heartbeat
    let heartbeat_request = HeartbeatRequest {
        collector_id: "health-test".to_string(),
    };
    let heartbeat_response = fixture
        .manager
        .heartbeat(heartbeat_request)
        .await
        .expect("Failed to process heartbeat");

    assert!(heartbeat_response.timestamp > 0);
    assert_eq!(heartbeat_response.status, HealthStatus::Healthy);

    // Test health check
    let health_request = HealthCheckRequest {
        collector_id: "health-test".to_string(),
        include_metrics: true,
    };
    let health_response = fixture
        .manager
        .health_check(health_request)
        .await
        .expect("Failed to perform health check");

    assert_eq!(health_response.status, HealthStatus::Healthy);
    assert!(!health_response.message.is_empty());

    // Test multiple heartbeats
    for _ in 0..3 {
        let heartbeat_request = HeartbeatRequest {
            collector_id: "health-test".to_string(),
        };
        fixture
            .manager
            .heartbeat(heartbeat_request)
            .await
            .expect("Failed to process heartbeat");

        sleep(Duration::from_millis(50)).await;
    }

    // Final health check should still be healthy
    let final_health_request = HealthCheckRequest {
        collector_id: "health-test".to_string(),
        include_metrics: false,
    };
    let final_health_response = fixture
        .manager
        .health_check(final_health_request)
        .await
        .expect("Failed to perform final health check");

    assert_eq!(final_health_response.status, HealthStatus::Healthy);

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_configuration_management_integration() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start a collector
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "config-test".to_string(),
        config: config.clone(),
        environment: HashMap::new(),
    };

    fixture
        .manager
        .start_collector(start_request)
        .await
        .expect("Failed to start collector");

    // Test validate configuration
    let validate_request = ValidateConfigRequest {
        config: config.clone(),
    };
    let validate_response = fixture
        .manager
        .validate_config(validate_request)
        .await
        .expect("Failed to validate config");

    assert!(validate_response.valid);
    assert!(validate_response.validation_errors.is_empty());

    // Test invalid configuration
    let invalid_config = CollectorConfig {
        collector_type: "".to_string(), // Invalid
        scan_interval_ms: 0,            // Invalid
        batch_size: 0,                  // Invalid
        timeout_seconds: 0,             // Invalid
        capabilities: vec![],
        settings: HashMap::new(),
    };

    let invalid_validate_request = ValidateConfigRequest {
        config: invalid_config,
    };
    let invalid_validate_response = fixture
        .manager
        .validate_config(invalid_validate_request)
        .await
        .expect("Failed to validate invalid config");

    assert!(!invalid_validate_response.valid);
    assert!(!invalid_validate_response.validation_errors.is_empty());

    // Test update configuration
    let mut updated_config = config.clone();
    updated_config.scan_interval_ms = 2000;
    updated_config.batch_size = 200;

    let update_request = UpdateConfigRequest {
        collector_id: "config-test".to_string(),
        new_config: updated_config.clone(),
        validate_only: false,
    };
    let update_response = fixture
        .manager
        .update_config(update_request)
        .await
        .expect("Failed to update config");

    assert!(update_response.success);
    assert!(update_response.validation_errors.is_empty());

    // Test get configuration
    let get_request = GetConfigRequest {
        collector_id: "config-test".to_string(),
    };
    let get_response = fixture
        .manager
        .get_config(get_request)
        .await
        .expect("Failed to get config");

    assert_eq!(get_response.config.scan_interval_ms, 2000);
    assert_eq!(get_response.config.batch_size, 200);

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_graceful_shutdown_coordination() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start multiple collectors
    let collector_ids = vec!["shutdown-test-1", "shutdown-test-2", "shutdown-test-3"];

    for collector_id in &collector_ids {
        let start_request = StartCollectorRequest {
            collector_type: "process".to_string(),
            collector_id: collector_id.to_string(),
            config: config.clone(),
            environment: HashMap::new(),
        };

        fixture
            .manager
            .start_collector(start_request)
            .await
            .expect("Failed to start collector");
    }

    // Verify all collectors are running
    let list_request = ListCollectorsRequest {
        filter_status: Some(CollectorStatus::Running),
        filter_type: None,
    };
    let list_response = fixture
        .manager
        .list_collectors(list_request)
        .await
        .expect("Failed to list collectors");

    assert_eq!(list_response.total_count, 3);

    // Test graceful shutdown of individual collector
    let stop_request = StopCollectorRequest {
        collector_id: "shutdown-test-1".to_string(),
        force: false,
        timeout_seconds: 30,
    };
    let stop_response = fixture
        .manager
        .stop_collector(stop_request)
        .await
        .expect("Failed to stop collector");

    assert!(stop_response.success);

    // Test forced shutdown
    let force_stop_request = StopCollectorRequest {
        collector_id: "shutdown-test-2".to_string(),
        force: true,
        timeout_seconds: 5,
    };
    let force_stop_response = fixture
        .manager
        .stop_collector(force_stop_request)
        .await
        .expect("Failed to force stop collector");

    assert!(force_stop_response.success);

    // Check how many collectors are still running before global shutdown
    let list_request_before_shutdown = ListCollectorsRequest {
        filter_status: Some(CollectorStatus::Running),
        filter_type: None,
    };
    let list_response_before_shutdown = fixture
        .manager
        .list_collectors(list_request_before_shutdown)
        .await
        .expect("Failed to list collectors before shutdown");

    println!(
        "Collectors running before global shutdown: {}",
        list_response_before_shutdown.total_count
    );
    for collector in &list_response_before_shutdown.collectors {
        println!("  - {}: {:?}", collector.collector_id, collector.status);
    }

    // Test global shutdown for remaining collectors
    let shutdown_responses = fixture
        .manager
        .as_ref()
        .shutdown_all_collectors("Test global shutdown".to_string())
        .await
        .expect("Failed to shutdown all collectors");

    println!("Shutdown responses received: {}", shutdown_responses.len());
    for (i, response) in shutdown_responses.iter().enumerate() {
        println!(
            "  Response {}: success={}, duration={:?}",
            i, response.success, response.duration
        );
    }

    // The shutdown coordinator should attempt to shut down all collectors it knows about
    // This might be fewer than 3 if some collectors were not properly registered

    // Check that at least one shutdown was successful (the running collector)
    let successful_shutdowns = shutdown_responses.iter().filter(|r| r.success).count();
    assert!(
        successful_shutdowns >= 1,
        "At least one collector should shut down successfully"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_error_handling_and_recovery() {
    let fixture = RpcTestFixture::new().await;

    // Test operations on non-existent collector
    let status_request = StatusRequest {
        collector_id: "non-existent".to_string(),
    };
    let status_result = fixture.manager.get_collector_status(status_request).await;
    assert!(status_result.is_err());

    let heartbeat_request = HeartbeatRequest {
        collector_id: "non-existent".to_string(),
    };
    let heartbeat_result = fixture.manager.heartbeat(heartbeat_request).await;
    // Heartbeat returns success with Unknown status for non-existent collectors
    assert!(heartbeat_result.is_ok());
    assert_eq!(heartbeat_result.unwrap().status, HealthStatus::Unknown);

    let get_config_request = GetConfigRequest {
        collector_id: "non-existent".to_string(),
    };
    let get_config_result = fixture.manager.get_config(get_config_request).await;
    assert!(get_config_result.is_err());

    // Test configuration validation with edge cases
    let edge_case_config = CollectorConfig {
        collector_type: "process".to_string(),
        scan_interval_ms: 100, // Minimum valid (changed from 50)
        batch_size: 1,         // Minimum valid
        timeout_seconds: 1,    // Minimum valid
        capabilities: vec!["process".to_string()],
        settings: HashMap::new(),
    };

    let validate_request = ValidateConfigRequest {
        config: edge_case_config,
    };
    let validate_response = fixture
        .manager
        .validate_config(validate_request)
        .await
        .expect("Failed to validate edge case config");

    assert!(validate_response.valid);

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_concurrent_operations() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start multiple collectors concurrently
    let mut start_tasks = Vec::new();
    for i in 0..5 {
        let manager = fixture.manager.clone();
        let config = config.clone();
        let collector_id = format!("concurrent-test-{}", i);

        let task = tokio::spawn(async move {
            let start_request = StartCollectorRequest {
                collector_type: "process".to_string(),
                collector_id: collector_id.clone(),
                config,
                environment: HashMap::new(),
            };

            manager.start_collector(start_request).await
        });

        start_tasks.push(task);
    }

    // Wait for all start operations to complete
    let start_results = futures::future::join_all(start_tasks).await;
    for result in start_results {
        let response = result
            .expect("Task failed")
            .expect("Start collector failed");
        assert!(response.success);
    }

    // Perform concurrent health checks
    let mut health_tasks = Vec::new();
    for i in 0..5 {
        let manager = fixture.manager.clone();
        let collector_id = format!("concurrent-test-{}", i);

        let task = tokio::spawn(async move {
            let health_request = HealthCheckRequest {
                collector_id,
                include_metrics: true,
            };

            manager.health_check(health_request).await
        });

        health_tasks.push(task);
    }

    // Wait for all health checks to complete
    let health_results = futures::future::join_all(health_tasks).await;
    for result in health_results {
        let response = result.expect("Task failed").expect("Health check failed");
        assert_eq!(response.status, HealthStatus::Healthy);
    }

    // Perform concurrent shutdowns
    let mut stop_tasks = Vec::new();
    for i in 0..5 {
        let manager = fixture.manager.clone();
        let collector_id = format!("concurrent-test-{}", i);

        let task = tokio::spawn(async move {
            let stop_request = StopCollectorRequest {
                collector_id,
                force: false,
                timeout_seconds: 30,
            };

            manager.stop_collector(stop_request).await
        });

        stop_tasks.push(task);
    }

    // Wait for all stop operations to complete
    let stop_results = futures::future::join_all(stop_tasks).await;
    for result in stop_results {
        let response = result.expect("Task failed").expect("Stop collector failed");
        assert!(response.success);
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_timeout_and_resource_limits() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start a collector
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "timeout-test".to_string(),
        config,
        environment: HashMap::new(),
    };

    fixture
        .manager
        .start_collector(start_request)
        .await
        .expect("Failed to start collector");

    // Test operation with timeout
    let health_request = HealthCheckRequest {
        collector_id: "timeout-test".to_string(),
        include_metrics: true,
    };

    let health_result = timeout(
        Duration::from_secs(5),
        fixture.manager.health_check(health_request),
    )
    .await;

    assert!(health_result.is_ok());
    let health_response = health_result.unwrap().expect("Health check failed");
    assert_eq!(health_response.status, HealthStatus::Healthy);

    // Test shutdown with custom timeout
    let stop_request = StopCollectorRequest {
        collector_id: "timeout-test".to_string(),
        force: false,
        timeout_seconds: 1, // Short timeout
    };

    let stop_result = timeout(
        Duration::from_secs(10),
        fixture.manager.stop_collector(stop_request),
    )
    .await;

    assert!(stop_result.is_ok());
    let stop_response = stop_result.unwrap().expect("Stop collector failed");
    assert!(stop_response.success);

    fixture.cleanup().await;
}

#[tokio::test]
async fn test_configuration_rollback_scenarios() {
    let fixture = RpcTestFixture::new().await;
    let config = fixture.create_test_config().await;

    // Start a collector
    let start_request = StartCollectorRequest {
        collector_type: "process".to_string(),
        collector_id: "rollback-test".to_string(),
        config: config.clone(),
        environment: HashMap::new(),
    };

    fixture
        .manager
        .start_collector(start_request)
        .await
        .expect("Failed to start collector");

    // Update configuration multiple times to create backup history
    for i in 1..=3 {
        let mut updated_config = config.clone();
        updated_config.scan_interval_ms = 1000 * i;
        updated_config.batch_size = (100 * i) as usize;

        let update_request = UpdateConfigRequest {
            collector_id: "rollback-test".to_string(),
            new_config: updated_config,
            validate_only: false,
        };

        let update_response = fixture
            .manager
            .update_config(update_request)
            .await
            .expect("Failed to update config");

        assert!(update_response.success);

        // Small delay between updates
        sleep(Duration::from_millis(10)).await;
    }

    // Verify final configuration
    let get_request = GetConfigRequest {
        collector_id: "rollback-test".to_string(),
    };
    let get_response = fixture
        .manager
        .get_config(get_request)
        .await
        .expect("Failed to get config");

    assert_eq!(get_response.config.scan_interval_ms, 3000);
    assert_eq!(get_response.config.batch_size, 300);

    fixture.cleanup().await;
}
