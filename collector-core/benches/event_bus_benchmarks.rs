//! Event bus performance benchmarks for regression testing.
//!
//! These benchmarks establish performance baselines for the event bus
//! and help detect regressions in critical paths. Expectations are
//! set conservatively to accommodate resource-constrained CI environments.

use collector_core::{
    CollectionEvent, EventBus, EventBusConfig, EventSubscription, LocalEventBus, ProcessEvent,
    SourceCaps,
};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::{
    hint::black_box as hint_black_box,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
use tokio::runtime::Runtime;

/// Creates a test process event for benchmarking.
fn create_test_event(pid: u32) -> CollectionEvent {
    CollectionEvent::Process(ProcessEvent {
        pid,
        ppid: Some(1),
        name: format!("bench_process_{}", pid),
        executable_path: Some(format!("/usr/bin/bench_{}", pid)),
        command_line: vec![format!("bench_{}", pid), "--test".to_owned()],
        start_time: Some(SystemTime::now()),
        cpu_usage: Some(1.5),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some("bench_hash".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        timestamp: SystemTime::now(),
        platform_metadata: None,
    })
}

/// Benchmark event bus initialization and startup.
///
/// Target: < 200ms on CI, < 100ms on dev machines
fn bench_event_bus_startup(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("event_bus_startup");

    // Reduce sample size since setup/teardown is expensive
    group.sample_size(10);

    group.bench_function("create_and_start", |b| {
        b.iter_batched(
            || {
                // Setup: create config
                EventBusConfig::default()
            },
            |config| {
                // Measurement: create, start, and shutdown
                rt.block_on(async {
                    let mut event_bus = LocalEventBus::new(config).await.unwrap();
                    event_bus.start().await.unwrap();
                    hint_black_box(&event_bus);
                    event_bus.shutdown().await.unwrap();
                })
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Benchmark subscription registration and unregistration.
///
/// Target: < 50ms per operation on CI, < 20ms on dev machines
fn bench_subscription_management(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("subscription_management");

    // Reduce sample size since setup/teardown is expensive
    group.sample_size(10);

    group.bench_function("subscribe_single", |b| {
        b.iter_batched(
            || {
                // Setup: create and start event bus
                rt.block_on(async {
                    let config = EventBusConfig::default();
                    let mut event_bus = LocalEventBus::new(config).await.unwrap();
                    event_bus.start().await.unwrap();
                    event_bus
                })
            },
            |mut event_bus| {
                // Measurement: subscribe and cleanup
                rt.block_on(async {
                    let subscription = EventSubscription {
                        subscriber_id: "bench-subscriber".to_string(),
                        capabilities: SourceCaps::PROCESS,
                        event_filter: None,
                        correlation_filter: None,
                        topic_patterns: None,
                        enable_wildcards: false,
                    };

                    let _receiver = event_bus.subscribe(subscription).await.unwrap();
                    hint_black_box(&event_bus);
                    event_bus.shutdown().await.unwrap();
                })
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("subscribe_and_unsubscribe", |b| {
        b.iter_batched(
            || {
                // Setup: create and start event bus
                rt.block_on(async {
                    let config = EventBusConfig::default();
                    let mut event_bus = LocalEventBus::new(config).await.unwrap();
                    event_bus.start().await.unwrap();
                    event_bus
                })
            },
            |mut event_bus| {
                // Measurement: subscribe, unsubscribe, and cleanup
                rt.block_on(async {
                    let subscription = EventSubscription {
                        subscriber_id: "bench-subscriber".to_string(),
                        capabilities: SourceCaps::PROCESS,
                        event_filter: None,
                        correlation_filter: None,
                        topic_patterns: None,
                        enable_wildcards: false,
                    };

                    let _receiver = event_bus.subscribe(subscription).await.unwrap();
                    event_bus.unsubscribe("bench-subscriber").await.unwrap();
                    hint_black_box(&event_bus);
                    event_bus.shutdown().await.unwrap();
                })
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Benchmark event publishing throughput.
///
/// Target: > 500 events/sec on CI, > 1000 events/sec on dev machines
fn bench_event_publishing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("event_publishing");

    for event_count in [10, 50, 100, 500].iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("publish_events", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let config = EventBusConfig::default();
                        let mut event_bus = LocalEventBus::new(config).await.unwrap();
                        event_bus.start().await.unwrap();

                        for i in 0..event_count {
                            let event = create_test_event(1000 + i as u32);
                            event_bus
                                .publish(event, format!("correlation_{}", i))
                                .await
                                .unwrap();
                        }

                        hint_black_box(&event_bus);

                        event_bus.shutdown().await.unwrap();
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark event delivery from publisher to subscriber.
///
/// Target: < 10ms end-to-end latency on CI, < 5ms on dev machines
fn bench_event_delivery(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("event_delivery");

    group.bench_function("single_event_roundtrip", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = EventBusConfig::default();
                let mut event_bus = LocalEventBus::new(config).await.unwrap();
                event_bus.start().await.unwrap();

                let subscription = EventSubscription {
                    subscriber_id: "bench-subscriber".to_string(),
                    capabilities: SourceCaps::PROCESS,
                    event_filter: None,
                    correlation_filter: None,
                    topic_patterns: None,
                    enable_wildcards: false,
                };

                let mut receiver = event_bus.subscribe(subscription).await.unwrap();

                // Publish event
                let event = create_test_event(1234);
                event_bus
                    .publish(event, "test_correlation".to_string())
                    .await
                    .unwrap();

                // Receive event (with timeout to avoid hanging)
                let received = tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                    .await
                    .unwrap();

                hint_black_box(&received);

                event_bus.shutdown().await.unwrap();
            })
        });
    });

    group.finish();
}

/// Benchmark multiple subscribers receiving the same events.
///
/// Target: Linear scaling up to 10 subscribers on CI, 20 on dev machines
fn bench_multi_subscriber_fanout(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("multi_subscriber_fanout");

    for subscriber_count in [1, 2, 5, 10].iter() {
        group.throughput(Throughput::Elements(*subscriber_count as u64));
        group.bench_with_input(
            BenchmarkId::new("fanout_to_subscribers", subscriber_count),
            subscriber_count,
            |b, &subscriber_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let config = EventBusConfig::default();
                        let mut event_bus = LocalEventBus::new(config).await.unwrap();
                        event_bus.start().await.unwrap();

                        // Create multiple subscribers
                        let mut receivers = Vec::new();
                        for i in 0..subscriber_count {
                            let subscription = EventSubscription {
                                subscriber_id: format!("bench-subscriber-{}", i),
                                capabilities: SourceCaps::PROCESS,
                                event_filter: None,
                                correlation_filter: None,
                                topic_patterns: None,
                                enable_wildcards: false,
                            };

                            let receiver = event_bus.subscribe(subscription).await.unwrap();
                            receivers.push(receiver);
                        }

                        // Publish single event
                        let event = create_test_event(5678);
                        event_bus
                            .publish(event, "test_correlation".to_string())
                            .await
                            .unwrap();

                        // Verify all subscribers received it
                        for mut receiver in receivers {
                            let _received =
                                tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                                    .await
                                    .unwrap();
                        }

                        event_bus.shutdown().await.unwrap();
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark sustained throughput with a single subscriber.
///
/// Target: > 500 events/sec on CI, > 1000 events/sec on dev machines
fn bench_sustained_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("sustained_throughput");

    for event_count in [50, 100, 200, 500].iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("sustained_events", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let config = EventBusConfig::default();
                        let mut event_bus = LocalEventBus::new(config).await.unwrap();
                        event_bus.start().await.unwrap();

                        let subscription = EventSubscription {
                            subscriber_id: "bench-subscriber".to_string(),
                            capabilities: SourceCaps::PROCESS,
                            event_filter: None,
                            correlation_filter: None,
                            topic_patterns: None,
                            enable_wildcards: false,
                        };

                        let mut receiver = event_bus.subscribe(subscription).await.unwrap();

                        // Publish events
                        for i in 0..event_count {
                            let event = create_test_event(2000 + i as u32);
                            event_bus
                                .publish(event, format!("correlation_{}", i))
                                .await
                                .unwrap();
                        }

                        // Drain receiver
                        let received_count = Arc::new(AtomicU64::new(0));
                        let received_clone = Arc::clone(&received_count);

                        let drain_handle = tokio::spawn(async move {
                            while let Ok(Some(_event)) =
                                tokio::time::timeout(Duration::from_millis(50), receiver.recv())
                                    .await
                            {
                                received_clone.fetch_add(1, Ordering::Relaxed);
                            }
                        });

                        let _ = drain_handle.await;

                        hint_black_box(received_count.load(Ordering::Relaxed));

                        event_bus.shutdown().await.unwrap();
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark statistics collection overhead.
///
/// Target: < 1ms per call on CI, < 0.5ms on dev machines
fn bench_statistics_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("statistics_overhead");

    group.bench_function("collect_statistics", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = EventBusConfig::default();
                let mut event_bus = LocalEventBus::new(config).await.unwrap();
                event_bus.start().await.unwrap();

                // Publish some events to generate statistics
                for i in 0..10 {
                    let event = create_test_event(3000 + i);
                    event_bus
                        .publish(event, format!("correlation_{}", i))
                        .await
                        .unwrap();
                }

                // Benchmark statistics collection
                let stats = event_bus.statistics().await;

                hint_black_box(&stats);

                event_bus.shutdown().await.unwrap();
            })
        });
    });

    group.finish();
}

/// Benchmark memory usage during high-load scenarios.
///
/// This is a sanity check rather than a strict regression test,
/// as memory usage can vary by platform and allocator.
fn bench_memory_pressure(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("memory_pressure");
    group.sample_size(10); // Reduce sample size for memory-intensive tests

    group.bench_function("high_event_volume", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = EventBusConfig {
                    max_buffer_size: 5000,
                    ..Default::default()
                };
                let mut event_bus = LocalEventBus::new(config).await.unwrap();
                event_bus.start().await.unwrap();

                let subscription = EventSubscription {
                    subscriber_id: "bench-subscriber".to_string(),
                    capabilities: SourceCaps::PROCESS,
                    event_filter: None,
                    correlation_filter: None,
                    topic_patterns: None,
                    enable_wildcards: false,
                };

                let mut receiver = event_bus.subscribe(subscription).await.unwrap();

                // Generate high event volume
                for i in 0..1000 {
                    let event = create_test_event(4000 + i);
                    event_bus
                        .publish(event, format!("correlation_{}", i))
                        .await
                        .unwrap();
                }

                // Drain receiver
                let drain_handle = tokio::spawn(async move {
                    while tokio::time::timeout(Duration::from_millis(10), receiver.recv())
                        .await
                        .is_ok()
                    {}
                });

                let _ = drain_handle.await;

                event_bus.shutdown().await.unwrap();
            })
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_event_bus_startup,
    bench_subscription_management,
    bench_event_publishing,
    bench_event_delivery,
    bench_multi_subscriber_fanout,
    bench_sustained_throughput,
    bench_statistics_overhead,
    bench_memory_pressure
);
criterion_main!(benches);
