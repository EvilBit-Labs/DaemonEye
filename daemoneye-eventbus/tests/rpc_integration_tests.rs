//! Integration tests for RPC call patterns
//!
//! These tests validate the RPC patterns for collector lifecycle management,
//! health checks, configuration updates, and graceful shutdown coordination.

use daemoneye_eventbus::{
    broker::DaemoneyeBroker,
    error::{EventBusError, Result},
    rpc::{
        CapabilitiesData, CollectorLifecycleRequest, CollectorOperation, CollectorRpcClient,
        CollectorRpcService, ComponentHealth, ConfigUpdateRequest, DeregistrationRequest,
        HealthCheckData, HealthStatus, RegistrationRequest, ResourceRequirements,
        RpcCorrelationMetadata, RpcPayload, RpcRequest, RpcResponse, RpcStatus,
        ServiceCapabilities, ShutdownRequest, ShutdownType, TimeoutLimits,
    },
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// Test RPC service for integration testing
struct TestRpcService {
    service: CollectorRpcService,
    collector_running: bool,
    config: HashMap<String, serde_json::Value>,
}

impl TestRpcService {
    fn new() -> Self {
        let capabilities = ServiceCapabilities {
            operations: vec![
                CollectorOperation::Register,
                CollectorOperation::Deregister,
                CollectorOperation::Start,
                CollectorOperation::Stop,
                CollectorOperation::Restart,
                CollectorOperation::HealthCheck,
                CollectorOperation::UpdateConfig,
                CollectorOperation::GetCapabilities,
                CollectorOperation::GracefulShutdown,
                CollectorOperation::ForceShutdown,
            ],
            max_concurrent_requests: 10,
            timeout_limits: TimeoutLimits {
                min_timeout_ms: 1000,
                max_timeout_ms: 300000,
                default_timeout_ms: 30000,
            },
            supported_collectors: vec!["test-collector".to_string()],
        };

        // Create dummy process manager for existing tests
        use daemoneye_eventbus::process_manager::{CollectorProcessManager, ProcessManagerConfig};
        let pm_config = ProcessManagerConfig::default();
        let process_manager = CollectorProcessManager::new(pm_config);

        let service =
            CollectorRpcService::new("test-service".to_string(), capabilities, process_manager);

        Self {
            service,
            collector_running: false,
            config: HashMap::new(),
        }
    }

    async fn handle_request(&mut self, request: RpcRequest) -> RpcResponse {
        // Simulate actual collector operations
        match request.operation {
            CollectorOperation::Start => {
                self.collector_running = true;
                self.create_success_response(request, None)
            }
            CollectorOperation::Stop => {
                self.collector_running = false;
                self.create_success_response(request, None)
            }
            CollectorOperation::GracefulShutdown | CollectorOperation::ForceShutdown => {
                // Handle shutdown operations in mock
                self.collector_running = false;
                self.create_success_response(request, None)
            }
            CollectorOperation::HealthCheck => {
                let health_data = self.create_health_data();
                self.create_success_response(request, Some(RpcPayload::HealthCheck(health_data)))
            }
            CollectorOperation::UpdateConfig => {
                if let RpcPayload::ConfigUpdate(ref config_req) = request.payload {
                    for (key, value) in &config_req.config_changes {
                        self.config.insert(key.clone(), value.clone());
                    }
                }
                self.create_success_response(request, None)
            }
            CollectorOperation::GetCapabilities => {
                let capabilities = self.create_capabilities_data();
                self.create_success_response(request, Some(RpcPayload::Capabilities(capabilities)))
            }
            _ => self.service.handle_request(request).await,
        }
    }

    fn create_success_response(
        &self,
        request: RpcRequest,
        payload: Option<RpcPayload>,
    ) -> RpcResponse {
        RpcResponse {
            request_id: request.request_id,
            service_id: self.service.service_id.clone(),
            operation: request.operation,
            status: RpcStatus::Success,
            payload,
            timestamp: SystemTime::now(),
            execution_time_ms: 10,
            queue_time_ms: None,
            total_time_ms: 10,
            error_details: None,
            correlation_metadata: request.correlation_metadata.increment_hop(),
        }
    }

    fn create_health_data(&self) -> HealthCheckData {
        let mut components = HashMap::new();
        components.insert(
            "process_enumeration".to_string(),
            ComponentHealth {
                name: "process_enumeration".to_string(),
                status: if self.collector_running {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                },
                message: Some(format!("Collector running: {}", self.collector_running)),
                last_check: SystemTime::now(),
                check_interval_seconds: 30,
            },
        );

        let mut metrics = HashMap::new();
        metrics.insert("cpu_usage_percent".to_string(), 2.5);
        metrics.insert("memory_usage_mb".to_string(), 45.0);

        HealthCheckData {
            collector_id: "test-collector".to_string(),
            status: if self.collector_running {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            },
            components,
            metrics,
            last_heartbeat: SystemTime::now(),
            uptime_seconds: 3600,
            error_count: 0,
        }
    }

    fn create_capabilities_data(&self) -> CapabilitiesData {
        let mut features = HashMap::new();
        features.insert("hash_verification".to_string(), true);
        features.insert("privilege_dropping".to_string(), true);

        CapabilitiesData {
            collector_id: "test-collector".to_string(),
            event_types: vec!["process".to_string()],
            operations: self.service.supported_operations.clone(),
            resource_requirements: ResourceRequirements {
                min_memory_bytes: 50 * 1024 * 1024,
                min_cpu_percent: 1.0,
                required_privileges: vec!["process_enumeration".to_string()],
                required_capabilities: vec!["CAP_SYS_PTRACE".to_string()],
            },
            platform_support: vec![
                "linux".to_string(),
                "macos".to_string(),
                "windows".to_string(),
            ],
            features,
        }
    }
}

#[tokio::test]
async fn test_collector_start_lifecycle() {
    let mut service = TestRpcService::new();

    // Create start request
    let lifecycle_req = CollectorLifecycleRequest::start("test-collector", None);
    let request = RpcRequest::lifecycle(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        CollectorOperation::Start,
        lifecycle_req,
        Duration::from_secs(30),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::Start);
    assert!(service.collector_running);
}

#[tokio::test]
async fn test_collector_stop_lifecycle() {
    let mut service = TestRpcService::new();
    service.collector_running = true; // Start in running state

    // Create stop request
    let lifecycle_req = CollectorLifecycleRequest::stop("test-collector");
    let request = RpcRequest::lifecycle(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        CollectorOperation::Stop,
        lifecycle_req,
        Duration::from_secs(30),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::Stop);
    assert!(!service.collector_running);
}

#[tokio::test]
async fn test_registration_and_deregistration() {
    let mut service = TestRpcService::new();

    let registration = RegistrationRequest {
        collector_id: "test-collector".to_string(),
        collector_type: "test-collector".to_string(),
        hostname: "localhost".to_string(),
        version: Some("1.0.0".to_string()),
        pid: Some(4242),
        capabilities: vec!["process-monitoring".to_string()],
        attributes: HashMap::new(),
        heartbeat_interval_ms: Some(15_000),
    };

    let register_request = RpcRequest::register(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        registration,
        Duration::from_secs(5),
    );

    let register_response = service.handle_request(register_request).await;
    assert_eq!(register_response.status, RpcStatus::Success);
    assert_eq!(register_response.operation, CollectorOperation::Register);
    if let Some(RpcPayload::RegistrationResponse(resp)) = register_response.payload {
        assert!(resp.accepted);
        assert_eq!(resp.collector_id, "test-collector");
        assert_eq!(resp.heartbeat_interval_ms, 15_000);
    } else {
        panic!("Expected RegistrationResponse payload");
    }

    let deregistration = DeregistrationRequest {
        collector_id: "test-collector".to_string(),
        reason: Some("test completed".to_string()),
        force: false,
    };

    let deregister_request = RpcRequest::deregister(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        deregistration,
        Duration::from_secs(5),
    );

    let deregister_response = service.handle_request(deregister_request).await;
    assert_eq!(deregister_response.status, RpcStatus::Success);
    assert_eq!(
        deregister_response.operation,
        CollectorOperation::Deregister
    );
    if let Some(RpcPayload::Generic(payload)) = deregister_response.payload {
        assert_eq!(
            payload.get("collector_id").and_then(|v| v.as_str()),
            Some("test-collector")
        );
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("deregistered")
        );
    } else {
        panic!("Expected Generic payload for deregistration");
    }
}

