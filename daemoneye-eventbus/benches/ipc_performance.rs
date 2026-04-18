#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::use_debug,
    clippy::dbg_macro,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::modulo_arithmetic,
    clippy::must_use_candidate,
    clippy::missing_const_for_fn,
    clippy::let_underscore_must_use,
    clippy::uninlined_format_args,
    clippy::explicit_iter_loop,
    clippy::used_underscore_binding,
    clippy::str_to_string,
    clippy::shadow_unrelated,
    clippy::non_ascii_literal,
    clippy::pattern_type_mismatch,
    clippy::unseparated_literal_suffix,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::clone_on_ref_ptr
)]
//! IPC performance benchmarks for high-load and low-latency scenarios
//!
//! These benchmarks measure:
//! - Throughput (messages/second)
//! - Latency (p50, p99)
//! - Backpressure handling
//! - Cross-platform performance
//! - Zero-copy optimization improvements
//! - END-297 SLO gate: <1ms p99 local-IPC round-trip latency

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use daemoneye_eventbus::message::{CollectionEvent, ProcessEvent};
use daemoneye_eventbus::transport::{SocketConfig, TransportClient, TransportServer};
use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// END-297 acceptance criterion: local-IPC round-trip p99 latency must stay under 1ms.
///
/// This threshold is machine-checked by [`latency_p99_slo`]. Changing it requires
/// updating the ticket and the acceptance-evidence artifact.
/// Ticket: END-297 <https://evilbitlabs.atlassian.net/browse/END-297>
const LATENCY_P99_SLO: Duration = Duration::from_millis(1);

/// Number of warmup iterations discarded before p99 measurement begins.
const SLO_WARMUP_ITERS: usize = 1_000;

/// Number of round-trip samples collected for the p99 computation.
const SLO_SAMPLE_COUNT: usize = 10_000;

fn throughput_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("throughput-bench.sock");

        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let _server = TransportServer::new(socket_config.clone())
            .await
            .expect("Failed to create server");

        let mut client = TransportClient::connect(&socket_config)
            .await
            .expect("Failed to connect");

        let mut group = c.benchmark_group("throughput");
        group.measurement_time(Duration::from_secs(10));
        group.sample_size(100);

        for client_count in [1, 10, 100].iter() {
            group.bench_with_input(
                BenchmarkId::new("pub_sub_messages", client_count),
                client_count,
                |b, &_count| {
                    b.iter(|| {
                        let test_data = black_box(b"benchmark message".to_vec());
                        rt.block_on(async {
                            client.send(&test_data).await.unwrap();
                        });
                    });
                },
            );
        }

        group.finish();
    });
}

fn latency_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("latency-bench.sock");

        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let server = TransportServer::new(socket_config.clone())
            .await
            .expect("Failed to create server");

        // Start echo handler for standalone server use
        server
            .start_echo_handler()
            .await
            .expect("Failed to start echo handler");

        let mut client = TransportClient::connect(&socket_config)
            .await
            .expect("Failed to connect");

        let mut group = c.benchmark_group("latency");
        group.measurement_time(Duration::from_secs(5));
        group.sample_size(1000);

        // Benchmark small messages (zero-copy optimization should help)
        group.bench_function("ping_latency_small", |b| {
            b.iter(|| {
                let start = Instant::now();
                rt.block_on(async {
                    let ping = black_box(b"PING");
                    client.send(ping).await.unwrap();
                    let _response = client.receive().await.unwrap();
                });
                let latency = start.elapsed();
                black_box(latency);
            });
        });

        // Benchmark medium messages
        let medium_msg = vec![0u8; 1024];
        group.bench_function("ping_latency_medium", |b| {
            b.iter(|| {
                let start = Instant::now();
                rt.block_on(async {
                    client.send(&medium_msg).await.unwrap();
                    let _response = client.receive().await.unwrap();
                });
                let latency = start.elapsed();
                black_box(latency);
            });
        });

        // Benchmark large messages (tests buffer reuse)
        let large_msg = vec![0u8; 32 * 1024];
        group.bench_function("ping_latency_large", |b| {
            b.iter(|| {
                let start = Instant::now();
                rt.block_on(async {
                    client.send(&large_msg).await.unwrap();
                    let _response = client.receive().await.unwrap();
                });
                let latency = start.elapsed();
                black_box(latency);
            });
        });

        group.finish();
    });
}

fn backpressure_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("backpressure-bench.sock");

        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let _server = TransportServer::new(socket_config.clone())
            .await
            .expect("Failed to create server");

        let client = TransportClient::connect(&socket_config)
            .await
            .expect("Failed to connect");

        let mut group = c.benchmark_group("backpressure");
        group.measurement_time(Duration::from_secs(5));

        group.bench_function("semaphore_acquire_release", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let permit = client.acquire_permit().await.unwrap();
                    let _ = black_box(permit);
                });
            });
        });

        group.finish();
    });
}

fn cross_platform_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("cross-platform-bench.sock");

        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let mut group = c.benchmark_group("cross_platform");
        group.measurement_time(Duration::from_secs(2));

        group.bench_function("connection_setup", |b| {
            b.iter(|| {
                rt.block_on(async {
                    let server = TransportServer::new(socket_config.clone()).await.unwrap();
                    let _client = TransportClient::connect(&socket_config).await.unwrap();
                    black_box((server, _client));
                });
            });
        });

        group.finish();
    });
}

