//! End-to-end integration tests for multi-collector coordination workflows.
//!
//! These tests validate the complete multi-process collector coordination system
//! including task distribution, capability-based routing, result aggregation,
//! load balancing, and failover mechanisms.

use collector_core::{
    capability_router::{CapabilityRouter, CollectorCapability, CollectorHealthStatus},
    event::{CollectionEvent, ProcessEvent},
    load_balancer::{LoadBalancer, LoadBalancerConfig, LoadBalancingStrategy},
    result_aggregator::{AggregationConfig, CollectorResult, ResultAggregator},
    source::SourceCaps,
    task_distributor::{DistributionTask, TaskDistributor, TaskPriority},
};
use daemoneye_eventbus::DaemoneyeBroker;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tempfile::tempdir;
use tokio::time::sleep;
use uuid::Uuid;

/// Test basic task distribution to multiple collector types
#[tokio::test]
async fn test_multi_collector_task_distribution() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-distribution.sock");

    // Create broker
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_str().unwrap())
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    // Create task distributor
    let distributor = TaskDistributor::new(Arc::clone(&broker));

    // Create tasks for different collector types
    let process_event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "test-process".to_string(),
        command_line: vec![],
        executable_path: None,
        ppid: None,
        start_time: Some(SystemTime::now()),
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: None,
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    let task = distributor
        .create_task_from_event(
            &process_event,
            TaskPriority::Normal,
            Duration::from_secs(30),
        )
        .unwrap();

    // Distribute task
    let result = distributor.distribute_task(task).await;
    assert!(result.is_ok());

    // Verify statistics
    let stats = distributor.get_stats().await;
    assert_eq!(stats.tasks_distributed, 1);
    assert_eq!(stats.tasks_delivered, 1);

    broker.shutdown().await.unwrap();
}

/// Test capability-based routing with multiple collectors
#[tokio::test]
async fn test_capability_based_routing() {
    let router = CapabilityRouter::new(Duration::from_secs(30));

    // Register collectors with different capabilities
    let process_collector = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        capabilities: SourceCaps::PROCESS | SourceCaps::REALTIME,
        topic_patterns: vec!["control.collector.process".to_string()],
        health_status: CollectorHealthStatus::Healthy,
        last_heartbeat: SystemTime::now(),
        metadata: HashMap::new(),
    };

    let network_collector = CollectorCapability {
        collector_id: "netmond-1".to_string(),
        collector_type: "netmond".to_string(),
        capabilities: SourceCaps::NETWORK | SourceCaps::REALTIME,
        topic_patterns: vec!["control.collector.network".to_string()],
        health_status: CollectorHealthStatus::Healthy,
        last_heartbeat: SystemTime::now(),
        metadata: HashMap::new(),
    };

    router.register_collector(process_collector).await.unwrap();
    router.register_collector(network_collector).await.unwrap();

    // Route process task
    let process_decision = router.route_task(SourceCaps::PROCESS).await.unwrap();
    assert_eq!(process_decision.collector_id, "procmond-1");
    assert_eq!(process_decision.target_topic, "control.collector.process");

    // Route network task
    let network_decision = router.route_task(SourceCaps::NETWORK).await.unwrap();
    assert_eq!(network_decision.collector_id, "netmond-1");
    assert_eq!(network_decision.target_topic, "control.collector.network");

    // Verify routing stats
    let stats = router.get_routing_stats().await;
    assert_eq!(stats.total_collectors, 2);
    assert_eq!(stats.healthy_collectors, 2);
}

/// Test result aggregation from multiple collectors
#[tokio::test]
async fn test_result_aggregation() {
    let config = AggregationConfig::default();
    let aggregator = ResultAggregator::new(config);

    let correlation_id = Uuid::new_v4().to_string();

    // Start aggregation expecting 3 results
    aggregator
        .start_aggregation(correlation_id.clone(), Some(3))
        .await
        .unwrap();

    // Add results from different collectors
    for i in 1..=3 {
        let result = CollectorResult {
            collector_id: format!("collector-{}", i),
            events: vec![CollectionEvent::Process(ProcessEvent {
                pid: 1000 + i as u32,
                name: format!("process-{}", i),
                command_line: vec![],
                executable_path: None,
                ppid: None,
                start_time: Some(SystemTime::now()),
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                user_id: None,
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            })],
            timestamp: SystemTime::now(),
            processing_duration: Duration::from_millis(100),
            metadata: HashMap::new(),
        };

        let completed = aggregator
            .add_result(&correlation_id, result)
            .await
            .unwrap();

        if i == 3 {
            // Should be complete on third result
            assert!(completed.is_some());
            let aggregated = completed.unwrap();
            assert_eq!(aggregated.results.len(), 3);
        } else {
            assert!(completed.is_none());
        }
    }

    // Verify statistics
    let stats = aggregator.get_stats().await;
    assert_eq!(stats.aggregations_completed, 1);
    assert_eq!(stats.total_results_aggregated, 3);
}