#[tokio::test]
async fn test_health_check_request() {
    let mut service = TestRpcService::new();
    service.collector_running = true;

    // Create health check request
    let request = RpcRequest::health_check(
        "test-client".to_string(),
        "control.health.test-collector".to_string(),
        Duration::from_secs(10),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::HealthCheck);

    if let Some(RpcPayload::HealthCheck(health_data)) = response.payload {
        assert_eq!(health_data.collector_id, "test-collector");
        assert_eq!(health_data.status, HealthStatus::Healthy);
        assert!(health_data.components.contains_key("process_enumeration"));
        assert!(health_data.metrics.contains_key("cpu_usage_percent"));
    } else {
        panic!("Expected HealthCheck payload");
    }
}

#[tokio::test]
async fn test_configuration_update_request() {
    let mut service = TestRpcService::new();

    // Create configuration update request
    let mut config_changes = HashMap::new();
    config_changes.insert("scan_interval_ms".to_string(), serde_json::json!(25000));
    config_changes.insert("batch_size".to_string(), serde_json::json!(500));

    let config_req = ConfigUpdateRequest {
        collector_id: "test-collector".to_string(),
        config_changes,
        validate_only: false,
        restart_required: false,
        rollback_on_failure: true,
    };

    let request = RpcRequest::config_update(
        "test-client".to_string(),
        "control.config.test-collector".to_string(),
        config_req,
        Duration::from_secs(30),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::UpdateConfig);

    // Verify configuration was updated
    assert_eq!(
        service.config.get("scan_interval_ms"),
        Some(&serde_json::json!(25000))
    );
    assert_eq!(
        service.config.get("batch_size"),
        Some(&serde_json::json!(500))
    );
}

#[tokio::test]
async fn test_capabilities_request() {
    let mut service = TestRpcService::new();

    // Create capabilities request
    let request = RpcRequest::capabilities(
        "test-client".to_string(),
        "control.capabilities.test-collector".to_string(),
        Duration::from_secs(10),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::GetCapabilities);

    if let Some(RpcPayload::Capabilities(capabilities)) = response.payload {
        assert_eq!(capabilities.collector_id, "test-collector");
        assert!(capabilities.event_types.contains(&"process".to_string()));
        assert!(capabilities.operations.contains(&CollectorOperation::Start));
        assert!(capabilities.platform_support.contains(&"linux".to_string()));
        assert_eq!(capabilities.features.get("hash_verification"), Some(&true));
    } else {
        panic!("Expected Capabilities payload");
    }
}

#[tokio::test]
async fn test_graceful_shutdown_request() {
    let mut service = TestRpcService::new();
    service.collector_running = true;

    // Create graceful shutdown request
    let shutdown_req = ShutdownRequest {
        collector_id: "test-collector".to_string(),
        shutdown_type: ShutdownType::Graceful,
        graceful_timeout_ms: 60000,
        force_after_timeout: true,
        reason: Some("Test shutdown".to_string()),
    };

    use daemoneye_eventbus::topics::shutdown;
    let request = RpcRequest::shutdown(
        "test-client".to_string(),
        shutdown::shutdown_topic("test-collector"),
        shutdown_req,
        Duration::from_secs(60),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::GracefulShutdown);
}

#[tokio::test]
async fn test_force_shutdown_request() {
    let mut service = TestRpcService::new();
    service.collector_running = true;

    // Create force shutdown request
    let shutdown_req = ShutdownRequest {
        collector_id: "test-collector".to_string(),
        shutdown_type: ShutdownType::Emergency,
        graceful_timeout_ms: 0,
        force_after_timeout: false,
        reason: Some("Emergency shutdown".to_string()),
    };

    use daemoneye_eventbus::topics::shutdown;
    let request = RpcRequest::shutdown(
        "test-client".to_string(),
        shutdown::shutdown_topic("test-collector"),
        shutdown_req,
        Duration::from_secs(5),
    );

    // Handle request
    let response = service.handle_request(request).await;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::ForceShutdown);
}

