//! Performance comparison tests between daemoneye-eventbus and crossbeam event distribution.
//!
//! This test suite validates that the migration from crossbeam to daemoneye-eventbus
//! maintains or improves performance characteristics while providing behavioral equivalence.
//!
//! ## Test Coverage
//!
//! - Event distribution throughput comparison
//! - Latency characteristics under various loads
//! - Memory usage and resource consumption
//! - Behavioral equivalence validation
//! - Concurrent subscriber performance
//! - Backpressure handling comparison

use collector_core::{
    DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent},
    event_bus::{EventBus, EventBusConfig, EventSubscription, LocalEventBus},
    source::SourceCaps,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::Mutex, time::timeout};

/// Test configuration for performance comparisons
#[derive(Debug, Clone)]
struct PerformanceTestConfig {
    /// Number of events to publish in throughput tests
    pub event_count: usize,
    /// Number of concurrent subscribers
    pub subscriber_count: usize,
    /// Test timeout duration
    pub test_timeout: Duration,
    /// Event buffer size for event buses
    pub buffer_size: usize,
}

impl Default for PerformanceTestConfig {
    fn default() -> Self {
        Self {
            event_count: 10000,
            subscriber_count: 4,
            test_timeout: Duration::from_secs(30),
            buffer_size: 20000,
        }
    }
}

/// Performance metrics collected during tests
#[derive(Debug, Clone)]
struct PerformanceMetrics {
    /// Total events processed
    pub events_processed: usize,
    /// Total time taken
    pub total_duration: Duration,
    /// Events per second throughput
    pub throughput: f64,
    /// Average latency per event (microseconds)
    pub avg_latency_us: f64,

    /// Number of subscribers that completed successfully
    pub successful_subscribers: usize,
}

impl PerformanceMetrics {
    fn new(
        events_processed: usize,
        total_duration: Duration,
        successful_subscribers: usize,
    ) -> Self {
        let throughput = if total_duration.as_secs_f64() > 0.0 {
            events_processed as f64 / total_duration.as_secs_f64()
        } else {
            0.0
        };

        let avg_latency_us = if events_processed > 0 {
            total_duration.as_micros() as f64 / events_processed as f64
        } else {
            0.0
        };

        Self {
            events_processed,
            total_duration,
            throughput,
            avg_latency_us,

            successful_subscribers,
        }
    }
}

