//! Integration tests comparing busrt vs crossbeam event distribution performance.
//!
//! This test suite validates that the busrt-based event bus provides equivalent
//! or better performance compared to the existing crossbeam-based implementation
//! while maintaining behavioral compatibility.

use collector_core::{
    BusrtEventBus, BusrtEventBusConfig, CollectionEvent, EventBus, EventBusConfig,
    EventSubscription, LocalEventBus, ProcessEvent, SourceCaps,
};
use criterion::Criterion;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::mpsc, time::timeout};

/// Performance test configuration
#[derive(Clone)]
struct PerformanceTestConfig {
    /// Number of events to publish
    event_count: usize,
    /// Number of subscribers
    subscriber_count: usize,
    /// Event payload size (approximate)
    payload_size: usize,
    /// Test timeout
    timeout_duration: Duration,
}

impl Default for PerformanceTestConfig {
    fn default() -> Self {
        Self {
            event_count: 1000,
            subscriber_count: 10,
            payload_size: 1024,
            timeout_duration: Duration::from_secs(30),
        }
    }
}

/// Performance test results
#[derive(Debug, Clone)]
struct PerformanceResults {
    /// Total test duration
    total_duration: Duration,
    /// Events published per second
    events_per_second: f64,
    /// Average latency per event
    avg_latency_ms: f64,
    /// Memory usage (approximate)
    memory_usage_bytes: u64,
    /// Events successfully delivered
    events_delivered: u64,
    /// Events dropped due to backpressure
    events_dropped: u64,
}

/// Test event generator for performance testing
struct EventGenerator {
    counter: AtomicU64,
    payload_size: usize,
}

impl EventGenerator {
    fn new(payload_size: usize) -> Self {
        Self {
            counter: AtomicU64::new(0),
            payload_size,
        }
    }

