//! WAL (Write-Ahead Log) performance benchmarks using Criterion.
//!
//! This benchmark suite measures WAL write latency, batch throughput,
//! replay performance, and file rotation overhead.
//!
//! # Running benchmarks
//!
//! ```bash
//! cargo bench --package procmond --bench wal_benchmarks
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::print_stdout,
    clippy::use_debug,
    clippy::uninlined_format_args,
    clippy::missing_assert_message,
    clippy::semicolon_if_nothing_returned,
    clippy::explicit_iter_loop,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse,
    clippy::cast_lossless,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls
)]

#[allow(dead_code)]
#[path = "bench_helpers.rs"]
mod bench_helpers;

use bench_helpers::{create_large_event, create_minimal_event, create_test_event};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use procmond::wal::WriteAheadLog;
use std::hint::black_box;
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Benchmark WAL single write latency.
fn bench_wal_write_single(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_write_single");

    // Different event sizes
    let event_types = [("minimal", 0), ("standard", 1), ("large", 2)];

    for (event_type, type_id) in event_types.iter() {
        group.bench_function(BenchmarkId::new("write_latency", event_type), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let temp_dir = TempDir::new().expect("Failed to create temp dir");
                    // Use 10MB rotation threshold for benchmarks
                    let wal = WriteAheadLog::with_rotation_threshold(
                        temp_dir.path().to_path_buf(),
                        10 * 1024 * 1024,
                    )
                    .await
                    .expect("Failed to create WAL");

                    let event = match type_id {
                        0 => create_minimal_event(1),
                        1 => create_test_event(1),
                        _ => create_large_event(1),
                    };

                    let sequence = wal.write(event).await.expect("Failed to write");
                    black_box(sequence)
                })
            });
        });
    }

    group.finish();
}

/// Benchmark WAL batch write throughput.
fn bench_wal_write_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_write_throughput");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let batch_sizes = [100, 500, 1000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("batch_write", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        // Use larger rotation threshold for batch tests
                        let wal = WriteAheadLog::with_rotation_threshold(
                            temp_dir.path().to_path_buf(),
                            50 * 1024 * 1024,
                        )
                        .await
                        .expect("Failed to create WAL");

                        let start = std::time::Instant::now();
                        for i in 0..batch_size {
                            let event = create_test_event(i as u32);
                            wal.write(event).await.expect("Failed to write");
                        }
                        let duration = start.elapsed();

                        let rate = batch_size as f64 / duration.as_secs_f64();
                        if batch_size >= 1000 {
                            eprintln!(
                                "WAL write throughput: {} events in {:.2}ms, rate: {:.1} events/sec",
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

/// Benchmark WAL replay latency.
fn bench_wal_replay(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_replay");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let event_counts = [100, 500, 1000];

    for event_count in event_counts.iter() {
        group.throughput(Throughput::Elements(*event_count as u64));
        group.bench_with_input(
            BenchmarkId::new("replay_latency", event_count),
            event_count,
            |b, &event_count| {
                // Pre-populate WAL with events before each benchmark iteration
                b.iter(|| {
                    rt.block_on(async {
                        let temp_dir = TempDir::new().expect("Failed to create temp dir");
                        let wal = WriteAheadLog::with_rotation_threshold(
                            temp_dir.path().to_path_buf(),
                            50 * 1024 * 1024,
                        )
                        .await
                        .expect("Failed to create WAL");

                        // Write events
                        for i in 0..event_count {
                            let event = create_test_event(i as u32);
                            wal.write(event).await.expect("Failed to write");
                        }

                        // Measure replay
                        let start = std::time::Instant::now();
                        let events = wal.replay().await.expect("Failed to replay");
                        let duration = start.elapsed();

                        assert_eq!(events.len(), event_count);

                        let rate = events.len() as f64 / duration.as_secs_f64();
                        if event_count >= 1000 {
                            eprintln!(
                                "WAL replay: {} events in {:.2}ms, rate: {:.1} events/sec",
                                events.len(),
                                duration.as_millis(),
                                rate
                            );
                        }

                        black_box((duration, events.len()))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WAL file rotation time.
fn bench_wal_rotation(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_rotation");

    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    // Use a very small rotation threshold to trigger rotation
    let rotation_threshold = 10 * 1024; // 10KB - will rotate frequently

    group.bench_function("rotation_triggered", |b| {
        b.iter(|| {
            rt.block_on(async {
                let temp_dir = TempDir::new().expect("Failed to create temp dir");
                let wal = WriteAheadLog::with_rotation_threshold(
                    temp_dir.path().to_path_buf(),
                    rotation_threshold,
                )
                .await
                .expect("Failed to create WAL");

                // Write events until rotation happens multiple times
                // Track rotation by counting .wal files in directory
                let initial_file_count = std::fs::read_dir(temp_dir.path())
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wal"))
                            .count()
                    })
                    .unwrap_or_else(|_err| {
                        eprintln!("WARNING: Could not measure memory for current process");
                        0
                    });

                for i in 0..500 {
                    let event = create_large_event(i);
                    wal.write(event).await.expect("Failed to write");
                }

                // Count rotations by checking final file count
                let final_file_count = std::fs::read_dir(temp_dir.path())
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wal"))
                            .count()
                    })
                    .unwrap_or_else(|_err| {
                        eprintln!("WARNING: Could not measure memory for current process");
                        0
                    });

                let rotations_triggered = final_file_count.saturating_sub(initial_file_count);
                black_box(rotations_triggered)
            })
        });
    });

    group.finish();
}

criterion_group!(
    wal_benchmarks,
    bench_wal_write_single,
    bench_wal_write_throughput,
    bench_wal_replay,
    bench_wal_rotation
);

criterion_main!(wal_benchmarks);
