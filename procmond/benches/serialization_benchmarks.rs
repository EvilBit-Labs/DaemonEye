//! Serialization and checksum performance benchmarks using Criterion.
//!
//! This benchmark suite measures postcard and JSON serialization performance,
//! CRC32 checksum computation, and batch serialization throughput.
//!
//! # Running benchmarks
//!
//! ```bash
//! cargo bench --package procmond --bench serialization_benchmarks
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::use_debug,
    clippy::uninlined_format_args,
    clippy::semicolon_if_nothing_returned,
    clippy::doc_markdown,
    clippy::explicit_iter_loop,
    clippy::pattern_type_mismatch,
    clippy::shadow_reuse
)]

#[allow(dead_code)]
#[path = "bench_helpers.rs"]
mod bench_helpers;

use bench_helpers::{create_large_event, create_minimal_event, create_test_event};
use collector_core::ProcessEvent;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use procmond::wal::WalEntry;
use std::hint::black_box;
use std::time::Duration;

/// Benchmark event serialization using postcard (as used by WAL).
fn bench_serialization_postcard(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_postcard");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        // Serialize benchmark
        group.bench_function(BenchmarkId::new("serialize", event_type), |b| {
            let entry = WalEntry::new(1, event.clone());
            b.iter(|| {
                let serialized = postcard::to_allocvec(&entry).expect("Failed to serialize");
                black_box(serialized)
            });
        });

        // Deserialize benchmark
        let entry = WalEntry::new(1, event.clone());
        let serialized = postcard::to_allocvec(&entry).expect("Failed to serialize");

        group.bench_function(BenchmarkId::new("deserialize", event_type), |b| {
            b.iter(|| {
                let deserialized: WalEntry =
                    postcard::from_bytes(&serialized).expect("Failed to deserialize");
                black_box(deserialized)
            });
        });
    }

    group.finish();
}

/// Benchmark event serialization using serde_json (for comparison and debugging).
fn bench_serialization_json(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_json");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        // Serialize benchmark
        group.bench_function(BenchmarkId::new("serialize", event_type), |b| {
            b.iter(|| {
                let serialized = serde_json::to_string(&event).expect("Failed to serialize");
                black_box(serialized)
            });
        });

        // Deserialize benchmark
        let serialized = serde_json::to_string(&event).expect("Failed to serialize");

        group.bench_function(BenchmarkId::new("deserialize", event_type), |b| {
            b.iter(|| {
                let deserialized: ProcessEvent =
                    serde_json::from_str(&serialized).expect("Failed to deserialize");
                black_box(deserialized)
            });
        });
    }

    group.finish();
}

/// Benchmark CRC32 checksum computation (used by WAL entries).
fn bench_checksum_computation(c: &mut Criterion) {
    use std::hash::Hasher;

    let mut group = c.benchmark_group("checksum_crc32c");

    let event_types = [
        ("minimal", create_minimal_event(1)),
        ("standard", create_test_event(1)),
        ("large", create_large_event(1)),
    ];

    for (event_type, event) in event_types.iter() {
        let serialized = postcard::to_allocvec(&event).expect("Failed to serialize");
        let data_size = serialized.len();

        group.throughput(Throughput::Bytes(data_size as u64));
        group.bench_function(BenchmarkId::new("compute", event_type), |b| {
            b.iter(|| {
                let mut crc = crc32c::Crc32cHasher::new(0);
                crc.write(&serialized);
                let checksum = crc.finish() as u32;
                black_box(checksum)
            });
        });
    }

    group.finish();
}

/// Benchmark batch serialization throughput.
fn bench_serialization_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization_throughput");

    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);

    let batch_sizes = [100, 1000, 5000];

    for batch_size in batch_sizes.iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));

        // Postcard batch serialization
        group.bench_with_input(
            BenchmarkId::new("postcard_batch", batch_size),
            batch_size,
            |b, &batch_size| {
                let events: Vec<ProcessEvent> = (0..batch_size)
                    .map(|i| create_test_event(i as u32))
                    .collect();

                b.iter(|| {
                    let mut total_bytes = 0_usize;
                    for event in &events {
                        let entry = WalEntry::new(1, event.clone());
                        let serialized =
                            postcard::to_allocvec(&entry).expect("Failed to serialize");
                        total_bytes = total_bytes.saturating_add(serialized.len());
                    }
                    black_box(total_bytes)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    serialization_benchmarks,
    bench_serialization_postcard,
    bench_serialization_json,
    bench_checksum_computation,
    bench_serialization_throughput
);

criterion_main!(serialization_benchmarks);