#[tokio::test]
async fn test_health_check_nonexistent_returns_success_unhealthy() {
    use daemoneye_eventbus::process_manager::{CollectorProcessManager, ProcessManagerConfig};
    use daemoneye_eventbus::rpc::{
        CollectorOperation, HealthStatus, RpcPayload, RpcRequest, RpcStatus,
    };
    use std::time::Duration;

    // Minimal service with empty process manager
    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::HealthCheck],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let service = CollectorRpcService::new("svc".to_string(), capabilities, pm);

    let mut payload = std::collections::HashMap::new();
    payload.insert(
        "collector_id".to_string(),
        serde_json::json!("does-not-exist"),
    );
    let req = RpcRequest::new(
        "client".to_string(),
        "control.health.does-not-exist".to_string(),
        CollectorOperation::HealthCheck,
        RpcPayload::Generic(payload),
        Duration::from_secs(2),
    );

    let resp = service.handle_request(req).await;
    assert_eq!(resp.status, RpcStatus::Success);
    if let Some(RpcPayload::HealthCheck(hd)) = resp.payload {
        assert_eq!(hd.collector_id, "does-not-exist");
        assert_eq!(hd.status, HealthStatus::Unhealthy);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

#[cfg(unix)] // Uses Unix executable permissions without proper Windows extension
#[tokio::test]
async fn test_config_validate_only_via_rpc() {
    use daemoneye_eventbus::process_manager::{
        CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
    };
    use tempfile::TempDir;

    // Prepare a valid config directory
    let temp_dir = TempDir::new().unwrap();
    let collector_id = "rpc-validate";
    let bin_path = temp_dir.path().join("bin");
    std::fs::write(&bin_path, b"").unwrap();

    // Make binary executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }

    let cfg = CollectorConfig {
        binary_path: std::path::PathBuf::from(&bin_path),
        ..Default::default()
    };
    // Seed disk file so ConfigManager::get_config succeeds
    let toml = toml::to_string_pretty(&cfg).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    // Service with custom config manager (no broker required here)
    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        std::path::PathBuf::from(temp_dir.path()),
    ));
    let service =
        CollectorRpcService::with_config_manager("svc".to_string(), capabilities, pm, cm, None);

    // Validate-only update
    let mut changes = std::collections::HashMap::new();
    changes.insert("max_restarts".to_string(), serde_json::json!(9));
    let req = RpcRequest::config_update(
        "client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: true,
            restart_required: false,
            rollback_on_failure: true,
        },
        Duration::from_secs(2),
    );

    let resp = service.handle_request(req).await;
    assert_eq!(resp.status, RpcStatus::Success);
}

#[cfg(unix)] // Uses Unix socket for broker
#[tokio::test]
async fn test_hot_reload_notification_published() {
    use daemoneye_eventbus::broker::DaemoneyeBroker;
    use daemoneye_eventbus::process_manager::{
        CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    let broker = Arc::new(
        DaemoneyeBroker::new("/tmp/test-hotreload.sock")
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    // Prepare valid config on disk
    let temp_dir = TempDir::new().unwrap();
    let collector_id = "rpc-hotreload";
    let bin_path = temp_dir.path().join("bin");
    std::fs::write(&bin_path, b"").unwrap();

    // Make binary executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }

    let cfg = CollectorConfig {
        binary_path: std::path::PathBuf::from(&bin_path),
        ..Default::default()
    };
    let toml = toml::to_string_pretty(&cfg).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        std::path::PathBuf::from(temp_dir.path()),
    ));
    let service = CollectorRpcService::with_config_manager(
        "svc".to_string(),
        capabilities,
        pm,
        cm,
        Some(broker.clone()),
    );

    // Subscribe to config change notifications
    let topic = format!("control.collector.config.{}", collector_id);
    let subscriber_id = Uuid::new_v4();
    let mut rx = broker.subscribe_raw(&topic, subscriber_id).await.unwrap();

    // Apply a change that does not require restart (auto_restart)
    let mut changes = std::collections::HashMap::new();
    changes.insert("auto_restart".to_string(), serde_json::json!(true));
    let req = RpcRequest::config_update(
        "client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false,
            rollback_on_failure: true,
        },
        Duration::from_secs(2),
    );

    let resp = service.handle_request(req).await;
    assert_eq!(resp.status, RpcStatus::Success);

    // Expect a notification on the topic
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut got = false;
    while std::time::Instant::now() < deadline {
        if let Some(_msg) = rx.recv().await {
            got = true;
            break;
        }
    }
    assert!(got, "Expected hot-reload notification to be published");
}

#[tokio::test]
#[ignore = "Flaky test - timing sensitive temp directory cleanup during process restart"]
async fn test_config_change_triggers_restart_path() {
    use tempfile::TempDir;

    // Prepare a valid config directory and start a collector so restart can succeed
    let temp_dir = TempDir::new().unwrap();
    let collector_id = "rpc-restart";
    let bin_path = temp_dir.path().join("bin.sh");
    // Create a simple shell script that sleeps to simulate running
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&bin_path, b"#!/bin/sh\nsleep 5\n").unwrap();
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::write(&bin_path, b"@echo off\n timeout /t 5 /nobreak > nul\n").unwrap();
    }

    let cfg_file = CollectorConfig {
        binary_path: std::path::PathBuf::from(&bin_path),
        ..Default::default()
    };
    let toml = toml::to_string_pretty(&cfg_file).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    // Process manager with short timeouts for faster test
    let pm_cfg = ProcessManagerConfig {
        default_graceful_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let pm = CollectorProcessManager::new(pm_cfg.clone());

    // Start the collector so restart has a target
    let cc = CollectorConfig {
        binary_path: std::path::PathBuf::from(&bin_path),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 3,
    };
    pm.start_collector(collector_id, "test", cc)
        .await
        .expect("start");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        std::path::PathBuf::from(temp_dir.path()),
    ));
    let service = CollectorRpcService::with_config_manager(
        "svc".to_string(),
        capabilities,
        pm.clone(),
        cm,
        None,
    );

    // Apply a change that requires restart (args)
    let mut changes = std::collections::HashMap::new();
    changes.insert("args".to_string(), serde_json::json!(vec!["--foo"]));
    let req = RpcRequest::config_update(
        "client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false, // rely on requires_restart
            rollback_on_failure: true,
        },
        Duration::from_secs(5),
    );

    let resp = service.handle_request(req).await;
    assert_eq!(resp.status, RpcStatus::Success);
    // Verify collector still exists after restart path
    let status = pm.get_collector_status(collector_id).await.unwrap();
    assert_eq!(
        status.state,
        daemoneye_eventbus::process_manager::CollectorState::Running
    );
}

#[tokio::test]
async fn test_rpc_request_serialization() {
    let lifecycle_req = CollectorLifecycleRequest::start("test-collector", None);
    let request = RpcRequest::lifecycle(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        CollectorOperation::Start,
        lifecycle_req,
        Duration::from_secs(30),
    );

    // Test serialization
    let serialized = postcard::to_allocvec(&request);
    assert!(serialized.is_ok());

    // Test deserialization
    let deserialized_request: RpcRequest = postcard::from_bytes(&serialized.unwrap()).unwrap();
    assert_eq!(deserialized_request.client_id, "test-client");
    assert_eq!(deserialized_request.operation, CollectorOperation::Start);
}

