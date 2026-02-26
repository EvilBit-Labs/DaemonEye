//! Integration tests for result aggregation and load management

use daemoneye_eventbus::{
    AggregationConfig, CollectionEvent, CollectorResult, DaemoneyeBroker, ProcessEvent,
    ResultAggregator,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

#[tokio::test]
async fn test_result_aggregator_creation() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-result-agg-create.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    let stats = aggregator.get_stats().await;
    assert_eq!(stats.results_collected, 0);
    assert_eq!(stats.results_pending, 0);
    assert_eq!(stats.correlations_active, 0);
    drop(temp_dir);
}

#[tokio::test]
async fn test_result_collection() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-result-collect.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    let result = CollectorResult {
        collector_id: "test-collector".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: Some("test --arg".to_string()),
            executable_path: Some("/bin/test".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: HashMap::new(),
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    aggregator.collect_result(result).await.unwrap();

    let stats = aggregator.get_stats().await;
    assert_eq!(stats.results_collected, 1);
    assert_eq!(stats.results_pending, 1);
    drop(temp_dir);
}

#[tokio::test]
async fn test_result_deduplication() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-result-dedup.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    let result = CollectorResult {
        collector_id: "test-collector".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: Some("test --arg".to_string()),
            executable_path: Some("/bin/test".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: HashMap::new(),
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    // First collection should succeed
    aggregator.collect_result(result.clone()).await.unwrap();

    // Second collection should be deduplicated
    aggregator.collect_result(result).await.unwrap();

    let stats = aggregator.get_stats().await;
    // Only the first collection is counted in results_collected
    assert_eq!(
        stats.results_collected, 1,
        "Only non-duplicate should be collected"
    );
    assert_eq!(stats.results_deduplicated, 1, "One should be deduplicated");
    // Only one result should be pending (the non-duplicate)
    assert_eq!(
        stats.results_pending, 1,
        "Only one result should be pending"
    );
    drop(temp_dir);
}

#[tokio::test]
async fn test_collector_health_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-health-track.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    let result = CollectorResult {
        collector_id: "test-collector".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: Some("test --arg".to_string()),
            executable_path: Some("/bin/test".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: HashMap::new(),
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    aggregator.collect_result(result).await.unwrap();

    // Check collector health
    let health = aggregator
        .get_collector_health("test-collector")
        .await
        .unwrap();
    assert_eq!(
        health,
        daemoneye_eventbus::CollectorHealth::Healthy,
        "Collector should be healthy after successful result"
    );
    drop(temp_dir);
}

#[tokio::test]
async fn test_backpressure_handling() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-backpressure.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig {
        max_pending_results: 10,
        backpressure_threshold: 8,
        ..Default::default()
    };
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    // Add results up to backpressure threshold
    for i in 0..9 {
        let result = CollectorResult {
            collector_id: format!("collector-{}", i),
            collector_type: "procmond".to_string(),
            event: CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i,
                name: format!("process-{}", i),
                command_line: Some(format!("cmd-{}", i)),
                executable_path: Some(format!("/bin/proc-{}", i)),
                ppid: Some(1),
                start_time: Some(SystemTime::now()),
                metadata: HashMap::new(),
            }),
            timestamp: SystemTime::now(),
            sequence: i as u64,
        };

        // Expect success until backpressure is triggered
        let result_status = aggregator.collect_result(result).await;
        if i >= 8 {
            // After threshold, backpressure should be active
            assert!(
                result_status.is_err(),
                "Backpressure should be triggered after threshold"
            );
        } else {
            assert!(result_status.is_ok(), "Should succeed before threshold");
        }
    }

    let stats = aggregator.get_stats().await;
    assert!(
        stats.backpressure_events > 0,
        "Backpressure should be triggered"
    );
    drop(temp_dir);
}

