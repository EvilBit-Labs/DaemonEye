//! Criterion benchmarks for the `daemoneye_lib::integrity` module.
//!
//! Measures streaming hash throughput across representative file sizes and
//! concurrency levels. Targets (see
//! `docs/plans/2026-04-07-001-feat-binary-hashing-integrity-plan.md`):
//!
//! - 1 KB warm: <50 µs (guards the `spawn_blocking` threshold)
//! - 50 MB warm, BLAKE3+SHA-256: <100 ms
//! - 250 MB warm: <500 ms
//!
//! Stub harness; real benches are added in P1.7.

use criterion::{Criterion, criterion_group, criterion_main};

fn placeholder(c: &mut Criterion) {
    c.bench_function("integrity_placeholder", |b| b.iter(|| 1_u32 + 2_u32));
}

criterion_group!(benches, placeholder);
criterion_main!(benches);
