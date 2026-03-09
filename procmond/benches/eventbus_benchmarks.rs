//! EventBus connector performance benchmarks using Criterion.
//!
//! This benchmark suite measures event buffering latency, throughput,
//! and WAL replay through the `EventBusConnector`.
//!
//! # Running benchmarks
//!
//! ```bash
//! cargo bench --package procmond --bench eventbus_benchmarks
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::print_stdout,
    clippy::use_debug,
    clippy::uninlined_format_args,
    clippy::semicolon_if_nothing_returned,
    clippy::doc_markdown,
    clippy::explicit_iter_loop,
    clippy::shadow_reuse,
    clippy::cast_lossless
)]

#[allow(dead_code)]
#[path = "bench_helpers.rs"]
mod bench_helpers;

use bench_helpers::create_test_event;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};
use std::hint::black_box;
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Benchmark event buffering latency (disconnected mode).
fn bench_eventbus_buffer_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_buffer");

    // Buffer single event
    group.bench_function("buffer_single_event", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                    .await
                    .expect("Failed to create connector");

                let event = create_test_event(1);
                let start = std::time::Instant::now();
                let sequence = connector
                    .publish(event, ProcessEventType::Start)
                    .await
                    .expect("Failed to publish");
                let duration = start.elapsed();

                black_box((sequence, duration))
            })
        });
    });

    group.finish();
}

/// Benchmark event buffering throughput.
fn bench_eventbus_buffer_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_buffer_throughput");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let batch_sizes = [100, 500, 1000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("buffer_batch", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                            .await
                            .expect("Failed to create connector");

                        let start = std::time::Instant::now();
                        for i in 0..batch_size {
                            let event = create_test_event(i as u32);
                            connector
                                .publish(event, ProcessEventType::Start)
                                .await
                                .expect("Failed to publish event in benchmark");
                        }
                        let duration = start.elapsed();

                        let rate = batch_size as f64 / duration.as_secs_f64();
                        if batch_size >= 500 {
                            eprintln!(
                                "EventBus buffer throughput: {} events in {:.2}ms, rate: {:.1} events/sec",
                                batch_size,
                                duration.as_millis(),
                                rate
                            );
                        }

                        black_box((duration, rate))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WAL replay through EventBusConnector.
fn bench_eventbus_wal_replay(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eventbus_wal_replay");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let event_counts = [100, 500, 1000];

    for event_count in event_counts.iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("wal_replay_through_connector", event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
                            .await
                            .expect("Failed to create connector");

                        // Publish events (they go to WAL and buffer)
                        for i in 0..event_count {
                            let event = create_test_event(i as u32);
                            connector
                                .publish(event, ProcessEventType::Start)
                                .await
                                .expect("Failed to publish");
                        }

                        // Measure replay time (while disconnected, events go to buffer)
                        let start = std::time::Instant::now();
                        let replayed = connector.replay_wal().await.expect("Failed to replay");
                        let duration = start.elapsed();

                        black_box((duration, replayed))
                    })
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    eventbus_benchmarks,
    bench_eventbus_buffer_operations,
    bench_eventbus_buffer_throughput,
    bench_eventbus_wal_replay
);

criterion_main!(eventbus_benchmarks);
