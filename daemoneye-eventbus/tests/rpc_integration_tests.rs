//! Integration tests for RPC call patterns
//!
//! These tests validate the RPC patterns for collector lifecycle management,
//! health checks, configuration updates, and graceful shutdown coordination.

use daemoneye_eventbus::rpc::{
    CapabilitiesData, CollectorLifecycleRequest, CollectorOperation, CollectorRpcService,
    ComponentHealth, ConfigUpdateRequest, HealthCheckData, HealthStatus, ResourceRequirements,
    RpcPayload, RpcRequest, RpcResponse, RpcStatus, ServiceCapabilities, ShutdownRequest,
    ShutdownType, TimeoutLimits,
};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

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

        let service = CollectorRpcService::new("test-service".to_string(), capabilities);

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
            CollectorOperation::HealthCheck => {
                let health_data = self.create_health_data();
                self.create_success_response(request, Some(RpcPayload::HealthCheck(health_data)))
            }
            CollectorOperation::UpdateConfig => {
                if let RpcPayload::ConfigUpdate(ref config_req) = request.payload {
                    for (key, value) in config_req.config_changes.clone() {
                        self.config.insert(key, value);
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
            error_details: None,
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

    let request = RpcRequest::shutdown(
        "test-client".to_string(),
        "control.shutdown.test-collector".to_string(),
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

    let request = RpcRequest::shutdown(
        "test-client".to_string(),
        "control.shutdown.test-collector".to_string(),
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
    let serialized = bincode::serde::encode_to_vec(&request, bincode::config::standard());
    assert!(serialized.is_ok());

    // Test deserialization
    let deserialized =
        bincode::serde::decode_from_slice(&serialized.unwrap(), bincode::config::standard());
    assert!(deserialized.is_ok());

    let (deserialized_request, _): (RpcRequest, _) = deserialized.unwrap();
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
        error_details: None,
    };

    // Test serialization
    let serialized = bincode::serde::encode_to_vec(&response, bincode::config::standard());
    assert!(serialized.is_ok());

    // Test deserialization
    let deserialized =
        bincode::serde::decode_from_slice(&serialized.unwrap(), bincode::config::standard());
    assert!(deserialized.is_ok());

    let (deserialized_response, _): (RpcResponse, _) = deserialized.unwrap();
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

    assert_eq!(request.timeout_ms, 5000);
    assert!(!request.request_id.is_empty());
    assert!(!request.correlation_id.is_empty());
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

    let service = CollectorRpcService::new("test-service".to_string(), capabilities);

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
