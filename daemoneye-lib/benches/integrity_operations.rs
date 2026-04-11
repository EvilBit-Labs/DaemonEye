#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::indexing_slicing
)]

//! Criterion benchmarks for the `daemoneye_lib::integrity` module.
//!
//! Measures streaming hash throughput across representative file sizes and
//! algorithm selections through the public [`MultiAlgorithmHasher`] API.
//! The goal is to guard the hot paths used by both procmond's inline
//! enumeration hashing and the triggered `BinaryHasherCollector`:
//!
//! - 1 KiB warm: <50 µs (guards the `spawn_blocking` threshold)
//! - 256 KiB warm, BLAKE3+SHA-256: <5 ms
//! - 4 MiB warm, BLAKE3+SHA-256: <100 ms
//!
//! Each benchmark hashes a real on-disk file through the engine's
//! `HashComputer` trait so the measurement path matches production —
//! including file open, metadata fstat, and cache lookup.

use criterion::{Criterion, criterion_group, criterion_main};
use daemoneye_lib::integrity::{HashAlgorithm, HashComputer, HasherConfig, MultiAlgorithmHasher};
use std::hint::black_box;
use std::io::Write;
use tempfile::NamedTempFile;
use tokio::runtime::Runtime;

/// Create a temp file filled with `size` bytes of deterministic data.
fn make_file(size: usize) -> NamedTempFile {
    let mut tmp = NamedTempFile::new().expect("create temp file");
    // Deterministic non-zero payload so compiler/hardware hashers cannot
    // short-circuit a zero-page fast path.
    let chunk: Vec<u8> = (0..=255_u8).cycle().take(size).collect();
    tmp.write_all(&chunk).expect("write temp file");
    tmp.flush().expect("flush temp file");
    tmp
}

/// Build a hasher with the requested algorithm list and a disabled cache so
/// each iteration hashes the file end-to-end.
fn build_hasher(algorithms: Vec<HashAlgorithm>) -> MultiAlgorithmHasher {
    let config = HasherConfig::default()
        .with_algorithms(algorithms)
        .with_cache_capacity(0);
    MultiAlgorithmHasher::new(config).expect("hasher config validates")
}

fn bench_hash_sha256_only_small(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let tmp = make_file(1024);
    let hasher = build_hasher(vec![HashAlgorithm::Sha256]);
    c.bench_function("integrity_hash_sha256_only_1kib", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    hasher
                        .compute(tmp.path())
                        .await
                        .expect("hash small sha256-only"),
                )
            });
        });
    });
}

fn bench_hash_multi_algo_small(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let tmp = make_file(1024);
    let hasher = build_hasher(vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3]);
    c.bench_function("integrity_hash_multi_algo_1kib", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    hasher
                        .compute(tmp.path())
                        .await
                        .expect("hash small multi-algo"),
                )
            });
        });
    });
}

fn bench_hash_multi_algo_medium(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let tmp = make_file(256 * 1024);
    let hasher = build_hasher(vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3]);
    c.bench_function("integrity_hash_multi_algo_256kib", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    hasher
                        .compute(tmp.path())
                        .await
                        .expect("hash medium multi-algo"),
                )
            });
        });
    });
}

fn bench_hash_multi_algo_large(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let tmp = make_file(4 * 1024 * 1024);
    let hasher = build_hasher(vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3]);
    c.bench_function("integrity_hash_multi_algo_4mib", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    hasher
                        .compute(tmp.path())
                        .await
                        .expect("hash large multi-algo"),
                )
            });
        });
    });
}

criterion_group!(
    benches,
    bench_hash_sha256_only_small,
    bench_hash_multi_algo_small,
    bench_hash_multi_algo_medium,
    bench_hash_multi_algo_large
);
criterion_main!(benches);