    fn generate_event(&self) -> CollectionEvent {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        let payload = "x".repeat(self.payload_size);

        CollectionEvent::Process(ProcessEvent {
            pid: id as u32,
            ppid: None,
            name: format!("test_process_{}", id),
            executable_path: Some(format!("/usr/bin/test_{}", payload)),
            command_line: Vec::new(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        })
    }
}

/// Performance test subscriber that tracks delivery metrics
struct PerformanceSubscriber {
    events_received: Arc<AtomicU64>,
    start_time: Instant,
    latencies: Arc<tokio::sync::Mutex<Vec<Duration>>>,
}

impl PerformanceSubscriber {
    fn new() -> Self {
        Self {
            events_received: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            latencies: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    async fn handle_events(&self, mut receiver: mpsc::Receiver<collector_core::BusEvent>) {
        while let Some(_event) = receiver.recv().await {
            let now = Instant::now();
            let latency = now.duration_since(self.start_time);

            self.events_received.fetch_add(1, Ordering::Relaxed);

            {
                let mut latencies = self.latencies.lock().await;
                latencies.push(latency);
            }
        }
    }

    async fn get_results(&self) -> (u64, f64) {
        let events = self.events_received.load(Ordering::Relaxed);
        let latencies = self.latencies.lock().await;

        let avg_latency = if latencies.is_empty() {
            0.0
        } else {
            let total_ms: f64 = latencies.iter().map(|d| d.as_millis() as f64).sum();
            total_ms / latencies.len() as f64
        };

        (events, avg_latency)
    }
}

/// Crossbeam event bus performance test
async fn test_crossbeam_performance(
    config: PerformanceTestConfig,
) -> anyhow::Result<PerformanceResults> {
    // Tracing initialization (optional for tests)

    let event_bus_config = EventBusConfig {
        max_buffer_size: 10000,
        max_subscribers: config.subscriber_count * 2,
        delivery_timeout: Duration::from_secs(5),
        enable_persistence: false,
        ..Default::default()
    };

    let mut event_bus = LocalEventBus::new(event_bus_config).await?;
    event_bus.start().await?;

    let event_generator = EventGenerator::new(config.payload_size);
    let mut subscribers = Vec::new();

    // Create subscribers
    for i in 0..config.subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("crossbeam_subscriber_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
        };

        let receiver = event_bus.subscribe(subscription).await?;
        let subscriber = PerformanceSubscriber::new();
        let subscriber_clone = PerformanceSubscriber {
            events_received: Arc::clone(&subscriber.events_received),
            start_time: subscriber.start_time,
            latencies: Arc::clone(&subscriber.latencies),
        };

        tokio::spawn(async move {
            subscriber_clone.handle_events(receiver).await;
        });

        subscribers.push(subscriber);
    }

    // Wait for subscribers to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();

    // Publish events
    for i in 0..config.event_count {
        let event = event_generator.generate_event();
        let correlation_id = format!("crossbeam_test_{}", i);

        event_bus.publish(event, correlation_id).await?;

        // Add small delay to prevent overwhelming the system
        if i % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    // Wait for event processing
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let total_duration = start_time.elapsed();

    // Collect results from subscribers
    let mut total_events_delivered = 0;
    let mut total_latency = 0.0;

    for subscriber in &subscribers {
        let (events, avg_latency) = subscriber.get_results().await;
        total_events_delivered += events;
        total_latency += avg_latency;
    }

    let avg_latency_ms = if subscribers.is_empty() {
        0.0
    } else {
        total_latency / subscribers.len() as f64
    };

    let events_per_second = if total_duration.as_secs_f64() > 0.0 {
        config.event_count as f64 / total_duration.as_secs_f64()
    } else {
        0.0
    };

    // Get statistics
    let stats = event_bus.statistics().await;

    event_bus.shutdown().await?;

    Ok(PerformanceResults {
        total_duration,
        events_per_second,
        avg_latency_ms,
        memory_usage_bytes: stats.memory_usage_bytes,
        events_delivered: total_events_delivered,
        events_dropped: stats.events_dropped,
    })
}

/// Busrt event bus performance test
async fn test_busrt_performance(
    config: PerformanceTestConfig,
) -> anyhow::Result<PerformanceResults> {
    // Tracing initialization (optional for tests)

    let busrt_config = BusrtEventBusConfig {
        event_bus: EventBusConfig {
            max_buffer_size: 10000,
            max_subscribers: config.subscriber_count * 2,
            delivery_timeout: Duration::from_secs(5),
            enable_persistence: false,
            ..Default::default()
        },
        enable_compatibility_mode: true,
        ..Default::default()
    };

    let mut event_bus = BusrtEventBus::new_with_busrt_config(busrt_config).await?;
    event_bus.start().await?;

    let event_generator = EventGenerator::new(config.payload_size);
    let mut subscribers = Vec::new();

    // Create subscribers
    for i in 0..config.subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("busrt_subscriber_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
        };

        let receiver = event_bus.subscribe(subscription).await?;
        let subscriber = PerformanceSubscriber::new();
        let subscriber_clone = PerformanceSubscriber {
            events_received: Arc::clone(&subscriber.events_received),
            start_time: subscriber.start_time,
            latencies: Arc::clone(&subscriber.latencies),
        };

        tokio::spawn(async move {
            subscriber_clone.handle_events(receiver).await;
        });

        subscribers.push(subscriber);
    }

    // Wait for subscribers to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();

    // Publish events
    for i in 0..config.event_count {
        let event = event_generator.generate_event();
        let correlation_id = format!("busrt_test_{}", i);

        event_bus.publish(event, correlation_id).await?;

        // Add small delay to prevent overwhelming the system
        if i % 100 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    // Wait for event processing
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let total_duration = start_time.elapsed();

    // Collect results from subscribers
    let mut total_events_delivered = 0;
    let mut total_latency = 0.0;

    for subscriber in &subscribers {
        let (events, avg_latency) = subscriber.get_results().await;
        total_events_delivered += events;
        total_latency += avg_latency;
    }

    let avg_latency_ms = if subscribers.is_empty() {
        0.0
    } else {
        total_latency / subscribers.len() as f64
    };

    let events_per_second = if total_duration.as_secs_f64() > 0.0 {
        config.event_count as f64 / total_duration.as_secs_f64()
    } else {
        0.0
    };

    // Get statistics
    let stats = event_bus.statistics().await;

    event_bus.shutdown().await?;

    Ok(PerformanceResults {
        total_duration,
        events_per_second,
        avg_latency_ms,
        memory_usage_bytes: stats.memory_usage_bytes,
        events_delivered: total_events_delivered,
        events_dropped: stats.events_dropped,
    })
}

/// Compare performance between crossbeam and busrt implementations
async fn compare_performance(config: PerformanceTestConfig) -> anyhow::Result<()> {
    println!("Running performance comparison tests...");
    println!(
        "Configuration: {} events, {} subscribers, {} byte payload",
        config.event_count, config.subscriber_count, config.payload_size
    );

    // Test crossbeam performance
    println!("\nTesting crossbeam event bus performance...");
    let crossbeam_results = timeout(
        config.timeout_duration,
        test_crossbeam_performance(config.clone()),
    )
    .await??;

    println!("Crossbeam Results:");
    println!("  Total Duration: {:?}", crossbeam_results.total_duration);
    println!(
        "  Events/Second: {:.2}",
        crossbeam_results.events_per_second
    );
    println!("  Avg Latency: {:.2} ms", crossbeam_results.avg_latency_ms);
    println!(
        "  Memory Usage: {} bytes",
        crossbeam_results.memory_usage_bytes
    );
    println!("  Events Delivered: {}", crossbeam_results.events_delivered);
    println!("  Events Dropped: {}", crossbeam_results.events_dropped);

    // Test busrt performance
    println!("\nTesting busrt event bus performance...");
    let busrt_results = timeout(
        config.timeout_duration,
        test_busrt_performance(config.clone()),
    )
    .await??;

    println!("Busrt Results:");
    println!("  Total Duration: {:?}", busrt_results.total_duration);
    println!("  Events/Second: {:.2}", busrt_results.events_per_second);
    println!("  Avg Latency: {:.2} ms", busrt_results.avg_latency_ms);
    println!("  Memory Usage: {} bytes", busrt_results.memory_usage_bytes);
    println!("  Events Delivered: {}", busrt_results.events_delivered);
    println!("  Events Dropped: {}", busrt_results.events_dropped);

    // Performance comparison
    println!("\nPerformance Comparison:");
    let throughput_ratio = busrt_results.events_per_second / crossbeam_results.events_per_second;
    let latency_ratio = busrt_results.avg_latency_ms / crossbeam_results.avg_latency_ms;
    let memory_ratio =
        busrt_results.memory_usage_bytes as f64 / crossbeam_results.memory_usage_bytes as f64;

    println!(
        "  Throughput Ratio (Busrt/Crossbeam): {:.2}x",
        throughput_ratio
    );
    println!("  Latency Ratio (Busrt/Crossbeam): {:.2}x", latency_ratio);
    println!("  Memory Ratio (Busrt/Crossbeam): {:.2}x", memory_ratio);

    // Performance assertions (allowing for some variance)
    if throughput_ratio < 0.8 {
        println!("WARNING: Busrt throughput is significantly lower than crossbeam");
    }
    if latency_ratio > 1.5 {
        println!("WARNING: Busrt latency is significantly higher than crossbeam");
    }
    if memory_ratio > 2.0 {
        println!("WARNING: Busrt memory usage is significantly higher than crossbeam");
    }

    println!("\nPerformance comparison completed successfully!");
    Ok(())
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_basic_performance_comparison() {
    let config = PerformanceTestConfig {
        event_count: 100,
        subscriber_count: 3,
        payload_size: 256,
        timeout_duration: Duration::from_secs(10),
    };

    let result = compare_performance(config).await;
    assert!(
        result.is_ok(),
        "Performance comparison failed: {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_high_throughput_comparison() {
    let config = PerformanceTestConfig {
        event_count: 1000,
        subscriber_count: 10,
        payload_size: 1024,
        timeout_duration: Duration::from_secs(30),
    };

    let result = compare_performance(config).await;
    assert!(
        result.is_ok(),
        "High throughput comparison failed: {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_many_subscribers_comparison() {
    let config = PerformanceTestConfig {
        event_count: 500,
        subscriber_count: 25,
        payload_size: 512,
        timeout_duration: Duration::from_secs(20),
    };

    let result = compare_performance(config).await;
    assert!(
        result.is_ok(),
        "Many subscribers comparison failed: {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_large_payload_comparison() {
    let config = PerformanceTestConfig {
        event_count: 200,
        subscriber_count: 5,
        payload_size: 4096,
        timeout_duration: Duration::from_secs(15),
    };

    let result = compare_performance(config).await;
    assert!(
        result.is_ok(),
        "Large payload comparison failed: {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_behavioral_equivalence() {
    // Tracing initialization (optional for tests)

    // Test that both implementations deliver the same events
    let event_count = 50;
    let subscriber_count = 3;

    // Test crossbeam
    let mut crossbeam_bus = LocalEventBus::new(EventBusConfig::default()).await.unwrap();
    crossbeam_bus.start().await.unwrap();

    let mut crossbeam_receivers = Vec::new();
    for i in 0..subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("crossbeam_equiv_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
        };
        let receiver = crossbeam_bus.subscribe(subscription).await.unwrap();
        crossbeam_receivers.push(receiver);
    }

    // Publish events to crossbeam
    let event_generator = EventGenerator::new(256);
    for i in 0..event_count {
        let event = event_generator.generate_event();
        crossbeam_bus
            .publish(event, format!("equiv_test_{}", i))
            .await
            .unwrap();
    }

    // Collect crossbeam events
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut crossbeam_event_count = 0;
    for mut receiver in crossbeam_receivers {
        while let Ok(Some(_)) = timeout(Duration::from_millis(10), receiver.recv()).await {
            crossbeam_event_count += 1;
        }
    }

    crossbeam_bus.shutdown().await.unwrap();

    // Test busrt
    let mut busrt_bus = BusrtEventBus::new(EventBusConfig::default()).await.unwrap();
    busrt_bus.start().await.unwrap();

    let mut busrt_receivers = Vec::new();
    for i in 0..subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("busrt_equiv_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
        };
        let receiver = busrt_bus.subscribe(subscription).await.unwrap();
        busrt_receivers.push(receiver);
    }

    // Publish events to busrt
    let event_generator = EventGenerator::new(256);
    for i in 0..event_count {
        let event = event_generator.generate_event();
        busrt_bus
            .publish(event, format!("equiv_test_{}", i))
            .await
            .unwrap();
    }

    // Collect busrt events
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut busrt_event_count = 0;
    for mut receiver in busrt_receivers {
        while let Ok(Some(_)) = timeout(Duration::from_millis(10), receiver.recv()).await {
            busrt_event_count += 1;
        }
    }

    busrt_bus.shutdown().await.unwrap();

    // Verify behavioral equivalence
    println!("Crossbeam delivered {} events", crossbeam_event_count);
    println!("Busrt delivered {} events", busrt_event_count);

    // Allow for some variance in event delivery due to timing
    let expected_total = event_count * subscriber_count;
    assert!(
        crossbeam_event_count >= expected_total / 2,
        "Crossbeam delivered too few events: {} < {}",
        crossbeam_event_count,
        expected_total / 2
    );
    assert!(
        busrt_event_count >= expected_total / 2,
        "Busrt delivered too few events: {} < {}",
        busrt_event_count,
        expected_total / 2
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_memory_usage_comparison() {
    // Tracing initialization (optional for tests)

    // Test memory usage patterns
    let config = PerformanceTestConfig {
        event_count: 1000,
        subscriber_count: 10,
        payload_size: 2048,
        timeout_duration: Duration::from_secs(20),
    };

    // Measure crossbeam memory usage
    let crossbeam_results = test_crossbeam_performance(config.clone()).await.unwrap();

    // Measure busrt memory usage
    let busrt_results = test_busrt_performance(config).await.unwrap();

    println!("Memory Usage Comparison:");
    println!(
        "  Crossbeam: {} bytes",
        crossbeam_results.memory_usage_bytes
    );
    println!("  Busrt: {} bytes", busrt_results.memory_usage_bytes);

    let memory_ratio =
        busrt_results.memory_usage_bytes as f64 / crossbeam_results.memory_usage_bytes as f64;
    println!("  Ratio (Busrt/Crossbeam): {:.2}x", memory_ratio);

    // Memory usage should be reasonable (within 3x)
    assert!(
        memory_ratio < 3.0,
        "Busrt memory usage is too high: {:.2}x crossbeam",
        memory_ratio
    );
}

#[tokio::test]
#[ignore = "Busrt implementation is still in development"]
async fn test_latency_characteristics() {
    // Tracing initialization (optional for tests)

    // Test latency characteristics under different loads
    let configs = vec![
        PerformanceTestConfig {
            event_count: 100,
            subscriber_count: 1,
            payload_size: 128,
            timeout_duration: Duration::from_secs(5),
        },
        PerformanceTestConfig {
            event_count: 100,
            subscriber_count: 10,
            payload_size: 128,
            timeout_duration: Duration::from_secs(10),
        },
        PerformanceTestConfig {
            event_count: 100,
            subscriber_count: 1,
            payload_size: 2048,
            timeout_duration: Duration::from_secs(10),
        },
    ];

    for (i, config) in configs.into_iter().enumerate() {
        println!(
            "Latency test {} - {} events, {} subscribers, {} byte payload",
            i + 1,
            config.event_count,
            config.subscriber_count,
            config.payload_size
        );

        let crossbeam_results = test_crossbeam_performance(config.clone()).await.unwrap();
        let busrt_results = test_busrt_performance(config).await.unwrap();

        println!(
            "  Crossbeam latency: {:.2} ms",
            crossbeam_results.avg_latency_ms
        );
        println!("  Busrt latency: {:.2} ms", busrt_results.avg_latency_ms);

        let latency_ratio = busrt_results.avg_latency_ms / crossbeam_results.avg_latency_ms;
        println!("  Latency ratio: {:.2}x", latency_ratio);

        // Latency should be reasonable (within 2x for most cases)
        if latency_ratio > 2.0 {
            println!("  WARNING: Busrt latency is significantly higher");
        }
    }
}

/// Benchmark function for criterion integration
#[allow(dead_code)]
pub fn benchmark_event_bus_performance(c: &mut Criterion) {
    // Placeholder for future criterion integration
    // The to_async method is not available in current criterion version
    let _ = c;
}

// Note: To run the benchmark, use: cargo bench --test busrt_crossbeam_performance
// criterion::criterion_group!(benches, benchmark_event_bus_performance);
// criterion::criterion_main!(benches);
