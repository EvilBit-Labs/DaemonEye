#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::print_stdout,
    clippy::clone_on_ref_ptr,
    clippy::indexing_slicing,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::let_underscore_must_use,
    clippy::items_after_statements,
    clippy::wildcard_enum_match_arm,
    clippy::non_ascii_literal,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::semicolon_outside_block,
    clippy::redundant_clone,
    clippy::pattern_type_mismatch,
    clippy::ignore_without_reason,
    clippy::redundant_else,
    clippy::explicit_iter_loop,
    clippy::match_same_arms,
    clippy::significant_drop_tightening,
    clippy::redundant_closure_for_method_calls,
    clippy::equatable_if_let,
    clippy::manual_string_new
)]

use super::*;
use std::time::{Duration, SystemTime};

#[test]
fn test_rpc_request_creation() {
    let request = RpcRequest::health_check(
        "test-client".to_string(),
        "control.collector.procmond".to_string(),
        Duration::from_secs(10),
    );

    assert_eq!(request.client_id, "test-client");
    assert_eq!(request.target, "control.collector.procmond");
    assert_eq!(request.operation, CollectorOperation::HealthCheck);
    let delta = request
        .deadline
        .duration_since(request.timestamp)
        .unwrap_or(Duration::ZERO);
    assert!(delta >= Duration::from_secs(9) && delta <= Duration::from_secs(11));
    assert!(!request.correlation_metadata.correlation_id.is_empty());
}

#[test]
fn test_lifecycle_request_creation() {
    let lifecycle_req = CollectorLifecycleRequest::start("procmond", None);
    assert_eq!(lifecycle_req.collector_id, "procmond");
    assert_eq!(lifecycle_req.collector_type, "procmond");
    assert_eq!(lifecycle_req.startup_timeout_ms, Some(30000));
}

#[test]
fn test_rpc_response_serialization() {
    let response = RpcResponse {
        request_id: "test-request".to_string(),
        service_id: "test-service".to_string(),
        operation: CollectorOperation::HealthCheck,
        status: RpcStatus::Success,
        payload: None,
        timestamp: SystemTime::now(),
        execution_time_ms: 100,
        queue_time_ms: None,
        total_time_ms: 100,
        error_details: None,
        correlation_metadata: RpcCorrelationMetadata::default(),
    };

    let serialized = postcard::to_allocvec(&response).expect("serialization should succeed");

    let deserialized_response: RpcResponse =
        postcard::from_bytes(&serialized).expect("deserialization should succeed");
    assert_eq!(deserialized_response.service_id, "test-service");
    assert_eq!(deserialized_response.status, RpcStatus::Success);
}

#[tokio::test]
async fn test_rpc_service_creation() {
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
        supported_collectors: vec!["procmond".to_string()],
    };

    // Create a dummy process manager for testing
    let process_manager_config = crate::process_manager::ProcessManagerConfig::default();
    let process_manager =
        crate::process_manager::CollectorProcessManager::new(process_manager_config);

    let service =
        CollectorRpcService::new("test-service".to_string(), capabilities, process_manager);
    assert_eq!(service.service_id, "test-service");
    assert_eq!(service.supported_operations.len(), 3);
}
