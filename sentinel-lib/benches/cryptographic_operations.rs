#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::needless_collect,
    clippy::indexing_slicing,
    clippy::unseparated_literal_suffix,
    clippy::shadow_unrelated,
    clippy::option_if_let_else
)]

use blake3::Hasher as Blake3Hasher;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rs_merkle::{MerkleTree, algorithms::Sha256};
use sha2::{Digest, Sha256 as Sha256Hasher};
use std::hint::black_box;

/// Benchmark BLAKE3 hashing performance
fn bench_blake3_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake3_hashing");

    let test_data = b"This is test data for BLAKE3 hashing benchmark";

    group.bench_function("blake3_single_hash", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let mut hasher = Blake3Hasher::new();
            hasher.update(test_data);
            let hash = hasher.finalize();
            let duration = start.elapsed();

            black_box((hash, duration))
        });
    });

    group.bench_function("blake3_incremental_hash", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let mut hasher = Blake3Hasher::new();

            // Simulate incremental hashing
            for chunk in test_data.chunks(8) {
                hasher.update(chunk);
            }

            let hash = hasher.finalize();
            let duration = start.elapsed();

            black_box((hash, duration))
        });
    });

    // Test with different data sizes
    let data_sizes = vec![100, 1000, 10_000, 100_000, 1_000_000];

    for data_size in data_sizes {
        group.bench_with_input(
            BenchmarkId::new("blake3_data_size", data_size),
            &data_size,
            |b, &size| {
                let test_data_bytes = vec![0_u8; size];

                b.iter(|| {
                    let start = std::time::Instant::now();
                    let mut hasher = Blake3Hasher::new();
                    hasher.update(&test_data_bytes);
                    let hash = hasher.finalize();
                    let duration = start.elapsed();

                    let throughput = size as f64 / duration.as_secs_f64();
                    black_box((hash, throughput))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark SHA-256 hashing for comparison
fn bench_sha256_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("sha256_hashing");

    let test_data = b"This is test data for SHA-256 hashing benchmark";

    group.bench_function("sha256_single_hash", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let mut hasher = Sha256Hasher::new();
            hasher.update(test_data);
            let hash = hasher.finalize();
            let duration = start.elapsed();

            black_box((hash, duration))
        });
    });

    // Test with different data sizes
    let data_sizes = vec![100, 1000, 10_000, 100_000, 1_000_000];

    for data_size in data_sizes {
        group.bench_with_input(
            BenchmarkId::new("sha256_data_size", data_size),
            &data_size,
            |b, &size| {
                let test_data_bytes = vec![0_u8; size];

                b.iter(|| {
                    let start = std::time::Instant::now();
                    let mut hasher = Sha256Hasher::new();
                    hasher.update(&test_data_bytes);
                    let hash = hasher.finalize();
                    let duration = start.elapsed();

                    let throughput = size as f64 / duration.as_secs_f64();
                    black_box((hash, throughput))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark Merkle tree operations
fn bench_merkle_tree_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_tree_operations");

    // Test with different tree sizes
    let tree_sizes = vec![10, 100, 1000, 10000];

    for tree_size in tree_sizes {
        group.bench_with_input(
            BenchmarkId::new("merkle_tree_build", tree_size),
            &tree_size,
            |b, &size| {
                b.iter(|| {
                    let start = std::time::Instant::now();

                    // Create test data
                    let leaves: Vec<[u8; 32]> = (0..size)
                        .map(|i| {
                            let mut hasher = Blake3Hasher::new();
                            hasher.update(format!("leaf_{i}").as_bytes());
                            hasher.finalize().into()
                        })
                        .collect();

                    // Build Merkle tree
                    let tree = MerkleTree::<Sha256>::from_leaves(&leaves);
                    let duration = start.elapsed();

                    black_box((tree, duration))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark Merkle proof generation and verification
fn bench_merkle_proof_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_proof_operations");

    // Test with different tree sizes
    let tree_sizes = vec![100, 1000, 10000];

    for tree_size in tree_sizes {
        group.bench_with_input(
            BenchmarkId::new("merkle_proof_generation", tree_size),
            &tree_size,
            |b, &size| {
                b.iter(|| {
                    let start = std::time::Instant::now();

                    // Create test data
                    let leaves: Vec<[u8; 32]> = (0..size)
                        .map(|i| {
                            let mut hasher = Blake3Hasher::new();
                            hasher.update(format!("leaf_{i}").as_bytes());
                            hasher.finalize().into()
                        })
                        .collect();

                    // Build Merkle tree
                    let tree = MerkleTree::<Sha256>::from_leaves(&leaves);

                    // Generate proof for first leaf
                    let proof = tree.proof(&[0]);
                    let duration = start.elapsed();

                    black_box((proof, duration))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark Merkle proof verification
fn bench_merkle_proof_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_proof_verification");

    // Test with different tree sizes
    let tree_sizes = vec![100, 1000, 10000];

    for tree_size in tree_sizes {
        group.bench_with_input(
            BenchmarkId::new("merkle_proof_verification", tree_size),
            &tree_size,
            |b, &size| {
                b.iter(|| {
                    let start = std::time::Instant::now();

                    // Create test data
                    let leaves: Vec<[u8; 32]> = (0..size)
                        .map(|i| {
                            let mut hasher = Blake3Hasher::new();
                            hasher.update(format!("leaf_{i}").as_bytes());
                            hasher.finalize().into()
                        })
                        .collect();

                    // Build Merkle tree
                    let tree = MerkleTree::<Sha256>::from_leaves(&leaves);
                    let root = tree.root();

                    // Generate proof for first leaf
                    let proof = tree.proof(&[0]);

                    // Verify proof
                    let is_valid = root.is_some_and(|root_hash| {
                        proof.verify(root_hash, &[0], &[*leaves.first().unwrap_or(&[0; 32])], 1)
                    });
                    let duration = start.elapsed();

                    black_box((is_valid, duration))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark hash chain operations
fn bench_hash_chain_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_chain_operations");

    // Test with different chain lengths
    let chain_lengths = vec![10, 100, 1000, 10000];

    for chain_length in chain_lengths {
        group.bench_with_input(
            BenchmarkId::new("hash_chain_build", chain_length),
            &chain_length,
            |b, &length| {
                b.iter(|| {
                    let start = std::time::Instant::now();

                    let mut chain = Vec::new();
                    let mut previous_hash = [0_u8; 32];

                    for i in 0..length {
                        let mut hasher = Blake3Hasher::new();
                        hasher.update(&previous_hash);
                        hasher.update(format!("entry_{i}").as_bytes());
                        let current_hash = hasher.finalize();

                        chain.push(current_hash);
                        previous_hash = current_hash.into();
                    }

                    let duration = start.elapsed();
                    black_box((chain.len(), duration))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent hashing operations
fn bench_concurrent_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_hashing");

    group.bench_function("concurrent_blake3", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();

            let results: Vec<_> = (0..100)
                .map(|i| {
                    std::thread::spawn(move || {
                        let mut hasher = Blake3Hasher::new();
                        hasher.update(format!("concurrent_data_{i}").as_bytes());
                        hasher.finalize()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Blake3Hasher::new().finalize())
                })
                .collect();

            let duration = start.elapsed();
            black_box((results.len(), duration))
        });
    });

    group.bench_function("concurrent_sha256", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();

            let results: Vec<_> = (0..100)
                .map(|i| {
                    std::thread::spawn(move || {
                        let mut hasher = Sha256Hasher::new();
                        hasher.update(format!("concurrent_data_{i}").as_bytes());
                        hasher.finalize()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Sha256Hasher::new().finalize())
                })
                .collect();

            let duration = start.elapsed();
            black_box((results.len(), duration))
        });
    });

    group.finish();
}

/// Benchmark hash comparison operations
fn bench_hash_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_comparison");

    group.bench_function("hash_equality", |b| {
        let hash1 = Blake3Hasher::new().update(b"test1").finalize();
        let hash2 = Blake3Hasher::new().update(b"test2").finalize();

        b.iter(|| {
            let start = std::time::Instant::now();
            let is_equal = hash1 == hash2;
            let duration = start.elapsed();

            black_box((is_equal, duration))
        });
    });

    group.bench_function("hash_ordering", |b| {
        let hash1 = Blake3Hasher::new().update(b"test1").finalize();
        let hash2 = Blake3Hasher::new().update(b"test2").finalize();

        b.iter(|| {
            let start = std::time::Instant::now();
            let ordering = hash1.as_bytes().cmp(hash2.as_bytes());
            let duration = start.elapsed();

            black_box((ordering, duration))
        });
    });

    group.finish();
}

/// Benchmark cryptographic key generation
fn bench_key_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_generation");

    group.bench_function("generate_random_key", |b| {
        b.iter(|| {
            let start = std::time::Instant::now();
            let key = Blake3Hasher::new()
                .update(&rand::random::<[u8; 32]>())
                .finalize();
            let duration = start.elapsed();

            black_box((key, duration))
        });
    });

    group.bench_function("derive_key_from_seed", |b| {
        let seed = b"test_seed_for_key_derivation";

        b.iter(|| {
            let start = std::time::Instant::now();
            let key = Blake3Hasher::new()
                .update(seed)
                .update(b"key_derivation_context")
                .finalize();
            let duration = start.elapsed();

            black_box((key, duration))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_blake3_hashing,
    bench_sha256_hashing,
    bench_merkle_tree_operations,
    bench_merkle_proof_operations,
    bench_merkle_proof_verification,
    bench_hash_chain_operations,
    bench_concurrent_hashing,
    bench_hash_comparison,
    bench_key_generation
);
criterion_main!(benches);