#[tokio::test]
async fn test_rpc_response_serialization() {
    let health_data = HealthCheckData {
        collector_id: "test-collector".to_string(),
        status: HealthStatus::Healthy,
        components: HashMap::new(),
        metrics: HashMap::new(),
        last_heartbeat: SystemTime::now(),
        uptime_seconds: 3600,
        error_count: 0,
    };

    let response = RpcResponse {
        request_id: "test-request".to_string(),
        service_id: "test-service".to_string(),
        operation: CollectorOperation::HealthCheck,
        status: RpcStatus::Success,
        payload: Some(RpcPayload::HealthCheck(health_data)),
        timestamp: SystemTime::now(),
        execution_time_ms: 100,
        queue_time_ms: None,
        total_time_ms: 100,
        error_details: None,
        correlation_metadata: RpcCorrelationMetadata::default(),
    };

    // Test serialization
    let serialized = postcard::to_allocvec(&response);
    assert!(serialized.is_ok());

    // Test deserialization
    let deserialized_response: RpcResponse = postcard::from_bytes(&serialized.unwrap()).unwrap();
    assert_eq!(deserialized_response.service_id, "test-service");
    assert_eq!(deserialized_response.status, RpcStatus::Success);

    if let Some(RpcPayload::HealthCheck(health_data)) = deserialized_response.payload {
        assert_eq!(health_data.collector_id, "test-collector");
        assert_eq!(health_data.status, HealthStatus::Healthy);
    } else {
        panic!("Expected HealthCheck payload");
    }
}

#[tokio::test]
async fn test_rpc_timeout_handling() {
    // Test that RPC requests include proper timeout information
    let request = RpcRequest::health_check(
        "test-client".to_string(),
        "control.health.test-collector".to_string(),
        Duration::from_secs(5),
    );

    let delta = request
        .deadline
        .duration_since(request.timestamp)
        .unwrap_or(Duration::ZERO);
    assert!(delta >= Duration::from_millis(4500) && delta <= Duration::from_millis(5500));
    assert!(!request.request_id.is_empty());
    assert!(!request.correlation_metadata.correlation_id.is_empty());
}

