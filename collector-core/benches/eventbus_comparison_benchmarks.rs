//! Criterion benchmarks comparing daemoneye-eventbus vs crossbeam performance.
//!
//! This benchmark suite provides detailed performance measurements for the
//! migration from crossbeam to daemoneye-eventbus, enabling regression detection
//! and performance optimization tracking.

use collector_core::{
    DaemoneyeEventBus,
    event::{CollectionEvent, ProcessEvent},
    event_bus::{CorrelationMetadata, EventBus, EventBusConfig, EventSubscription, LocalEventBus},
    source::SourceCaps,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::future;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{runtime::Runtime, time::timeout};

/// Create test events for benchmarking
fn create_benchmark_events(count: usize) -> Vec<CollectionEvent> {
    (0..count)
        .map(|i| {
            CollectionEvent::Process(ProcessEvent {
                pid: 2000 + (i as u32),
                ppid: Some(1),
                name: format!("bench_process_{}", i),
                executable_path: Some(format!("/usr/bin/bench_{}", i)),
                command_line: vec![format!("bench_{}", i), "--perf".to_string()],
                start_time: Some(SystemTime::now()),
                cpu_usage: Some(2.0 + (i as f64 % 3.0)),
                memory_usage: Some(2 * 1024 * 1024 + (i as u64 * 1024)),
                executable_hash: Some(format!("bench_hash_{:08x}", i)),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: SystemTime::now(),
                platform_metadata: None,
            })
        })
        .collect()
}

/// Benchmark crossbeam LocalEventBus throughput
async fn benchmark_crossbeam_throughput(event_count: usize, subscriber_count: usize) -> f64 {
    let event_bus_config = EventBusConfig {
        max_subscribers: subscriber_count * 2,
        buffer_size: event_count * 2,
        enable_statistics: true,
    };

    let mut event_bus = LocalEventBus::new(event_bus_config);
    let events = create_benchmark_events(event_count);
    let events_received = Arc::new(AtomicUsize::new(0));

    // Create subscribers
    let mut receivers = Vec::new();
    for i in 0..subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("crossbeam_bench_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let receiver = event_bus.subscribe(subscription).await.unwrap();
        receivers.push(receiver);
    }

    // Start receiving tasks
    let mut receive_handles = Vec::new();
    for mut receiver in receivers.into_iter() {
        let events_received_clone = Arc::clone(&events_received);
        let expected_events = event_count;

        let handle = tokio::spawn(async move {
            let mut received_count = 0;
            while received_count < expected_events {
                match timeout(Duration::from_secs(2), receiver.recv()).await {
                    Ok(Some(_event)) => {
                        received_count += 1;
                        events_received_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        });
        receive_handles.push(handle);
    }

    // Measure publishing performance
    let start_time = std::time::Instant::now();

    for (i, event) in events.into_iter().enumerate() {
        let correlation_metadata = CorrelationMetadata::new(format!("crossbeam-benchmark-{}", i));
        let _ = event_bus.publish(event, correlation_metadata).await;
    }

    // Wait for all events to be received
    let _ = timeout(Duration::from_secs(10), future::join_all(receive_handles)).await;
    let total_duration = start_time.elapsed();

    let final_events_received = events_received.load(Ordering::Relaxed);
    if total_duration.as_secs_f64() > 0.0 {
        final_events_received as f64 / total_duration.as_secs_f64()
    } else {
        0.0
    }
}

/// Benchmark daemoneye-eventbus throughput
async fn benchmark_daemoneye_throughput(event_count: usize, subscriber_count: usize) -> f64 {
    let event_bus_config = EventBusConfig {
        max_subscribers: subscriber_count * 2,
        buffer_size: event_count * 2,
        enable_statistics: true,
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("bench_daemoneye.sock");
    let mut event_bus = DaemoneyeEventBus::new(event_bus_config, socket_path.to_str().unwrap())
        .await
        .unwrap();

    event_bus.start().await.unwrap();

    let events = create_benchmark_events(event_count);
    let events_received = Arc::new(AtomicUsize::new(0));

    // Create subscribers
    let mut receivers = Vec::new();
    for i in 0..subscriber_count {
        let subscription = EventSubscription {
            subscriber_id: format!("daemoneye_bench_{}", i),
            capabilities: SourceCaps::PROCESS,
            event_filter: None,
            correlation_filter: None,
            topic_patterns: Some(vec!["events.process.+".to_string()]),
            enable_wildcards: true,
            topic_filter: None,
        };

        let receiver = event_bus.subscribe(subscription).await.unwrap();
        receivers.push(receiver);
    }

    // Start receiving tasks
    let mut receive_handles = Vec::new();
    for mut receiver in receivers.into_iter() {
        let events_received_clone = Arc::clone(&events_received);
        let expected_events = event_count;

        let handle = tokio::spawn(async move {
            let mut received_count = 0;
            while received_count < expected_events {
                match timeout(Duration::from_secs(2), receiver.recv()).await {
                    Ok(Some(_event)) => {
                        received_count += 1;
                        events_received_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        });
        receive_handles.push(handle);
    }

    // Measure publishing performance
    let start_time = std::time::Instant::now();

    for (i, event) in events.into_iter().enumerate() {
        let correlation_metadata = CorrelationMetadata::new(format!("daemoneye-benchmark-{}", i));
        let _ = event_bus.publish(event, correlation_metadata).await;
    }

    // Wait for all events to be received
    let _ = timeout(Duration::from_secs(10), future::join_all(receive_handles)).await;
    let total_duration = start_time.elapsed();

    event_bus.shutdown().await.unwrap();

    let final_events_received = events_received.load(Ordering::Relaxed);
    if total_duration.as_secs_f64() > 0.0 {
        final_events_received as f64 / total_duration.as_secs_f64()
    } else {
        0.0
    }
}
/// Benchmark event bus throughput comparison
fn bench_eventbus_throughput_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_throughput_comparison");

    for event_count in [1000, 5000, 10000].iter() {
        group.throughput(Throughput::Elements(*event_count as u64));

        // Benchmark crossbeam LocalEventBus
        group.bench_with_input(
            BenchmarkId::new("crossbeam", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async { benchmark_crossbeam_throughput(event_count, 2).await })
                });
            },
        );

        // Benchmark daemoneye-eventbus
        group.bench_with_input(
            BenchmarkId::new("daemoneye", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async { benchmark_daemoneye_throughput(event_count, 2).await })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark subscriber scalability comparison
fn bench_subscriber_scalability_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("subscriber_scalability_comparison");

    let event_count = 2000; // Fixed event count
    for subscriber_count in [1, 2, 4, 8].iter() {
        group.throughput(Throughput::Elements(
            (event_count * subscriber_count) as u64,
        ));

        // Benchmark crossbeam LocalEventBus
        group.bench_with_input(
            BenchmarkId::new("crossbeam", subscriber_count),
            subscriber_count,
            |b, &subscriber_count| {
                b.iter(|| {
                    rt.block_on(async {
                        benchmark_crossbeam_throughput(event_count, subscriber_count).await
                    })
                });
            },
        );

        // Benchmark daemoneye-eventbus
        group.bench_with_input(
            BenchmarkId::new("daemoneye", subscriber_count),
            subscriber_count,
            |b, &subscriber_count| {
                b.iter(|| {
                    rt.block_on(async {
                        benchmark_daemoneye_throughput(event_count, subscriber_count).await
                    })
                });
            },
        );
    }
    group.finish();
}

/// Benchmark latency characteristics comparison
fn bench_latency_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("latency_comparison");
    group.sample_size(20); // Reduce sample size for latency-focused benchmarks

    let test_cases = [
        ("small_batch", 100),
        ("medium_batch", 1000),
        ("large_batch", 5000),
    ];

    for (test_name, event_count) in test_cases.iter() {
        // Benchmark crossbeam LocalEventBus latency
        group.bench_function(BenchmarkId::new("crossbeam", test_name), |b| {
            b.iter(|| rt.block_on(async { benchmark_crossbeam_throughput(*event_count, 1).await }));
        });

        // Benchmark daemoneye-eventbus latency
        group.bench_function(BenchmarkId::new("daemoneye", test_name), |b| {
            b.iter(|| rt.block_on(async { benchmark_daemoneye_throughput(*event_count, 1).await }));
        });
    }
    group.finish();
}

/// Benchmark memory efficiency comparison
fn bench_memory_efficiency_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("memory_efficiency_comparison");
    group.sample_size(10); // Reduce sample size for memory-focused benchmarks

    let event_count = 8000;
    let subscriber_count = 4;

    // Benchmark crossbeam memory efficiency
    group.bench_function("crossbeam_memory", |b| {
        b.iter(|| {
            rt.block_on(async {
                benchmark_crossbeam_throughput(event_count, subscriber_count).await
            })
        });
    });

    // Benchmark daemoneye-eventbus memory efficiency
    group.bench_function("daemoneye_memory", |b| {
        b.iter(|| {
            rt.block_on(async {
                benchmark_daemoneye_throughput(event_count, subscriber_count).await
            })
        });
    });

    group.finish();
}

criterion_group!(
    eventbus_comparison_benches,
    bench_eventbus_throughput_comparison,
    bench_subscriber_scalability_comparison,
    bench_latency_comparison,
    bench_memory_efficiency_comparison
);
criterion_main!(eventbus_comparison_benches);
