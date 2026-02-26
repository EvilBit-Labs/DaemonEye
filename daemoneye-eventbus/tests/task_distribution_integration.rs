//! Integration tests for task distribution and capability-based routing

use daemoneye_eventbus::{
    CollectorCapability, DaemoneyeBroker, RoutingStrategy, TaskDistributor, TaskRequest,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

#[tokio::test]
async fn test_task_distribution_with_multiple_collectors() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-multi-collectors.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Register multiple collectors
    for i in 1..=3 {
        let capability = CollectorCapability {
            collector_id: format!("procmond-{}", i),
            collector_type: "procmond".to_string(),
            supported_operations: vec!["enumerate_processes".to_string()],
            max_concurrent_tasks: 5,
            priority_levels: vec![1, 2, 3, 4, 5],
            metadata: HashMap::new(),
        };
        distributor.register_collector(capability).await.unwrap();
    }

    // Verify collectors are registered
    let stats = distributor.get_stats().await;
    assert_eq!(stats.active_collectors, 3);

    // Create and distribute a task
    let task = TaskRequest {
        task_id: "task-1".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 3,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: Some("test-correlation".to_string()),
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(task).await.unwrap();
    assert!(result.starts_with("procmond-"));

    // Verify task was distributed
    let stats = distributor.get_stats().await;
    assert_eq!(stats.tasks_distributed, 1);
}

#[tokio::test]
async fn test_capability_based_routing() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-capability-routing.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Register collectors with different capabilities
    let procmond_capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec![
            "enumerate_processes".to_string(),
            "scan_process".to_string(),
        ],
        max_concurrent_tasks: 5,
        priority_levels: vec![1, 2, 3],
        metadata: HashMap::new(),
    };

    let netmond_capability = CollectorCapability {
        collector_id: "netmond-1".to_string(),
        collector_type: "netmond".to_string(),
        supported_operations: vec!["enumerate_connections".to_string()],
        max_concurrent_tasks: 5,
        priority_levels: vec![1, 2, 3],
        metadata: HashMap::new(),
    };

    distributor
        .register_collector(procmond_capability)
        .await
        .unwrap();
    distributor
        .register_collector(netmond_capability)
        .await
        .unwrap();

    // Task for process enumeration should go to procmond
    let process_task = TaskRequest {
        task_id: "task-process".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(process_task).await.unwrap();
    assert_eq!(result, "procmond-1");

    // Task for network enumeration should go to netmond
    let network_task = TaskRequest {
        task_id: "task-network".to_string(),
        operation: "enumerate_connections".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(network_task).await.unwrap();
    assert_eq!(result, "netmond-1");
}

#[tokio::test]
async fn test_priority_queue_ordering() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-priority-queue.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Don't register any collectors - tasks will be queued

    // Create tasks with different priorities
    for priority in [1, 5, 3, 2, 4] {
        let task = TaskRequest {
            task_id: format!("task-priority-{}", priority),
            operation: "enumerate_processes".to_string(),
            priority,
            payload: vec![],
            timeout_ms: 30000,
            metadata: HashMap::new(),
            correlation_id: None,
            created_at: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
        };

        let result = distributor.distribute_task(task).await.unwrap();
        assert_eq!(result, "queued");
    }

    // Verify all tasks are queued
    let stats = distributor.get_stats().await;
    assert_eq!(stats.tasks_queued, 5);

    // Register a collector
    let capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec!["enumerate_processes".to_string()],
        max_concurrent_tasks: 10,
        priority_levels: vec![1, 2, 3, 4, 5],
        metadata: HashMap::new(),
    };
    distributor.register_collector(capability).await.unwrap();

    // Process the queue
    let processed = distributor.process_queue().await.unwrap();
    assert_eq!(processed, 5);

    // Verify queue is empty
    let stats = distributor.get_stats().await;
    assert_eq!(stats.tasks_queued, 0);
    assert_eq!(stats.tasks_distributed, 5);
}

#[tokio::test]
async fn test_routing_strategies() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-routing-strategies.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Register multiple collectors
    for i in 1..=3 {
        let capability = CollectorCapability {
            collector_id: format!("procmond-{}", i),
            collector_type: "procmond".to_string(),
            supported_operations: vec!["enumerate_processes".to_string()],
            max_concurrent_tasks: 10,
            priority_levels: vec![1, 2, 3],
            metadata: HashMap::new(),
        };
        distributor.register_collector(capability).await.unwrap();
    }

    // Test RoundRobin strategy
    distributor
        .set_routing_strategy(RoutingStrategy::RoundRobin)
        .await;

    let mut collectors_used = std::collections::HashSet::new();
    for i in 0..6 {
        let task = TaskRequest {
            task_id: format!("task-rr-{}", i),
            operation: "enumerate_processes".to_string(),
            priority: 2,
            payload: vec![],
            timeout_ms: 30000,
            metadata: HashMap::new(),
            correlation_id: None,
            created_at: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
        };

        let collector_id = distributor.distribute_task(task).await.unwrap();
        collectors_used.insert(collector_id);
    }

    // With round-robin, all collectors should be used
    assert_eq!(collectors_used.len(), 3);

    // Test FirstAvailable strategy
    distributor
        .set_routing_strategy(RoutingStrategy::FirstAvailable)
        .await;

    let task = TaskRequest {
        task_id: "task-first".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let collector_id = distributor.distribute_task(task).await.unwrap();
    assert!(collector_id.starts_with("procmond-"));
}