#[tokio::test]
async fn test_service_capabilities_validation() {
    let capabilities = ServiceCapabilities {
        operations: vec![
            CollectorOperation::Start,
            CollectorOperation::Stop,
            CollectorOperation::HealthCheck,
        ],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    // Create dummy process manager
    use daemoneye_eventbus::process_manager::{CollectorProcessManager, ProcessManagerConfig};
    let pm_config = ProcessManagerConfig::default();
    let process_manager = CollectorProcessManager::new(pm_config);

    let service =
        CollectorRpcService::new("test-service".to_string(), capabilities, process_manager);

    // Verify service configuration
    assert_eq!(service.service_id, "test-service");
    assert_eq!(service.supported_operations.len(), 3);
    assert!(
        service
            .supported_operations
            .contains(&CollectorOperation::Start)
    );
    assert!(
        service
            .supported_operations
            .contains(&CollectorOperation::Stop)
    );
    assert!(
        service
            .supported_operations
            .contains(&CollectorOperation::HealthCheck)
    );
}

#[tokio::test]
async fn test_error_response_handling() {
    let service = TestRpcService::new();

    // Create request for unsupported operation
    let request = RpcRequest::new(
        "test-client".to_string(),
        "control.collector.test-collector".to_string(),
        CollectorOperation::Pause, // Not supported by test service
        RpcPayload::Empty,
        Duration::from_secs(10),
    );

    // Handle request - should return error for unsupported operation
    let response = service.service.handle_request(request).await;

    // Verify error response
    assert_eq!(response.status, RpcStatus::Error);
    assert!(response.error_details.is_some());

    if let Some(error) = response.error_details {
        assert_eq!(error.code, "UNSUPPORTED_OPERATION");
        assert!(error.message.contains("not supported"));
    }
}

//
// Broker-based RPC Integration Tests
//

/// Test helper function to setup broker and client
async fn setup_test_broker_and_client()
-> Result<(tempfile::TempDir, Arc<DaemoneyeBroker>, CollectorRpcClient)> {
    // Create temporary directory for socket
    let temp_dir = tempfile::tempdir()
        .map_err(|e| EventBusError::configuration(format!("Failed to create temp dir: {}", e)))?;

    let socket_path = temp_dir.path().join("test_broker.sock");
    let socket_path_str = socket_path.to_string_lossy().to_string();

    // Create and start broker
    let broker = DaemoneyeBroker::new(&socket_path_str).await?;
    broker.start().await?;
    let broker = Arc::new(broker);

    // Create RPC client
    let client = CollectorRpcClient::new("control.collector.test", broker.clone()).await?;

    Ok((temp_dir, broker, client))
}

/// Test helper function to setup RPC service handler
async fn setup_test_service_handler(
    broker: Arc<DaemoneyeBroker>,
    service_topic: &str,
) -> Result<tokio::task::JoinHandle<()>> {
    let service_topic = service_topic.to_string();
    let subscriber_id = Uuid::new_v4();

    // Subscribe to service topic for incoming RPC requests
    let mut message_receiver = broker.subscribe_raw(&service_topic, subscriber_id).await?;

    // Spawn background task to handle requests
    let handle = tokio::spawn(async move {
        let mut test_service = TestRpcService::new();

        while let Some(message) = message_receiver.recv().await {
            // Debug the received message
            println!(
                "DEBUG: Service received message - topic: {}, payload length: {}",
                message.topic,
                message.payload.len()
            );

            // Skip response messages (only handle request messages)
            if message.topic.contains("rpc.response") {
                println!(
                    "DEBUG: Skipping response message on topic: {}",
                    message.topic
                );
                continue;
            }

            println!(
                "DEBUG: Message payload first 20 bytes: {:?}",
                &message.payload[..std::cmp::min(20, message.payload.len())]
            );

            // Deserialize RPC request
            let request: RpcRequest = match postcard::from_bytes::<RpcRequest>(&message.payload) {
                Ok(req) => {
                    println!(
                        "DEBUG: Successfully deserialized RPC request: {:?}",
                        req.operation
                    );
                    req
                }
                Err(e) => {
                    eprintln!("Failed to deserialize RPC request: {}", e);
                    eprintln!("DEBUG: Payload bytes: {:?}", message.payload);
                    continue;
                }
            };

            // Handle request
            let response = test_service.handle_request(request.clone()).await;

            // Determine response topic from request - use client_id as the unique identifier
            let response_topic = format!("control.rpc.response.{}", request.client_id);

            println!(
                "DEBUG: Sending response to topic '{}' for request ID '{}'",
                response_topic, request.request_id
            );

            // Serialize and publish response
            let payload = match postcard::to_allocvec(&response) {
                Ok(data) => {
                    println!("DEBUG: Serialized response payload length: {}", data.len());
                    data
                }
                Err(e) => {
                    eprintln!("Failed to serialize RPC response: {}", e);
                    continue;
                }
            };

            match broker
                .publish(&response_topic, &response.request_id, payload)
                .await
            {
                Ok(()) => println!(
                    "DEBUG: Successfully published response to {}",
                    response_topic
                ),
                Err(e) => eprintln!("Failed to publish RPC response: {}", e),
            }
        }
    });

    Ok(handle)
}

#[tokio::test]
async fn test_rpc_call_through_broker() -> Result<()> {
    let (_temp_dir, broker, client) = setup_test_broker_and_client().await?;
    let _service_handle =
        setup_test_service_handler(broker.clone(), "control.collector.test").await?;

    // Give service handler time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create health check request
    let request = RpcRequest::health_check(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        Duration::from_secs(5),
    );

    println!("DEBUG: Created RPC request: {:?}", request);
    println!("DEBUG: Operation variant: {:?}", request.operation as u8);
    println!(
        "DEBUG: Payload variant: {:?}",
        std::mem::discriminant(&request.payload)
    );

    // Test serialization/deserialization locally first
    let serialized = postcard::to_allocvec(&request).unwrap();
    println!("DEBUG: Serialized length: {}", serialized.len());

    let deserialized: RpcRequest = postcard::from_bytes(&serialized).unwrap();
    println!(
        "DEBUG: Deserialized successfully: {:?}",
        deserialized.operation
    );

    // Make RPC call
    let response = client.call(request, Duration::from_secs(5)).await?;

    // Verify response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::HealthCheck);

    // Verify health check payload
    if let Some(RpcPayload::HealthCheck(health_data)) = response.payload {
        assert_eq!(health_data.status, HealthStatus::Unhealthy);
    } else {
        panic!("Expected HealthCheck payload in response");
    }

    Ok(())
}

#[tokio::test]
async fn test_rpc_call_with_lifecycle_operation() -> Result<()> {
    let (_temp_dir, broker, client) = setup_test_broker_and_client().await?;
    let _service_handle =
        setup_test_service_handler(broker.clone(), "control.collector.test").await?;

    // Give service handler time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create start request
    let lifecycle_request = CollectorLifecycleRequest::start("test-collector", None);
    let request = RpcRequest::lifecycle(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        CollectorOperation::Start,
        lifecycle_request,
        Duration::from_secs(5),
    );

    // Make RPC call for start
    let response = client.call(request, Duration::from_secs(5)).await?;

    // Verify start response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::Start);

    // Create stop request
    let lifecycle_request = CollectorLifecycleRequest::stop("test-collector");
    let request = RpcRequest::lifecycle(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        CollectorOperation::Stop,
        lifecycle_request,
        Duration::from_secs(5),
    );

    // Make RPC call for stop
    let response = client.call(request, Duration::from_secs(5)).await?;

    // Verify stop response
    assert_eq!(response.status, RpcStatus::Success);
    assert_eq!(response.operation, CollectorOperation::Stop);

    Ok(())
}

#[tokio::test]
async fn test_rpc_call_timeout() -> Result<()> {
    let (_temp_dir, _broker, client) = setup_test_broker_and_client().await?;
    // Note: No service handler setup, so requests will timeout

    // Create request with short timeout
    let request = RpcRequest::health_check(
        client.client_id.clone(),
        "control.collector.nonexistent".to_string(),
        Duration::from_millis(100),
    );

    // Make RPC call and expect timeout
    let result = client.call(request, Duration::from_millis(100)).await;

    // Verify timeout error
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(
        error.is_timeout(),
        "Expected timeout error, got: {:?}",
        error
    );

    Ok(())
}

#[tokio::test]
async fn test_rpc_call_timeout_cleanup() -> Result<()> {
    let (_temp_dir, broker, client) = setup_test_broker_and_client().await?;
    // No service handler, so all requests will timeout

    // Send multiple requests with short timeouts
    let mut handles = Vec::new();

    for _i in 0..5 {
        let broker_clone = broker.clone();
        let handle = tokio::spawn(async move {
            // Create a separate client for each timeout test
            let client =
                CollectorRpcClient::new("control.collector.nonexistent", broker_clone).await?;

            let request = RpcRequest::health_check(
                client.client_id.clone(),
                "control.collector.nonexistent".to_string(),
                Duration::from_millis(50),
            );

            client.call(request, Duration::from_millis(50)).await
        });
        handles.push(handle);
    }

    // Wait for all requests to timeout
    for handle in handles {
        let result = handle.await.unwrap(); // This returns Result<RpcResponse, EventBusError>
        assert!(result.is_err()); // This should be a timeout error
        assert!(result.unwrap_err().is_timeout());
    }

    // Verify subsequent requests still work (no resource leaks)
    let request = RpcRequest::health_check(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        Duration::from_millis(100),
    );

    let result = client.call(request, Duration::from_millis(100)).await;
    // Should timeout again but not crash
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_rpc_response_correlation() -> Result<()> {
    let (_temp_dir, broker, _client) = setup_test_broker_and_client().await?;
    let _service_handle =
        setup_test_service_handler(broker.clone(), "control.collector.test").await?;

    // Give service handler time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send multiple concurrent requests
    let mut handles = Vec::new();

    for _i in 0..3 {
        let broker_clone = broker.clone();
        let handle = tokio::spawn(async move {
            // Create a separate client for each concurrent request
            let client = CollectorRpcClient::new("control.collector.test", broker_clone).await?;

            let request = RpcRequest::health_check(
                client.client_id.clone(),
                "control.collector.test".to_string(),
                Duration::from_secs(5),
            );

            let request_id = request.request_id.clone();
            let response = client.call(request, Duration::from_secs(5)).await?;

            // Verify correlation
            assert_eq!(response.request_id, request_id);
            assert_eq!(response.status, RpcStatus::Success);

            Ok::<_, EventBusError>(response.request_id)
        });
        handles.push(handle);
    }

    // Wait for all responses and verify each is correctly correlated
    for handle in handles {
        let request_id = handle.await.unwrap()?;
        // Each request should have received its own response
        assert!(!request_id.is_empty());
    }

    Ok(())
}

#[tokio::test]
async fn test_multiple_rpc_clients() -> Result<()> {
    let (_temp_dir, broker, _) = setup_test_broker_and_client().await?;
    let _service_handle =
        setup_test_service_handler(broker.clone(), "control.collector.test").await?;

    // Give service handler time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create multiple clients
    let client1 = CollectorRpcClient::new("control.collector.test", broker.clone()).await?;
    let client2 = CollectorRpcClient::new("control.collector.test", broker.clone()).await?;
    let client3 = CollectorRpcClient::new("control.collector.test", broker.clone()).await?;

    // Send requests from all clients concurrently
    let handle1 = tokio::spawn(async move {
        let request = RpcRequest::health_check(
            client1.client_id.clone(),
            "control.collector.test".to_string(),
            Duration::from_secs(5),
        );
        client1.call(request, Duration::from_secs(5)).await
    });

    let handle2 = tokio::spawn(async move {
        let request = RpcRequest::health_check(
            client2.client_id.clone(),
            "control.collector.test".to_string(),
            Duration::from_secs(5),
        );
        client2.call(request, Duration::from_secs(5)).await
    });

    let handle3 = tokio::spawn(async move {
        let request = RpcRequest::health_check(
            client3.client_id.clone(),
            "control.collector.test".to_string(),
            Duration::from_secs(5),
        );
        client3.call(request, Duration::from_secs(5)).await
    });

    // Wait for all responses
    let response1 = handle1.await.unwrap()?;
    let response2 = handle2.await.unwrap()?;
    let response3 = handle3.await.unwrap()?;

    // Verify all responses are successful and unique
    assert_eq!(response1.status, RpcStatus::Success);
    assert_eq!(response2.status, RpcStatus::Success);
    assert_eq!(response3.status, RpcStatus::Success);

    // Verify request IDs are different (no cross-contamination)
    assert_ne!(response1.request_id, response2.request_id);
    assert_ne!(response2.request_id, response3.request_id);
    assert_ne!(response1.request_id, response3.request_id);

    Ok(())
}

#[tokio::test]
async fn test_rpc_client_shutdown() -> Result<()> {
    let (_temp_dir, broker, client) = setup_test_broker_and_client().await?;
    let _service_handle =
        setup_test_service_handler(broker.clone(), "control.collector.test").await?;

    // Give service handler time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Make a successful request first
    let request = RpcRequest::health_check(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        Duration::from_secs(5),
    );

    let response = client.call(request, Duration::from_secs(5)).await?;
    assert_eq!(response.status, RpcStatus::Success);

    // Shutdown client
    client.shutdown().await?;

    // Subsequent requests should fail appropriately
    let request = RpcRequest::health_check(
        client.client_id.clone(),
        "control.collector.test".to_string(),
        Duration::from_secs(1),
    );

    // The call should either fail immediately or timeout
    let result = client.call(request, Duration::from_secs(1)).await;
    // We expect this to fail in some way after shutdown
    assert!(result.is_err());

    // Cleanup
    broker.shutdown().await?;

    Ok(())
}

//
// Process Manager Integration Tests
//

use daemoneye_eventbus::process_manager::{
    CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
};
use std::path::PathBuf;
use tempfile::TempDir;

/// Setup test process manager with mock binaries
#[allow(dead_code)]
fn setup_test_process_manager() -> (Arc<CollectorProcessManager>, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create mock collector binary
    let binary_path = create_mock_collector_binary(&temp_dir, 300);

    let mut collector_binaries = HashMap::new();
    collector_binaries.insert("test-collector".to_string(), binary_path.clone());

    let config = ProcessManagerConfig {
        collector_binaries,
        default_graceful_timeout: Duration::from_secs(5),
        default_force_timeout: Duration::from_secs(2),
        health_check_interval: Duration::from_secs(10),
        enable_auto_restart: false,
        heartbeat_timeout_multiplier: 3,
    };

    let manager = CollectorProcessManager::new(config);
    (manager, temp_dir)
}

/// Create mock collector binary for testing
#[cfg(unix)]
fn create_mock_collector_binary(temp_dir: &TempDir, sleep_duration: u64) -> PathBuf {
    let script_path = temp_dir.path().join("mock_collector.sh");
    let script_content = format!(
        r#"#!/bin/bash
# Mock collector that sleeps for {} seconds
echo "Mock collector started"
sleep {}
"#,
        sleep_duration, sleep_duration
    );

    std::fs::write(&script_path, script_content).expect("Failed to write script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path)
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).expect("Failed to set permissions");
    }

    script_path
}

#[cfg(windows)]
fn create_mock_collector_binary(temp_dir: &TempDir, sleep_duration: u64) -> PathBuf {
    let script_path = temp_dir.path().join("mock_collector.bat");
    let script_content = format!(
        r#"@echo off
REM Mock collector that sleeps for {} seconds
echo Mock collector started
timeout /t {} /nobreak
"#,
        sleep_duration, sleep_duration
    );

    std::fs::write(&script_path, script_content).expect("Failed to write script");
    script_path
}

#[tokio::test]
#[cfg_attr(
    not(all(unix, feature = "freebsd")),
    ignore = "Process manager tests require FreeBSD feature for signal support"
)]
async fn test_rpc_start_collector_with_process_manager() -> Result<()> {
    let (process_manager, _temp_dir) = setup_test_process_manager();

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Start],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create start request
    let lifecycle_req = CollectorLifecycleRequest {
        collector_id: "test-1".to_string(),
        collector_type: "test-collector".to_string(),
        config_overrides: Some(HashMap::new()),
        environment: None,
        working_directory: None,
        resource_limits: None,
        startup_timeout_ms: Some(10000),
    };

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Start,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(10000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    }; // Start collector
    let response = service.handle_request(request.clone()).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify collector is running
    let status = process_manager
        .get_collector_status("test-1")
        .await
        .expect("Failed to get status");
    assert!(status.pid > 0);

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(all(unix, feature = "freebsd")),
    ignore = "Process manager tests require FreeBSD feature for signal support"
)]
async fn test_rpc_stop_collector_with_process_manager() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Stop],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create stop request with Lifecycle payload (not Shutdown)
    let lifecycle_req = CollectorLifecycleRequest::stop("test-1");

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Stop,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(10000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Stop collector
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify collector is stopped
    let result = process_manager.get_collector_status("test-1").await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(all(unix, feature = "freebsd")),
    ignore = "Process manager tests require FreeBSD feature for signal support"
)]
async fn test_rpc_restart_collector_with_process_manager() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    let initial_pid = process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Restart],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create restart request
    let lifecycle_req = CollectorLifecycleRequest {
        collector_id: "test-1".to_string(),
        collector_type: "test-collector".to_string(),
        config_overrides: Some(HashMap::new()),
        environment: None,
        working_directory: None,
        resource_limits: None,
        startup_timeout_ms: Some(15000),
    };

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Restart,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(15000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Restart collector
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify collector restarted with new PID
    let status = process_manager
        .get_collector_status("test-1")
        .await
        .expect("Failed to get status");
    assert_ne!(status.pid, initial_pid);
    assert_eq!(status.restart_count, 1);

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(all(unix, feature = "freebsd")),
    ignore = "Process manager tests require FreeBSD feature for signal support"
)]
async fn test_rpc_health_check_with_running_collector() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::HealthCheck],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create health check request with proper payload containing collector_id
    let mut payload_map = HashMap::new();
    payload_map.insert(
        "collector_id".to_string(),
        serde_json::Value::String("test-1".to_string()),
    );

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Generic(payload_map),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(5000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Check health
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify health data
    if let Some(RpcPayload::HealthCheck(health_data)) = response.payload {
        assert_eq!(health_data.status, HealthStatus::Healthy);
        assert_eq!(health_data.collector_id, "test-1");
    } else {
        panic!("Expected HealthCheck payload");
    }

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(all(unix, feature = "freebsd")),
    ignore = "Process manager tests require FreeBSD feature for signal support"
)]
async fn test_rpc_health_check_with_stopped_collector() -> Result<()> {
    let (process_manager, _temp_dir) = setup_test_process_manager();

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::HealthCheck],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create health check request for non-existent collector
    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::HealthCheck,
        payload: RpcPayload::Empty,
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(5000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Check health - should return success status with unhealthy health data
    // The RPC call itself succeeds, but the health status indicates the collector is unhealthy
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify the health data shows unhealthy status
    if let Some(RpcPayload::HealthCheck(health_data)) = response.payload {
        assert_eq!(health_data.status, HealthStatus::Unhealthy);
    } else {
        panic!("Expected HealthCheck payload");
    }

    Ok(())
}