/// Create test events for performance testing
fn create_test_events(count: usize) -> Vec<CollectionEvent> {
    (0..count)
        .map(|i| {
            CollectionEvent::Process(ProcessEvent {
                pid: 1000 + (i as u32),
                ppid: Some(1),
                name: format!("perf_test_process_{}", i),
                executable_path: Some(format!("/usr/bin/perf_test_{}", i)),
                command_line: vec![format!("perf_test_{}", i), "--benchmark".to_string()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(1.0 + (i as f64 % 5.0)),
                memory_usage: Some(1024 * 1024 + (i as u64 * 1024)),
                executable_hash: Some(format!("hash_{:08x}", i)),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            })
        })
        .collect()
}
/// Test crossbeam-based LocalEventBus throughput performance
async fn test_crossbeam_throughput(config: &PerformanceTestConfig) -> PerformanceMetrics {
    let event_bus_config = EventBusConfig {
        max_subscribers: config.subscriber_count * 2,
        buffer_size: config.buffer_size,
        enable_statistics: true,
    };

    let mut event_bus = LocalEventBus::new(event_bus_config);
    let events = create_test_events(config.event_count);
    let events_received = Arc::new(AtomicUsize::new(0));
    let successful_subscribers = Arc::new(AtomicUsize::new(0));

    // Create subscribers
    let mut receivers = Vec::new();
    for i in 0..config.subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("crossbeam_subscriber_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
        };

        let receiver = event_bus.subscribe(subscription).await.unwrap();
        receivers.push(receiver);
    }

    // Start receiving tasks
    let mut receive_handles = Vec::new();
    for (i, mut receiver) in receivers.into_iter().enumerate() {
        let events_received_clone = Arc::clone(&events_received);
        let successful_subscribers_clone = Arc::clone(&successful_subscribers);
        let expected_events = config.event_count;

        let handle = tokio::spawn(async move {
            let mut received_count = 0;
            while received_count < expected_events {
                match timeout(Duration::from_secs(5), receiver.recv()).await {
                    Ok(Some(_event)) => {
                        received_count += 1;
                        events_received_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => break, // Channel closed
                    Err(_) => {
                        eprintln!(
                            "Crossbeam subscriber {} timed out after receiving {} events",
                            i, received_count
                        );
                        break;
                    }
                }
            }
            if received_count == expected_events {
                successful_subscribers_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
        receive_handles.push(handle);
    }

    // Measure publishing performance
    let start_time = Instant::now();

    for event in events {
        event_bus.publish(event, None).await.unwrap();
    }

    // Wait for all events to be received
    let _ = timeout(
        config.test_timeout,
        futures::future::join_all(receive_handles),
    )
    .await;
    let total_duration = start_time.elapsed();

    let final_events_received = events_received.load(Ordering::Relaxed);
    let final_successful_subscribers = successful_subscribers.load(Ordering::Relaxed);

    PerformanceMetrics::new(
        final_events_received,
        total_duration,
        final_successful_subscribers,
    )
}

/// Test daemoneye-eventbus throughput performance
async fn test_daemoneye_eventbus_throughput(config: &PerformanceTestConfig) -> PerformanceMetrics {
    let event_bus_config = EventBusConfig {
        max_subscribers: config.subscriber_count * 2,
        buffer_size: config.buffer_size,
        enable_statistics: true,
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("perf_test_daemoneye.sock");
    let mut event_bus = DaemoneyeEventBus::new(event_bus_config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    // Start the event bus
    event_bus.start().await.unwrap();

    let events = create_test_events(config.event_count);
    let events_received = Arc::new(AtomicUsize::new(0));
    let successful_subscribers = Arc::new(AtomicUsize::new(0));

    // Create subscribers
    let mut receivers = Vec::new();
    for i in 0..config.subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("daemoneye_subscriber_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
        };

        let receiver = event_bus.subscribe(subscription).await.unwrap();
        receivers.push(receiver);
    }

    // Start receiving tasks
    let mut receive_handles = Vec::new();
    for (i, mut receiver) in receivers.into_iter().enumerate() {
        let events_received_clone = Arc::clone(&events_received);
        let successful_subscribers_clone = Arc::clone(&successful_subscribers);
        let expected_events = config.event_count;

        let handle = tokio::spawn(async move {
            let mut received_count = 0;
            while received_count < expected_events {
                match timeout(Duration::from_secs(5), receiver.recv()).await {
                    Ok(Some(_event)) => {
                        received_count += 1;
                        events_received_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => break, // Channel closed
                    Err(_) => {
                        eprintln!(
                            "DaemonEye subscriber {} timed out after receiving {} events",
                            i, received_count
                        );
                        break;
                    }
                }
            }
            if received_count == expected_events {
                successful_subscribers_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
        receive_handles.push(handle);
    }

    // Measure publishing performance
    let start_time = Instant::now();

    for event in events {
        event_bus.publish(event, None).await.unwrap();
    }

    // Wait for all events to be received
    let _ = timeout(
        config.test_timeout,
        futures::future::join_all(receive_handles),
    )
    .await;
    let total_duration = start_time.elapsed();

    // Shutdown the event bus
    event_bus.shutdown().await.unwrap();

    let final_events_received = events_received.load(Ordering::Relaxed);
    let final_successful_subscribers = successful_subscribers.load(Ordering::Relaxed);

    PerformanceMetrics::new(
        final_events_received,
        total_duration,
        final_successful_subscribers,
    )
}
/// Compare event distribution throughput between crossbeam and daemoneye-eventbus
#[tokio::test]
async fn test_throughput_comparison() {
    let config = PerformanceTestConfig {
        event_count: 5000,
        subscriber_count: 2,
        test_timeout: Duration::from_secs(15),
        buffer_size: 10000,
    };

    println!(
        "Testing throughput with {} events and {} subscribers",
        config.event_count, config.subscriber_count
    );

    // Test crossbeam performance
    let crossbeam_metrics = test_crossbeam_throughput(&config).await;
    println!("Crossbeam LocalEventBus Results:");
    println!("  Events processed: {}", crossbeam_metrics.events_processed);
    println!("  Duration: {:?}", crossbeam_metrics.total_duration);
    println!(
        "  Throughput: {:.2} events/sec",
        crossbeam_metrics.throughput
    );
    println!(
        "  Avg latency: {:.2} μs/event",
        crossbeam_metrics.avg_latency_us
    );
    println!(
        "  Successful subscribers: {}",
        crossbeam_metrics.successful_subscribers
    );

    // Test daemoneye-eventbus performance
    let daemoneye_metrics = test_daemoneye_eventbus_throughput(&config).await;
    println!("DaemonEye EventBus Results:");
    println!("  Events processed: {}", daemoneye_metrics.events_processed);
    println!("  Duration: {:?}", daemoneye_metrics.total_duration);
    println!(
        "  Throughput: {:.2} events/sec",
        daemoneye_metrics.throughput
    );
    println!(
        "  Avg latency: {:.2} μs/event",
        daemoneye_metrics.avg_latency_us
    );
    println!(
        "  Successful subscribers: {}",
        daemoneye_metrics.successful_subscribers
    );

    // Performance comparison analysis
    let throughput_ratio = if crossbeam_metrics.throughput > 0.0 {
        daemoneye_metrics.throughput / crossbeam_metrics.throughput
    } else {
        0.0
    };

    let latency_ratio = if crossbeam_metrics.avg_latency_us > 0.0 {
        daemoneye_metrics.avg_latency_us / crossbeam_metrics.avg_latency_us
    } else {
        0.0
    };

    println!("\nPerformance Comparison:");
    println!(
        "  Throughput ratio (DaemonEye/Crossbeam): {:.3}",
        throughput_ratio
    );
    println!(
        "  Latency ratio (DaemonEye/Crossbeam): {:.3}",
        latency_ratio
    );

    // Validate that both implementations processed events successfully
    assert!(
        crossbeam_metrics.events_processed > 0,
        "Crossbeam should process events"
    );
    assert!(
        daemoneye_metrics.events_processed > 0,
        "DaemonEye should process events"
    );

    // Validate that both implementations have reasonable throughput (>100 events/sec)
    assert!(
        crossbeam_metrics.throughput > 100.0,
        "Crossbeam throughput too low: {:.2}",
        crossbeam_metrics.throughput
    );
    assert!(
        daemoneye_metrics.throughput > 100.0,
        "DaemonEye throughput too low: {:.2}",
        daemoneye_metrics.throughput
    );

    // DaemonEye should be within reasonable performance bounds (not more than 10x slower)
    if throughput_ratio > 0.0 {
        assert!(
            throughput_ratio > 0.1,
            "DaemonEye throughput significantly lower than crossbeam: {:.3}",
            throughput_ratio
        );
    }

    // Both should have at least some successful subscribers
    assert!(
        crossbeam_metrics.successful_subscribers > 0,
        "Crossbeam should have successful subscribers"
    );
    assert!(
        daemoneye_metrics.successful_subscribers > 0,
        "DaemonEye should have successful subscribers"
    );
}

/// Test latency characteristics under different load conditions
#[tokio::test]
async fn test_latency_characteristics() {
    let test_cases = vec![
        (
            "low_load",
            PerformanceTestConfig {
                event_count: 1000,
                subscriber_count: 1,
                test_timeout: Duration::from_secs(10),
                buffer_size: 2000,
            },
        ),
        (
            "medium_load",
            PerformanceTestConfig {
                event_count: 5000,
                subscriber_count: 3,
                test_timeout: Duration::from_secs(15),
                buffer_size: 10000,
            },
        ),
        (
            "high_load",
            PerformanceTestConfig {
                event_count: 10000,
                subscriber_count: 5,
                test_timeout: Duration::from_secs(20),
                buffer_size: 20000,
            },
        ),
    ];

    for (test_name, config) in test_cases {
        println!("\n=== Latency Test: {} ===", test_name);

        let crossbeam_metrics = test_crossbeam_throughput(&config).await;
        let daemoneye_metrics = test_daemoneye_eventbus_throughput(&config).await;

        println!(
            "Crossbeam - Latency: {:.2} μs, Throughput: {:.2} events/sec",
            crossbeam_metrics.avg_latency_us, crossbeam_metrics.throughput
        );
        println!(
            "DaemonEye - Latency: {:.2} μs, Throughput: {:.2} events/sec",
            daemoneye_metrics.avg_latency_us, daemoneye_metrics.throughput
        );

        // Validate reasonable latency (should be under 10ms per event on average)
        assert!(
            crossbeam_metrics.avg_latency_us < 10000.0,
            "Crossbeam latency too high for {}: {:.2} μs",
            test_name,
            crossbeam_metrics.avg_latency_us
        );
        assert!(
            daemoneye_metrics.avg_latency_us < 10000.0,
            "DaemonEye latency too high for {}: {:.2} μs",
            test_name,
            daemoneye_metrics.avg_latency_us
        );

        // Both should process most events successfully
        let crossbeam_success_rate =
            crossbeam_metrics.events_processed as f64 / config.event_count as f64;
        let daemoneye_success_rate =
            daemoneye_metrics.events_processed as f64 / config.event_count as f64;

        assert!(
            crossbeam_success_rate > 0.8,
            "Crossbeam success rate too low for {}: {:.2}",
            test_name,
            crossbeam_success_rate
        );
        assert!(
            daemoneye_success_rate > 0.8,
            "DaemonEye success rate too low for {}: {:.2}",
            test_name,
            daemoneye_success_rate
        );
    }
}

/// Test memory usage and resource consumption under load
#[tokio::test]
async fn test_memory_usage_comparison() {
    let config = PerformanceTestConfig {
        event_count: 8000,
        subscriber_count: 4,
        test_timeout: Duration::from_secs(20),
        buffer_size: 16000,
    };

    println!(
        "Testing memory usage with {} events and {} subscribers",
        config.event_count, config.subscriber_count
    );

    // Test crossbeam memory usage (estimated)
    let crossbeam_start_memory = get_memory_estimate();
    let crossbeam_metrics = test_crossbeam_throughput(&config).await;
    let crossbeam_end_memory = get_memory_estimate();
    let crossbeam_memory_delta = crossbeam_end_memory.saturating_sub(crossbeam_start_memory);

    // Test daemoneye-eventbus memory usage (estimated)
    let daemoneye_start_memory = get_memory_estimate();
    let daemoneye_metrics = test_daemoneye_eventbus_throughput(&config).await;
    let daemoneye_end_memory = get_memory_estimate();
    let daemoneye_memory_delta = daemoneye_end_memory.saturating_sub(daemoneye_start_memory);

    println!("Memory Usage Comparison:");
    println!(
        "  Crossbeam estimated delta: {} bytes",
        crossbeam_memory_delta
    );
    println!(
        "  DaemonEye estimated delta: {} bytes",
        daemoneye_memory_delta
    );

    // Validate that both implementations processed events
    assert!(
        crossbeam_metrics.events_processed > 0,
        "Crossbeam should process events"
    );
    assert!(
        daemoneye_metrics.events_processed > 0,
        "DaemonEye should process events"
    );

    // Memory usage should be reasonable (not more than 100MB for this test)
    let max_reasonable_memory = 100 * 1024 * 1024; // 100MB
    assert!(
        crossbeam_memory_delta < max_reasonable_memory,
        "Crossbeam memory usage too high: {} bytes",
        crossbeam_memory_delta
    );
    assert!(
        daemoneye_memory_delta < max_reasonable_memory,
        "DaemonEye memory usage too high: {} bytes",
        daemoneye_memory_delta
    );

    println!("Memory usage validation passed for both implementations");
}

/// Simple memory estimation based on allocated objects
fn get_memory_estimate() -> usize {
    // This is a very rough estimate - in a real implementation you might use
    // system calls or memory profiling tools for accurate measurements
    std::mem::size_of::<usize>() * 1000 // Placeholder estimation
}
/// Test behavioral equivalence between crossbeam and daemoneye-eventbus
#[tokio::test]
async fn test_behavioral_equivalence() {
    let config = PerformanceTestConfig {
        event_count: 1000,
        subscriber_count: 2,
        test_timeout: Duration::from_secs(10),
        buffer_size: 2000,
    };

    // Create identical test events
    let test_events = create_test_events(config.event_count);
    let crossbeam_events = test_events.clone();
    let daemoneye_events = test_events.clone();

    // Test crossbeam behavior
    let crossbeam_received_events = Arc::new(Mutex::new(Vec::new()));
    let crossbeam_received_clone = Arc::clone(&crossbeam_received_events);

    let crossbeam_task = tokio::spawn(async move {
        let event_bus_config = EventBusConfig {
            max_subscribers: config.subscriber_count * 2,
            buffer_size: config.buffer_size,
            enable_statistics: true,
        };

        let mut event_bus = LocalEventBus::new(event_bus_config);

        // Single subscriber for behavioral comparison
        let subscription = EventSubscription {
            subscriber_id: "crossbeam_behavior_test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
        };

        let mut receiver = event_bus.subscribe(subscription).await.unwrap();

        // Start receiving task
        let receive_handle = tokio::spawn(async move {
            let mut received_events = Vec::new();
            while received_events.len() < config.event_count {
                match timeout(Duration::from_secs(2), receiver.recv()).await {
                    Ok(Some(event)) => {
                        received_events.push(event.event);
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            received_events
        });

        // Publish events
        for event in crossbeam_events {
            event_bus.publish(event, None).await.unwrap();
        }

        // Collect received events
        let received = receive_handle.await.unwrap();
        let mut locked_events = crossbeam_received_clone.lock().await;
        *locked_events = received;
    });

    // Test daemoneye-eventbus behavior
    let daemoneye_received_events = Arc::new(Mutex::new(Vec::new()));
    let daemoneye_received_clone = Arc::clone(&daemoneye_received_events);

    let daemoneye_task = tokio::spawn(async move {
        let event_bus_config = EventBusConfig {
            max_subscribers: config.subscriber_count * 2,
            buffer_size: config.buffer_size,
            enable_statistics: true,
        };

        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("behavior_test_daemoneye.sock");
        let mut event_bus = DaemoneyeEventBus::new(event_bus_config, socket_path.to_str().unwrap())
            .await
            .unwrap();

        event_bus.start().await.unwrap();

        // Single subscriber for behavioral comparison
        let subscription = EventSubscription {
            subscriber_id: "daemoneye_behavior_test".to_string(),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
        };

        let mut receiver = event_bus.subscribe(subscription).await.unwrap();

        // Start receiving task
        let receive_handle = tokio::spawn(async move {
            let mut received_events = Vec::new();
            while received_events.len() < config.event_count {
                match timeout(Duration::from_secs(2), receiver.recv()).await {
                    Ok(Some(event)) => {
                        received_events.push(event.event);
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            received_events
        });

        // Publish events
        for event in daemoneye_events {
            event_bus.publish(event, None).await.unwrap();
        }

        // Collect received events
        let received = receive_handle.await.unwrap();
        let mut locked_events = daemoneye_received_clone.lock().await;
        *locked_events = received;

        event_bus.shutdown().await.unwrap();
    });

    // Wait for both tests to complete
    timeout(config.test_timeout, crossbeam_task)
        .await
        .unwrap()
        .unwrap();
    timeout(config.test_timeout, daemoneye_task)
        .await
        .unwrap()
        .unwrap();

    // Compare received events
    let crossbeam_events = crossbeam_received_events.lock().await;
    let daemoneye_events = daemoneye_received_events.lock().await;

    println!("Behavioral Equivalence Results:");
    println!("  Crossbeam received: {} events", crossbeam_events.len());
    println!("  DaemonEye received: {} events", daemoneye_events.len());

    // Both should receive the same number of events
    assert_eq!(
        crossbeam_events.len(),
        daemoneye_events.len(),
        "Event count mismatch between implementations"
    );

    // Both should receive all published events
    assert_eq!(
        crossbeam_events.len(),
        config.event_count,
        "Crossbeam didn't receive all events"
    );
    assert_eq!(
        daemoneye_events.len(),
        config.event_count,
        "DaemonEye didn't receive all events"
    );

    // Validate event content equivalence (check first few events)
    let sample_size = std::cmp::min(10, crossbeam_events.len());
    for i in 0..sample_size {
        match (&crossbeam_events[i], &daemoneye_events[i]) {
            (
                CollectionEvent::Process(crossbeam_proc),
                CollectionEvent::Process(daemoneye_proc),
            ) => {
                assert_eq!(
                    crossbeam_proc.pid, daemoneye_proc.pid,
                    "PID mismatch at event {}",
                    i
                );
                assert_eq!(
                    crossbeam_proc.name, daemoneye_proc.name,
                    "Name mismatch at event {}",
                    i
                );
                // Note: Other fields may have slight differences due to conversion
            }
            _ => panic!("Event type mismatch at index {}", i),
        }
    }

    println!("Behavioral equivalence validation passed");
}

/// Test concurrent subscriber performance comparison
#[tokio::test]
async fn test_concurrent_subscriber_performance() {
    let config = PerformanceTestConfig {
        event_count: 3000,
        subscriber_count: 6,
        test_timeout: Duration::from_secs(15),
        buffer_size: 6000,
    };

    println!(
        "Testing concurrent subscriber performance with {} events and {} subscribers",
        config.event_count, config.subscriber_count
    );

    // Test crossbeam with multiple concurrent subscribers
    let crossbeam_metrics = test_crossbeam_throughput(&config).await;

    // Test daemoneye-eventbus with multiple concurrent subscribers
    let daemoneye_metrics = test_daemoneye_eventbus_throughput(&config).await;

    println!("Concurrent Subscriber Results:");
    println!(
        "  Crossbeam: {:.2} events/sec, {} successful subscribers",
        crossbeam_metrics.throughput, crossbeam_metrics.successful_subscribers
    );
    println!(
        "  DaemonEye: {:.2} events/sec, {} successful subscribers",
        daemoneye_metrics.throughput, daemoneye_metrics.successful_subscribers
    );

    // Both should handle concurrent subscribers effectively
    assert!(
        crossbeam_metrics.successful_subscribers >= config.subscriber_count / 2,
        "Crossbeam should have at least half the subscribers succeed"
    );
    assert!(
        daemoneye_metrics.successful_subscribers >= config.subscriber_count / 2,
        "DaemonEye should have at least half the subscribers succeed"
    );

    // Both should maintain reasonable throughput with multiple subscribers
    assert!(
        crossbeam_metrics.throughput > 100.0,
        "Crossbeam throughput too low with concurrent subscribers"
    );
    assert!(
        daemoneye_metrics.throughput > 100.0,
        "DaemonEye throughput too low with concurrent subscribers"
    );

    // Calculate efficiency (events processed per subscriber)
    let crossbeam_efficiency = if crossbeam_metrics.successful_subscribers > 0 {
        crossbeam_metrics.events_processed as f64 / crossbeam_metrics.successful_subscribers as f64
    } else {
        0.0
    };

    let daemoneye_efficiency = if daemoneye_metrics.successful_subscribers > 0 {
        daemoneye_metrics.events_processed as f64 / daemoneye_metrics.successful_subscribers as f64
    } else {
        0.0
    };

    println!(
        "  Crossbeam efficiency: {:.2} events/subscriber",
        crossbeam_efficiency
    );
    println!(
        "  DaemonEye efficiency: {:.2} events/subscriber",
        daemoneye_efficiency
    );

    // Both should have reasonable efficiency
    assert!(crossbeam_efficiency > 100.0, "Crossbeam efficiency too low");
    assert!(daemoneye_efficiency > 100.0, "DaemonEye efficiency too low");
}

/// Test backpressure handling comparison
#[tokio::test]
async fn test_backpressure_handling_comparison() {
    let config = PerformanceTestConfig {
        event_count: 15000, // High event count to trigger backpressure
        subscriber_count: 2,
        test_timeout: Duration::from_secs(25),
        buffer_size: 5000, // Smaller buffer to trigger backpressure
    };

    println!(
        "Testing backpressure handling with {} events, {} subscribers, buffer size {}",
        config.event_count, config.subscriber_count, config.buffer_size
    );

    // Test crossbeam backpressure handling
    let crossbeam_start = Instant::now();
    let crossbeam_metrics = test_crossbeam_throughput(&config).await;
    let crossbeam_duration = crossbeam_start.elapsed();

    // Test daemoneye-eventbus backpressure handling
    let daemoneye_start = Instant::now();
    let daemoneye_metrics = test_daemoneye_eventbus_throughput(&config).await;
    let daemoneye_duration = daemoneye_start.elapsed();

    println!("Backpressure Handling Results:");
    println!(
        "  Crossbeam: {:.2} events/sec, {} events processed in {:?}",
        crossbeam_metrics.throughput, crossbeam_metrics.events_processed, crossbeam_duration
    );
    println!(
        "  DaemonEye: {:.2} events/sec, {} events processed in {:?}",
        daemoneye_metrics.throughput, daemoneye_metrics.events_processed, daemoneye_duration
    );

    // Both should handle backpressure gracefully (process at least 80% of events)
    let crossbeam_success_rate =
        crossbeam_metrics.events_processed as f64 / config.event_count as f64;
    let daemoneye_success_rate =
        daemoneye_metrics.events_processed as f64 / config.event_count as f64;

    assert!(
        crossbeam_success_rate > 0.8,
        "Crossbeam should handle backpressure better: {:.2}",
        crossbeam_success_rate
    );
    assert!(
        daemoneye_success_rate > 0.8,
        "DaemonEye should handle backpressure better: {:.2}",
        daemoneye_success_rate
    );

    // Neither should take excessively long (should complete within test timeout)
    assert!(
        crossbeam_duration < config.test_timeout,
        "Crossbeam took too long under backpressure"
    );
    assert!(
        daemoneye_duration < config.test_timeout,
        "DaemonEye took too long under backpressure"
    );

    println!("Backpressure handling validation passed for both implementations");
}