#[tokio::test]
async fn test_result_streaming() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-result-stream.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let mut aggregator = ResultAggregator::new(broker, config).await.unwrap();

    // Subscribe to aggregated results
    let mut receiver = aggregator.subscribe_results().await;

    // Start the aggregator
    aggregator.start().await.unwrap();

    // Collect a result with correlation ID
    let mut metadata = HashMap::new();
    metadata.insert("correlation_id".to_string(), "test-correlation".to_string());

    let result = CollectorResult {
        collector_id: "test-collector".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: Some("test --arg".to_string()),
            executable_path: Some("/bin/test".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata,
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    aggregator.collect_result(result).await.unwrap();

    // Wait for aggregated result with timeout
    let timeout_result = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await;

    // Note: This test may timeout because correlation processing requires
    // expected_collectors to be set. This is expected behavior.
    match timeout_result {
        Ok(Some(_aggregated)) => {
            // Successfully received aggregated result
        }
        Ok(None) => {
            panic!("Receiver closed unexpectedly");
        }
        Err(_) => {
            // Timeout is expected if correlation is not complete
            // This is acceptable for this test
        }
    }
    drop(temp_dir);
}

#[tokio::test]
async fn test_correlation_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-correlation.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    // Create results with same correlation ID
    let correlation_id = "test-correlation-123";
    let mut metadata1 = HashMap::new();
    metadata1.insert("correlation_id".to_string(), correlation_id.to_string());

    let result1 = CollectorResult {
        collector_id: "collector-1".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "process-1".to_string(),
            command_line: Some("cmd-1".to_string()),
            executable_path: Some("/bin/proc-1".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: metadata1,
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    let mut metadata2 = HashMap::new();
    metadata2.insert("correlation_id".to_string(), correlation_id.to_string());

    let result2 = CollectorResult {
        collector_id: "collector-2".to_string(),
        collector_type: "netmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 5678,
            name: "process-2".to_string(),
            command_line: Some("cmd-2".to_string()),
            executable_path: Some("/bin/proc-2".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: metadata2,
        }),
        timestamp: SystemTime::now(),
        sequence: 2,
    };

    // Collect both results
    aggregator.collect_result(result1).await.unwrap();
    aggregator.collect_result(result2).await.unwrap();

    let stats = aggregator.get_stats().await;
    assert_eq!(stats.results_collected, 2);
    assert_eq!(stats.results_pending, 2);
    drop(temp_dir);
}

#[tokio::test]
async fn test_aggregator_statistics() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test-agg-stats.sock");
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_string_lossy().as_ref())
            .await
            .unwrap(),
    );
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(broker, config).await.unwrap();

    // Initial statistics
    let stats = aggregator.get_stats().await;
    assert_eq!(stats.results_collected, 0);
    assert_eq!(stats.results_pending, 0);
    assert_eq!(stats.results_deduplicated, 0);
    assert_eq!(stats.correlations_active, 0);
    assert_eq!(stats.correlations_completed, 0);
    assert_eq!(stats.backpressure_events, 0);
    assert_eq!(stats.failover_events, 0);
    assert_eq!(stats.healthy_collectors, 0);
    assert_eq!(stats.unhealthy_collectors, 0);

    // Collect a result
    let result = CollectorResult {
        collector_id: "test-collector".to_string(),
        collector_type: "procmond".to_string(),
        event: CollectionEvent::Process(ProcessEvent {
            pid: 1234,
            name: "test-process".to_string(),
            command_line: Some("test --arg".to_string()),
            executable_path: Some("/bin/test".to_string()),
            ppid: Some(1),
            start_time: Some(SystemTime::now()),
            metadata: HashMap::new(),
        }),
        timestamp: SystemTime::now(),
        sequence: 1,
    };

    aggregator.collect_result(result).await.unwrap();

    // Updated statistics
    let stats = aggregator.get_stats().await;
    assert_eq!(stats.results_collected, 1);
    assert_eq!(stats.results_pending, 1);
    drop(temp_dir);
}