#[tokio::test]
#[cfg(all(unix, feature = "freebsd"))]
async fn test_rpc_pause_collector_with_process_manager() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Pause],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create pause request
    let lifecycle_req = CollectorLifecycleRequest {
        collector_id: "test-1".to_string(),
        collector_type: "test-collector".to_string(),
        config_overrides: Some(HashMap::new()),
        environment: None,
        working_directory: None,
        resource_limits: None,
        startup_timeout_ms: Some(5000),
    };

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Pause,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(5000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Pause collector
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify collector is paused
    let status = process_manager
        .get_collector_status("test-1")
        .await
        .expect("Failed to get status");
    assert_eq!(format!("{:?}", status.state), "Paused");

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

#[tokio::test]
#[cfg(all(unix, feature = "freebsd"))]
async fn test_rpc_resume_collector_with_process_manager() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    // Pause first
    process_manager
        .pause_collector("test-1")
        .await
        .expect("Failed to pause collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Resume],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create resume request
    let lifecycle_req = CollectorLifecycleRequest {
        collector_id: "test-1".to_string(),
        collector_type: "test-collector".to_string(),
        config_overrides: Some(HashMap::new()),
        environment: None,
        working_directory: None,
        resource_limits: None,
        startup_timeout_ms: Some(5000),
    };

    let now = SystemTime::now();
    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Resume,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: now,
        deadline: now.checked_add(Duration::from_millis(5000)).unwrap_or(now),
        correlation_metadata: RpcCorrelationMetadata::new(Uuid::new_v4().to_string()),
    };

    // Resume collector
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify collector is running
    let status = process_manager
        .get_collector_status("test-1")
        .await
        .expect("Failed to get status");
    assert_eq!(format!("{:?}", status.state), "Running");

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

#[tokio::test]
#[cfg(windows)]
async fn test_rpc_pause_not_supported_windows() -> Result<()> {
    let (process_manager, temp_dir) = setup_test_process_manager();

    let binary_path = create_mock_collector_binary(&temp_dir, 300);
    let collector_config = CollectorConfig {
        binary_path,
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        resource_limits: None,
        auto_restart: false,
        max_restarts: 0,
    };

    // Start collector directly
    process_manager
        .start_collector("test-1", "test-collector", collector_config)
        .await
        .expect("Failed to start collector");

    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::Pause],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec!["test-collector".to_string()],
    };

    let service = CollectorRpcService::new(
        "test-service".to_string(),
        capabilities,
        process_manager.clone(),
    );

    // Create pause request
    let lifecycle_req = CollectorLifecycleRequest {
        collector_id: "test-1".to_string(),
        collector_type: "test-collector".to_string(),
        config_overrides: None,
        environment: None,
        working_directory: None,
        resource_limits: None,
        startup_timeout_ms: None,
    };

    let request = RpcRequest {
        request_id: Uuid::new_v4().to_string(),
        client_id: "test-client".to_string(),
        target: "control.collector.test".to_string(),
        operation: CollectorOperation::Pause,
        payload: RpcPayload::Lifecycle(lifecycle_req),
        timestamp: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(5),
        correlation_metadata: RpcCorrelationMetadata::default(),
    };

    // Pause collector - should fail on Windows
    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Error);
    assert!(response.error_details.is_some());

    // Cleanup
    let _ = process_manager
        .stop_collector("test-1", false, Duration::from_secs(2))
        .await;

    Ok(())
}

