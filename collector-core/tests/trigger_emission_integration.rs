//! Integration tests for trigger request emission system.

use collector_core::{
    event::{AnalysisType, TriggerPriority, TriggerRequest},
    trigger::ConditionType,
    trigger::{TriggerCapabilities, TriggerConfig, TriggerManager, TriggerResourceLimits},
};
use std::collections::HashMap;
use std::time::SystemTime;

/// Creates test collector capabilities for testing.
fn create_test_capabilities() -> TriggerCapabilities {
    TriggerCapabilities {
        collector_id: "binary-hasher".to_string(),
        supported_conditions: vec![
            ConditionType::ProcessNamePattern("test".to_string()),
            ConditionType::MissingExecutable,
        ],
        supported_analysis: vec![AnalysisType::BinaryHash, AnalysisType::MemoryAnalysis],
        max_trigger_rate: 100,
        max_concurrent_analysis: 10,
        supported_priorities: vec![
            TriggerPriority::Low,
            TriggerPriority::Normal,
            TriggerPriority::High,
            TriggerPriority::Critical,
        ],
        resource_limits: TriggerResourceLimits::default(),
    }
}

/// Creates a test trigger request for testing.
fn create_test_trigger_request() -> TriggerRequest {
    TriggerRequest {
        trigger_id: "test_trigger_123".to_string(),
        target_collector: "binary-hasher".to_string(),
        analysis_type: AnalysisType::BinaryHash,
        priority: TriggerPriority::High,
        target_pid: Some(1234),
        target_path: Some("/usr/bin/test".to_string()),
        correlation_id: "test_correlation_456".to_string(),
        metadata: {
            let mut metadata = HashMap::new();
            metadata.insert("test_key".to_string(), "test_value".to_string());
            metadata
        },
        timestamp: SystemTime::now(),
    }
}

#[tokio::test]
async fn test_trigger_emission_system_integration() {
    // Create trigger manager
    let config = TriggerConfig::default();
    let manager = TriggerManager::new(config);

    // Register collector capabilities
    let capabilities = create_test_capabilities();
    manager
        .register_collector_capabilities(capabilities)
        .unwrap();

    // Test trigger request validation
    let trigger = create_test_trigger_request();
    let validation_result = manager.validate_trigger_request(&trigger).await;
    assert!(
        validation_result.is_ok(),
        "Trigger validation should succeed"
    );

    // Test timeout tracking
    let timeout_result = manager.track_trigger_timeout(&trigger).await;
    assert!(timeout_result.is_ok(), "Timeout tracking should succeed");

    // Test statistics collection
    let stats = manager.get_statistics();
    assert_eq!(stats.registered_capabilities, 1);
    assert_eq!(stats.emission_stats.total_emitted, 0);

    // Test trigger completion
    let completion_result = manager.complete_trigger_request(&trigger.trigger_id).await;
    assert!(
        completion_result.is_ok(),
        "Trigger completion should succeed"
    );

    println!("✅ Trigger emission system integration test passed");
}

#[tokio::test]
async fn test_trigger_validation_errors() {
    let config = TriggerConfig::default();
    let manager = TriggerManager::new(config);

    // Register collector capabilities
    let capabilities = create_test_capabilities();
    manager
        .register_collector_capabilities(capabilities)
        .unwrap();

    // Test validation with unknown collector
    let mut invalid_trigger = create_test_trigger_request();
    invalid_trigger.target_collector = "unknown-collector".to_string();

    let result = manager.validate_trigger_request(&invalid_trigger).await;
    assert!(
        result.is_err(),
        "Validation should fail for unknown collector"
    );

    // Test validation with unsupported analysis type
    let mut invalid_trigger = create_test_trigger_request();
    invalid_trigger.analysis_type = AnalysisType::Custom("unsupported".to_string());

    let result = manager.validate_trigger_request(&invalid_trigger).await;
    assert!(
        result.is_err(),
        "Validation should fail for unsupported analysis type"
    );

    println!("✅ Trigger validation error handling test passed");
}

#[tokio::test]
async fn test_timeout_handling() {
    let config = TriggerConfig::default();
    let manager = TriggerManager::new(config);

    // Register collector capabilities
    let capabilities = create_test_capabilities();
    manager
        .register_collector_capabilities(capabilities)
        .unwrap();

    // Create and track a trigger
    let trigger = create_test_trigger_request();
    let trigger_id = trigger.trigger_id.clone();

    manager.track_trigger_timeout(&trigger).await.unwrap();

    // Verify trigger is being tracked
    assert!(manager.is_trigger_tracked(&trigger_id));

    // Complete the trigger
    manager.complete_trigger_request(&trigger_id).await.unwrap();

    // Verify trigger is no longer tracked
    assert!(!manager.is_trigger_tracked(&trigger_id));

    println!("✅ Timeout handling test passed");
}