/// Test load balancing across multiple collector instances
#[tokio::test]
async fn test_load_balancing() {
    let config = LoadBalancerConfig {
        strategy: LoadBalancingStrategy::LeastConnections,
        ..Default::default()
    };
    let balancer = LoadBalancer::new(config);

    // Register multiple collectors
    for i in 1..=3 {
        balancer
            .register_collector(format!("collector-{}", i), 1.0)
            .await
            .unwrap();
    }

    let collectors = create_test_collectors(3);

    // Distribute tasks and verify load balancing
    let mut selections = HashMap::new();
    for _ in 0..9 {
        let selected = balancer
            .select_collector(SourceCaps::PROCESS, &collectors)
            .await
            .unwrap();
        *selections.entry(selected).or_insert(0) += 1;
    }

    // Each collector should get approximately equal tasks (3 each)
    for count in selections.values() {
        assert!(*count >= 2 && *count <= 4); // Allow some variance
    }

    // Verify statistics
    let stats = balancer.get_stats().await;
    assert_eq!(stats.total_tasks_distributed, 9);
}

/// Test failover mechanism when collector fails
#[tokio::test]
async fn test_failover_mechanism() {
    let config = LoadBalancerConfig {
        failover_threshold: 2,
        ..Default::default()
    };
    let balancer = LoadBalancer::new(config);

    // Register collectors
    balancer
        .register_collector("collector-1".to_string(), 1.0)
        .await
        .unwrap();
    balancer
        .register_collector("collector-2".to_string(), 1.0)
        .await
        .unwrap();

    // Record failures for collector-1
    let should_failover1 = balancer.record_failure("collector-1").await.unwrap();
    assert!(!should_failover1);

    let should_failover2 = balancer.record_failure("collector-1").await.unwrap();
    assert!(should_failover2);

    // Trigger failover - filter out the failed collector
    let all_collectors = create_test_collectors(2);
    let available_collectors: Vec<_> = all_collectors
        .iter()
        .filter(|c| c.collector_id != "collector-1")
        .cloned()
        .collect();

    let event = balancer
        .trigger_failover("collector-1", &available_collectors, SourceCaps::PROCESS)
        .await
        .unwrap();

    assert_eq!(event.failed_collector_id, "collector-1");
    assert_eq!(event.failover_collector_id, "collector-2");

    // Verify statistics
    let stats = balancer.get_stats().await;
    assert_eq!(stats.total_failovers, 1);
}

/// Test complete workflow: distribution -> routing -> aggregation
#[tokio::test]
async fn test_complete_coordination_workflow() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-workflow.sock");

    // Create broker
    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_str().unwrap())
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    // Create components
    let distributor = TaskDistributor::new(Arc::clone(&broker));
    let router = CapabilityRouter::new(Duration::from_secs(30));
    let aggregator = ResultAggregator::new(AggregationConfig::default());

    // Register collectors
    let collector1 = CollectorCapability {
        collector_id: "collector-1".to_string(),
        collector_type: "procmond".to_string(),
        capabilities: SourceCaps::PROCESS,
        topic_patterns: vec!["control.collector.process".to_string()],
        health_status: CollectorHealthStatus::Healthy,
        last_heartbeat: SystemTime::now(),
        metadata: HashMap::new(),
    };

    router.register_collector(collector1).await.unwrap();

    // Create and distribute task
    let event = CollectionEvent::Process(ProcessEvent {
        pid: 1234,
        name: "test-process".to_string(),
        command_line: vec![],
        executable_path: None,
        ppid: None,
        start_time: Some(SystemTime::now()),
        cpu_usage: None,
        memory_usage: None,
        executable_hash: None,
        user_id: None,
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    });

    let task = distributor
        .create_task_from_event(&event, TaskPriority::High, Duration::from_secs(30))
        .unwrap();

    let correlation_id = task.correlation_id.clone();

    // Route task
    let routing_decision = router.route_task(SourceCaps::PROCESS).await.unwrap();
    assert_eq!(routing_decision.collector_id, "collector-1");

    // Distribute task
    distributor.distribute_task(task).await.unwrap();

    // Start aggregation
    aggregator
        .start_aggregation(correlation_id.clone(), Some(1))
        .await
        .unwrap();

    // Simulate result from collector
    let result = CollectorResult {
        collector_id: "collector-1".to_string(),
        events: vec![event],
        timestamp: SystemTime::now(),
        processing_duration: Duration::from_millis(50),
        metadata: HashMap::new(),
    };

    let completed = aggregator
        .add_result(&correlation_id, result)
        .await
        .unwrap();
    assert!(completed.is_some());

    // Verify complete workflow
    let dist_stats = distributor.get_stats().await;
    assert_eq!(dist_stats.tasks_distributed, 1);

    let agg_stats = aggregator.get_stats().await;
    assert_eq!(agg_stats.aggregations_completed, 1);

    broker.shutdown().await.unwrap();
}