/// Test that config update requiring restart changes PID
#[tokio::test]
#[ignore = "Flaky test - timing sensitive process restart under concurrent test execution"]
async fn test_restart_required_changes_pid() -> Result<()> {
    use daemoneye_eventbus::process_manager::{
        CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
    };
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let collector_id = "test-restart-pid";

    // Create executable script
    let bin_path = temp_dir.path().join("bin.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&bin_path, b"#!/bin/sh\nsleep 10\n").unwrap();
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::write(&bin_path, b"@echo off\ntimeout /t 10 /nobreak > nul\n").unwrap();
    }

    // Write initial config
    let cfg = CollectorConfig {
        binary_path: bin_path.clone(),
        ..Default::default()
    };
    let toml = toml::to_string_pretty(&cfg).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    // Create process manager and start collector
    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());
    pm.start_collector(collector_id, "test", cfg.clone())
        .await
        .expect("Failed to start collector");

    // Wait for process to stabilize
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get initial PID
    let initial_status = pm
        .get_collector_status(collector_id)
        .await
        .expect("Failed to get collector status");
    let initial_pid = initial_status.pid;

    // Create service with config manager
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        temp_dir.path().to_path_buf(),
    ));
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let service = CollectorRpcService::with_config_manager(
        "test-service".to_string(),
        capabilities,
        pm.clone(),
        cm,
        None,
    );

    // Apply config change that requires restart (args)
    let mut changes = HashMap::new();
    changes.insert("args".to_string(), serde_json::json!(vec!["--test"]));

    let request = RpcRequest::config_update(
        "test-client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false, // will be detected
            rollback_on_failure: true,
        },
        Duration::from_secs(10),
    );

    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Wait for restart to complete
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Get new PID
    let new_status = pm
        .get_collector_status(collector_id)
        .await
        .expect("Failed to get new collector status");
    let new_pid = new_status.pid;

    // Verify PID changed
    assert_ne!(initial_pid, new_pid, "PID should change after restart");

    // Cleanup
    let _ = pm
        .stop_collector(collector_id, false, Duration::from_secs(2))
        .await;

    Ok(())
}