/// Build a representative `CollectionEvent::Process` payload for SLO latency runs.
///
/// Mirrors the shape used by `throughput.rs` so the SLO assertion reflects
/// production-like envelope size rather than an artificially trivial one.
fn build_slo_payload() -> Vec<u8> {
    let event = CollectionEvent::Process(ProcessEvent {
        pid: 4242,
        name: "slo_bench_process".to_owned(),
        command_line: Some("slo_bench_process --latency-gate".to_owned()),
        executable_path: Some("/usr/local/bin/slo_bench_process".to_owned()),
        ppid: Some(1000),
        start_time: Some(SystemTime::now()),
        metadata: HashMap::new(),
    });
    postcard::to_allocvec(&event).expect("serialize CollectionEvent::Process")
}

/// p99 latency SLO gate for local-IPC round-trip (END-297 R9).
///
/// Collects [`SLO_SAMPLE_COUNT`] round-trip samples after a
/// [`SLO_WARMUP_ITERS`]-iteration warmup, computes p99 from a sorted sample
/// vector (no external histogram dep), and panics if the observed p99 exceeds
/// [`LATENCY_P99_SLO`]. The panic fails the benchmark job so the SLO cannot
/// silently regress.
///
/// Uses the existing `TransportServer`/`TransportClient` echo path for a realistic
/// send/receive round trip over the local transport.
///
/// The histogram collection and SLO assertion run once in the function body.
/// A trivial `bench_function` pass-through is registered so criterion emits a
/// row for this target in standard output and the filter
/// `latency_p99_slo` targets it precisely via `cargo bench ... -- latency_p99_slo`.
fn latency_p99_slo(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("slo-latency-bench.sock");

        let socket_config = SocketConfig {
            unix_path: socket_path.to_string_lossy().to_string(),
            windows_pipe: socket_path.to_string_lossy().to_string(),
            connection_limit: 100,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: 10 * 1024 * 1024,
            rate_limit_config: None,
            correlation_config: None,
        };

        let server = TransportServer::new(socket_config.clone())
            .await
            .expect("Failed to create server");

        // start_echo_handler is an infinite accept-loop; run it on a background
        // task so the bench can drive the client on the main runtime.
        tokio::spawn(async move {
            let _ = server.start_echo_handler().await;
        });

        let mut client = TransportClient::connect(&socket_config)
            .await
            .expect("Failed to connect");

        let payload = build_slo_payload();

        // Warmup — drain cold-path effects (first-fault page faults, socket
        // buffer priming, allocator warm-up) before measuring.
        for _ in 0..SLO_WARMUP_ITERS {
            client.send(&payload).await.unwrap();
            let _ = client.receive().await.unwrap();
        }

        // Collect SLO_SAMPLE_COUNT round-trip latencies in nanoseconds.
        let mut samples: Vec<u64> = Vec::with_capacity(SLO_SAMPLE_COUNT);
        for _ in 0..SLO_SAMPLE_COUNT {
            let start = Instant::now();
            client.send(&payload).await.unwrap();
            let response = client.receive().await.unwrap();
            // Duration::as_nanos -> u128; local-IPC round-trip will never exceed u64 nanos.
            let elapsed = start.elapsed().as_nanos() as u64;
            samples.push(elapsed);
            black_box(response);
        }

        // Compute p99 from a sorted sample vector.
        //
        // hdrhistogram is not a workspace dependency (checked Cargo.lock before
        // authoring this bench); the AGENTS.md rule against adding external
        // deps without approval steers us toward a sorted-Vec implementation.
        // For N=10k samples this is O(N log N) ~ negligible vs. the measured
        // round-trips themselves.
        samples.sort_unstable();
        let p99_index = (samples.len() * 99) / 100;
        let p99_nanos = samples.get(p99_index).copied().unwrap();
        let p99 = Duration::from_nanos(p99_nanos);

        // Also surface p50 and max for diagnostic output when the assertion fires.
        let p50_index = samples.len() / 2;
        let p50 = Duration::from_nanos(samples.get(p50_index).copied().unwrap());
        let max = Duration::from_nanos(samples.last().copied().unwrap());

        println!(
            "latency_p99_slo: samples={} p50={:?} p99={:?} max={:?} threshold={:?}",
            samples.len(),
            p50,
            p99,
            max,
            LATENCY_P99_SLO,
        );

        // Gate the SLO assertion on Linux and macOS only; Windows and FreeBSD are
        // informational per AGENTS.md OS support matrix and the END-297 plan
        // (Key Technical Decisions § "Gate the benchmark SLO check only on
        // x86_64 Linux + macOS for now").
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert!(
                p99 <= LATENCY_P99_SLO,
                "END-297 SLO breach: local-IPC round-trip p99 = {p99:?} exceeds \
                 {LATENCY_P99_SLO:?} threshold (samples={sample_count}, p50={p50:?}, max={max:?}). \
                 See daemoneye-eventbus/benches/ipc_performance.rs::latency_p99_slo.",
                sample_count = samples.len(),
            );
        }
    });

    // Register a trivial bench entry so this target shows up in criterion's
    // standard output and is filterable via `cargo bench -- latency_p99_slo`.
    // The real measurement and SLO assertion happened above; this runs a
    // no-op computation to satisfy criterion's reporter.
    c.bench_function("latency_p99_slo", |b| {
        b.iter(|| black_box(LATENCY_P99_SLO));
    });
}

// END-297 SLO bench runs FIRST so its assertion fires before any pre-existing
// bench that may exhibit nested-runtime behavior. The SLO group is separate so
// `cargo bench --bench ipc_performance -- latency_p99_slo` targets it cleanly.
criterion_group!(slo_benches, latency_p99_slo);
criterion_group!(
    benches,
    throughput_benchmark,
    latency_benchmark,
    backpressure_benchmark,
    cross_platform_benchmark
);
criterion_main!(slo_benches, benches);