/// Test stale collector detection and health updates
#[tokio::test]
async fn test_stale_collector_detection() {
    let router = CapabilityRouter::new(Duration::from_millis(100));

    // Register collector with old heartbeat
    let collector = CollectorCapability {
        collector_id: "stale-collector".to_string(),
        collector_type: "procmond".to_string(),
        capabilities: SourceCaps::PROCESS,
        topic_patterns: vec!["control.collector.process".to_string()],
        health_status: CollectorHealthStatus::Healthy,
        last_heartbeat: SystemTime::now() - Duration::from_secs(1),
        metadata: HashMap::new(),
    };

    router.register_collector(collector).await.unwrap();

    // Wait for heartbeat to become stale
    sleep(Duration::from_millis(200)).await;

    // Check for stale collectors
    let stale = router.check_stale_collectors().await;
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0], "stale-collector");

    // Verify collector is marked unavailable
    let collector = router.get_collector("stale-collector").await.unwrap();
    assert_eq!(collector.health_status, CollectorHealthStatus::Unavailable);
}

/// Test aggregation timeout handling
#[tokio::test]
async fn test_aggregation_timeout() {
    let config = AggregationConfig {
        timeout: Duration::from_millis(100),
        ..Default::default()
    };
    let aggregator = ResultAggregator::new(config);

    let correlation_id = Uuid::new_v4().to_string();

    // Start aggregation expecting 2 results
    aggregator
        .start_aggregation(correlation_id.clone(), Some(2))
        .await
        .unwrap();

    // Add only one result
    let result = CollectorResult {
        collector_id: "collector-1".to_string(),
        events: vec![],
        timestamp: SystemTime::now(),
        processing_duration: Duration::from_millis(10),
        metadata: HashMap::new(),
    };

    aggregator
        .add_result(&correlation_id, result)
        .await
        .unwrap();

    // Wait for timeout
    sleep(Duration::from_millis(200)).await;

    // Check for timeouts
    let timed_out = aggregator.check_timeouts().await.unwrap();
    assert_eq!(timed_out.len(), 1);

    // Verify statistics
    let stats = aggregator.get_stats().await;
    assert_eq!(stats.aggregations_timed_out, 1);
}

/// Test task priority ordering in distribution
#[tokio::test]
async fn test_task_priority_ordering() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-priority.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(socket_path.to_str().unwrap())
            .await
            .unwrap(),
    );
    broker.start().await.unwrap();

    let distributor = TaskDistributor::new(Arc::clone(&broker));

    // Create tasks with different priorities
    let priorities = vec![
        TaskPriority::Low,
        TaskPriority::Critical,
        TaskPriority::Normal,
        TaskPriority::High,
    ];

    for priority in priorities {
        let task = DistributionTask {
            task_id: Uuid::new_v4().to_string(),
            priority,
            required_capabilities: SourceCaps::PROCESS,
            target_topic: "control.collector.process".to_string(),
            payload: vec![],
            correlation_id: Uuid::new_v4().to_string(),
            created_at: SystemTime::now(),
            timeout: Duration::from_secs(30),
            metadata: HashMap::new(),
        };

        distributor.distribute_task(task).await.unwrap();
    }

    // Verify all tasks were distributed
    let stats = distributor.get_stats().await;
    assert_eq!(stats.tasks_distributed, 4);

    broker.shutdown().await.unwrap();
}

/// Helper function to create test collectors
fn create_test_collectors(count: usize) -> Vec<CollectorCapability> {
    (1..=count)
        .map(|i| CollectorCapability {
            collector_id: format!("collector-{}", i),
            collector_type: "procmond".to_string(),
            capabilities: SourceCaps::PROCESS,
            topic_patterns: vec!["control.collector.process".to_string()],
            health_status: CollectorHealthStatus::Healthy,
            last_heartbeat: SystemTime::now(),
            metadata: HashMap::new(),
        })
        .collect()
}