/// Test that hot-reload publishes notification and PID remains same
#[cfg(unix)] // Uses Unix socket for broker
#[tokio::test]
async fn test_hot_reload_notification_and_pid_stable() -> Result<()> {
    use daemoneye_eventbus::broker::DaemoneyeBroker;
    use daemoneye_eventbus::process_manager::{
        CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    let broker = Arc::new(
        DaemoneyeBroker::new("/tmp/test-hotreload-pid.sock")
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    let temp_dir = TempDir::new().unwrap();
    let collector_id = "test-hotreload-pid";

    // Create executable script
    let bin_path = temp_dir.path().join("bin.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&bin_path, b"#!/bin/sh\nsleep 10\n").unwrap();
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::write(&bin_path, b"@echo off\ntimeout /t 10 /nobreak > nul\n").unwrap();
    }

    // Write initial config
    let cfg = CollectorConfig {
        binary_path: bin_path.clone(),
        ..Default::default()
    };
    let toml = toml::to_string_pretty(&cfg).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    // Create process manager and start collector
    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());
    pm.start_collector(collector_id, "test", cfg.clone())
        .await
        .expect("Failed to start collector");

    // Wait for process to stabilize
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get initial PID
    let initial_status = pm
        .get_collector_status(collector_id)
        .await
        .expect("Failed to get initial collector status");
    let initial_pid = initial_status.pid;

    // Create service with config manager and broker
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        temp_dir.path().to_path_buf(),
    ));
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let service = CollectorRpcService::with_config_manager(
        "test-service".to_string(),
        capabilities,
        pm.clone(),
        cm,
        Some(broker.clone()),
    );

    // Subscribe to config change notifications
    let topic = format!("control.collector.config.{}", collector_id);
    let subscriber_id = Uuid::new_v4();
    let mut rx = broker.subscribe_raw(&topic, subscriber_id).await.unwrap();

    // Apply hot-reload change (auto_restart doesn't require restart)
    let mut changes = HashMap::new();
    changes.insert("auto_restart".to_string(), serde_json::json!(true));

    let request = RpcRequest::config_update(
        "test-client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false,
            rollback_on_failure: true,
        },
        Duration::from_secs(5),
    );

    let response = service.handle_request(request).await;
    assert_eq!(response.status, RpcStatus::Success);

    // Verify notification was published
    let notification_received =
        tokio::time::timeout(Duration::from_secs(2), async { rx.recv().await }).await;
    assert!(
        notification_received.is_ok() && notification_received.unwrap().is_some(),
        "Expected hot-reload notification to be published"
    );

    // Wait a moment for any potential restart
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get current PID and verify it hasn't changed
    let current_status = pm
        .get_collector_status(collector_id)
        .await
        .expect("Failed to get current collector status");
    let current_pid = current_status.pid;

    assert_eq!(
        initial_pid, current_pid,
        "PID should remain same after hot-reload"
    );

    // Cleanup
    let _ = pm
        .stop_collector(collector_id, false, Duration::from_secs(2))
        .await;
    broker.shutdown().await?;

    Ok(())
}

/// Test config update rollback on validation failure
#[cfg(unix)] // Uses Unix shell scripts for mock collectors
#[tokio::test]
async fn test_config_update_rollback_on_failure() -> Result<()> {
    use daemoneye_eventbus::process_manager::{
        CollectorConfig, CollectorProcessManager, ProcessManagerConfig,
    };
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let collector_id = "test-rollback";

    // Create valid executable
    let bin_path = temp_dir.path().join("bin.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&bin_path, b"#!/bin/sh\nsleep 10\n").unwrap();
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::write(&bin_path, b"@echo off\ntimeout /t 10 /nobreak > nul\n").unwrap();
    }

    // Write initial config with auto_restart=false
    let cfg = CollectorConfig {
        binary_path: bin_path.clone(),
        auto_restart: false,
        ..Default::default()
    };
    let toml = toml::to_string_pretty(&cfg).unwrap();
    std::fs::write(temp_dir.path().join(format!("{}.toml", collector_id)), toml).unwrap();

    // Create process manager
    let pm = CollectorProcessManager::new(ProcessManagerConfig::default());

    // Create service with config manager
    let cm = Arc::new(daemoneye_eventbus::ConfigManager::new(
        temp_dir.path().to_path_buf(),
    ));
    let capabilities = ServiceCapabilities {
        operations: vec![CollectorOperation::UpdateConfig],
        max_concurrent_requests: 10,
        timeout_limits: TimeoutLimits {
            min_timeout_ms: 1000,
            max_timeout_ms: 300000,
            default_timeout_ms: 30000,
        },
        supported_collectors: vec![],
    };
    let service = CollectorRpcService::with_config_manager(
        "test-service".to_string(),
        capabilities,
        pm.clone(),
        cm.clone(),
        None,
    );

    // Try to apply invalid config change (invalid memory limit)
    let mut changes = HashMap::new();
    changes.insert(
        "resource_limits".to_string(),
        serde_json::json!({
            "max_memory_bytes": 0  // Invalid: zero not allowed
        }),
    );

    let request = RpcRequest::config_update(
        "test-client".to_string(),
        format!("control.config.{}", collector_id),
        ConfigUpdateRequest {
            collector_id: collector_id.to_string(),
            config_changes: changes,
            validate_only: false,
            restart_required: false,
            rollback_on_failure: true,
        },
        Duration::from_secs(5),
    );

    let response = service.handle_request(request).await;

    // Should fail validation
    assert_eq!(response.status, RpcStatus::Error);
    assert!(response.error_details.is_some());

    // Verify original config is still in place
    let current_config = cm
        .get_config(collector_id)
        .await
        .expect("Failed to get config");
    assert!(!current_config.auto_restart);
    assert!(current_config.resource_limits.is_none());

    Ok(())
}
