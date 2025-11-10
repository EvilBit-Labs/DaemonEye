//! IPC performance benchmarks for high-load and low-latency scenarios
//!
//! These benchmarks measure:
//! - Throughput (messages/second)
//! - Latency (p50, p99)
//! - Backpressure handling
//! - Cross-platform performance
//! - Zero-copy optimization improvements

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use daemoneye_eventbus::transport::{SocketConfig, TransportClient, TransportServer};
use std::hint::black_box;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;

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

criterion_group!(
    benches,
    throughput_benchmark,
    latency_benchmark,
    backpressure_benchmark,
    cross_platform_benchmark
);
criterion_main!(benches);
