// Benchmarks intentionally use `.expect(...)` for setup failures: a
// benchmark harness that silently eats errors is worse than a loud panic,
// and there is no user-facing error path to preserve. Scoped crate-wide
// because nearly every function performs setup. Other clippy lints
// (uninlined_format_args, shadow_reuse, as_conversions, indexing_slicing)
// are NOT allowed here — if a future edit trips one, fix the code instead
// of silencing the warning.
#![allow(clippy::expect_used)]

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
#[cfg(feature = "fuzzy-hashes")]
use daemoneye_lib::integrity::fuzzy;
use daemoneye_lib::integrity::{HashAlgorithm, HashComputer, HasherConfig, MultiAlgorithmHasher};
use std::hint::black_box;
#[cfg(feature = "fuzzy-hashes")]
use std::io::Cursor;
use std::io::Write;
use tempfile::NamedTempFile;
use tokio::runtime::Runtime;

/// Deterministic non-zero byte payload of `size` bytes.
fn make_bytes(size: usize) -> Vec<u8> {
    (0..=255_u8).cycle().take(size).collect()
}

/// Create a temp file filled with `size` bytes of deterministic data.
fn make_file(size: usize) -> NamedTempFile {
    let mut tmp = NamedTempFile::new().expect("create temp file");
    // Deterministic non-zero payload so compiler/hardware hashers cannot
    // short-circuit a zero-page fast path.
    tmp.write_all(&make_bytes(size)).expect("write temp file");
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

/// ssdeep/CTPH fuzzy-hash cost in isolation, across the representative sizes,
/// so the added cost over the SHA-256-only baseline above is quantifiable for
/// the R2 AC4 sustained-CPU budget. Streams via `compute_ssdeep_from_reader`,
/// the same path procmond's hash pass uses.
///
/// Gated on `fuzzy-hashes`: with the feature off, `compute_ssdeep_from_reader`
/// is a stub that returns `FeatureDisabled`, so the bench would hard-fail.
#[cfg(feature = "fuzzy-hashes")]
fn bench_ssdeep_only(c: &mut Criterion) {
    for (label, size) in [
        ("1kib", 1024_usize),
        ("256kib", 256 * 1024),
        ("4mib", 4 * 1024 * 1024),
    ] {
        let bytes = make_bytes(size);
        c.bench_function(&format!("integrity_ssdeep_only_{label}"), |b| {
            b.iter(|| {
                black_box(
                    fuzzy::compute_ssdeep_from_reader(&mut Cursor::new(black_box(&bytes)))
                        .expect("ssdeep digest"),
                )
            });
        });
    }
}

/// Combined SHA-256 identity hash + ssdeep fuzzy hash, matching what procmond's
/// hash pass now computes per executable: SHA-256 over the file plus ssdeep over
/// the same bytes via `compute_ssdeep_from_reader` (the streaming path procmond
/// uses). Compare against `integrity_hash_sha256_only_*` to read the ssdeep
/// overhead end-to-end.
#[cfg(feature = "fuzzy-hashes")]
fn bench_sha256_plus_ssdeep_medium(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let size = 256 * 1024;
    let tmp = make_file(size);
    let bytes = make_bytes(size);
    let hasher = build_hasher(vec![HashAlgorithm::Sha256]);
    c.bench_function("integrity_sha256_plus_ssdeep_256kib", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(hasher.compute(tmp.path()).await.expect("sha256 hash"));
            });
            black_box(
                fuzzy::compute_ssdeep_from_reader(&mut Cursor::new(&bytes)).expect("ssdeep digest"),
            );
        });
    });
}

#[cfg(feature = "fuzzy-hashes")]
criterion_group!(
    benches,
    bench_hash_sha256_only_small,
    bench_hash_multi_algo_small,
    bench_hash_multi_algo_medium,
    bench_hash_multi_algo_large,
    bench_ssdeep_only,
    bench_sha256_plus_ssdeep_medium
);
#[cfg(not(feature = "fuzzy-hashes"))]
criterion_group!(
    benches,
    bench_hash_sha256_only_small,
    bench_hash_multi_algo_small,
    bench_hash_multi_algo_medium,
    bench_hash_multi_algo_large
);
criterion_main!(benches);