#[tokio::test]
async fn test_collector_deregistration() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-deregistration.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Register a collector
    let capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec!["enumerate_processes".to_string()],
        max_concurrent_tasks: 5,
        priority_levels: vec![1, 2, 3],
        metadata: HashMap::new(),
    };
    distributor.register_collector(capability).await.unwrap();

    let stats = distributor.get_stats().await;
    assert_eq!(stats.active_collectors, 1);

    // Deregister the collector
    distributor
        .deregister_collector("procmond-1")
        .await
        .unwrap();

    let stats = distributor.get_stats().await;
    assert_eq!(stats.active_collectors, 0);

    // Try to distribute a task - should be queued
    let task = TaskRequest {
        task_id: "task-after-dereg".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(task).await.unwrap();
    assert_eq!(result, "queued");
}

#[tokio::test]
async fn test_task_completion_tracking() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-completion.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );
    let distributor = TaskDistributor::new(Arc::clone(&broker)).await.unwrap();

    // Register a collector with limited capacity
    let capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec!["enumerate_processes".to_string()],
        max_concurrent_tasks: 2,
        priority_levels: vec![1, 2, 3],
        metadata: HashMap::new(),
    };
    distributor.register_collector(capability).await.unwrap();

    // Distribute tasks up to capacity
    for i in 1..=2 {
        let task = TaskRequest {
            task_id: format!("task-{}", i),
            operation: "enumerate_processes".to_string(),
            priority: 2,
            payload: vec![],
            timeout_ms: 30000,
            metadata: HashMap::new(),
            correlation_id: None,
            created_at: SystemTime::now(),
            deadline: SystemTime::now() + Duration::from_secs(30),
        };

        let result = distributor.distribute_task(task).await.unwrap();
        assert_eq!(result, "procmond-1");
    }

    // Next task should be queued (collector at capacity)
    let task = TaskRequest {
        task_id: "task-3".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(task).await.unwrap();
    assert_eq!(result, "queued");

    // Mark a task as completed
    distributor
        .task_completed("procmond-1", "task-1")
        .await
        .unwrap();

    // Process queue - should now succeed
    let processed = distributor.process_queue().await.unwrap();
    assert_eq!(processed, 1);
}

#[tokio::test]
async fn test_heartbeat_and_health_checks() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test-heartbeat.sock");

    let broker = Arc::new(
        DaemoneyeBroker::new(&socket_path.to_string_lossy())
            .await
            .unwrap(),
    );

    // Create distributor with short heartbeat timeout
    let distributor = TaskDistributor::with_config(
        Arc::clone(&broker),
        10000,
        3,
        Duration::from_millis(100), // Very short timeout for testing
        RoutingStrategy::LeastLoaded,
    )
    .await
    .unwrap();

    // Register a collector
    let capability = CollectorCapability {
        collector_id: "procmond-1".to_string(),
        collector_type: "procmond".to_string(),
        supported_operations: vec!["enumerate_processes".to_string()],
        max_concurrent_tasks: 5,
        priority_levels: vec![1, 2, 3],
        metadata: HashMap::new(),
    };
    distributor.register_collector(capability).await.unwrap();

    // Wait for heartbeat timeout
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Check health - collector should be marked unhealthy
    distributor.check_collector_health().await.unwrap();

    // Try to distribute a task - should be queued (no healthy collectors)
    let task = TaskRequest {
        task_id: "task-unhealthy".to_string(),
        operation: "enumerate_processes".to_string(),
        priority: 2,
        payload: vec![],
        timeout_ms: 30000,
        metadata: HashMap::new(),
        correlation_id: None,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(30),
    };

    let result = distributor.distribute_task(task).await.unwrap();
    assert_eq!(result, "queued");

    // Update heartbeat
    distributor.update_heartbeat("procmond-1").await.unwrap();

    // Check health again - collector should be available
    distributor.check_collector_health().await.unwrap();

    // Process queue - should now succeed
    let processed = distributor.process_queue().await.unwrap();
    assert_eq!(processed, 1);
}
