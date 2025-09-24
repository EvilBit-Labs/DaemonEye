//! Performance benchmarks for IPC client comprehensive validation
//!
//! This benchmark suite implements performance validation requirements from task 3.5:
//! - Performance benchmarks ensuring no regression in message throughput or latency
//! - Comprehensive client performance validation
//! - Cross-platform performance characteristics

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use daemoneye_lib::ipc::client::{CollectorEndpoint, LoadBalancingStrategy, ResilientIpcClient};
use daemoneye_lib::ipc::interprocess_transport::{InterprocessClient, InterprocessServer};
use daemoneye_lib::ipc::{IpcConfig, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Sanitize a directory name for use in Windows named pipe paths
///
/// Windows named pipes have restrictions on valid characters. This function:
/// - Replaces any character not in [A-Za-z0-9._-] with '_'
/// - Trims leading/trailing dots and slashes
/// - Enforces a maximum length of 200 characters
/// - Falls back to "`bench_validation`" if the result is empty
#[cfg(windows)]
fn sanitize_pipe_name(dir_name: &str) -> String {
    let mut sanitized = String::with_capacity(dir_name.len());

    for ch in dir_name.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => {
                sanitized.push(ch);
            }
            _ => {
                sanitized.push('_');
            }
        }
    }

    // Trim leading/trailing dots and slashes
    let trimmed = sanitized.trim_matches(|c| c == '.' || c == '/' || c == '\\');

    // Enforce maximum length
    let truncated = if trimmed.len() > 200 {
        trimmed.chars().take(200).collect::<String>()
    } else {
        trimmed.to_owned()
    };

    // Fallback if empty
    if truncated.is_empty() {
        "bench_validation".to_owned()
    } else {
        truncated
    }
}

/// Create a benchmark configuration
fn create_benchmark_config(test_name: &str) -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_benchmark_endpoint(&temp_dir, test_name);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 10 * 1024 * 1024, // 10MB for large message tests
        accept_timeout_ms: 5000,
        read_timeout_ms: 30000,
        write_timeout_ms: 30000,
        max_connections: 32,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    (config, temp_dir)
}

/// Create platform-specific benchmark endpoint
fn create_benchmark_endpoint(temp_dir: &TempDir, test_name: &str) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join(format!("bench_validation_{test_name}.sock"))
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bench_validation");
        let sanitized_dir_name = sanitize_pipe_name(dir_name);
        format!(r"\\.\pipe\daemoneye\bench-validation-{test_name}-{sanitized_dir_name}")
    }
}

/// Create a benchmark detection task
fn create_benchmark_task(task_id: &str, metadata_size: usize) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: (metadata_size > 0).then(|| "x".repeat(metadata_size)),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    }
}

/// Create a benchmark process record
fn create_benchmark_process_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        pid,
        ppid: Some(pid.saturating_sub(1)),
        name: format!("benchmark_process_{pid}"),
        executable_path: Some(format!("/usr/bin/benchmark_{pid}")),
        command_line: vec![
            format!("benchmark_{pid}"),
            "--benchmark".to_owned(),
            format!("--pid={pid}"),
        ],
        start_time: Some(chrono::Utc::now().timestamp()),
        cpu_usage: Some(25.5),
        memory_usage: Some(1024_u64.saturating_mul(1024).saturating_mul(u64::from(pid))),
        executable_hash: Some(format!("benchmark_hash_{pid:08x}")),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        collection_time: chrono::Utc::now().timestamp_millis(),
    }
}

/// Create a benchmark detection result
fn create_benchmark_result(task_id: &str, num_processes: usize) -> DetectionResult {
    let max = num_processes.min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
    let mut processes = Vec::with_capacity(max);
    for i in 0..max {
        processes.push(create_benchmark_process_record(
            u32::try_from(i).unwrap_or(u32::MAX),
        ));
    }

    DetectionResult {
        task_id: task_id.to_owned(),
        success: true,
        error_message: None,
        processes,
        hash_result: None,
        network_events: vec![],
        filesystem_events: vec![],
        performance_events: vec![],
    }
}

/// Benchmark resilient client creation and initialization
fn bench_resilient_client_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("resilient_client_creation");

    group.bench_function("single_endpoint_client", |b| {
        b.iter(|| {
            let (config, _temp_dir) = create_benchmark_config("single_endpoint");
            let client = ResilientIpcClient::new(&config);
            black_box(client)
        });
    });

    group.bench_function("multi_endpoint_client", |b| {
        b.iter(|| {
            let (config1, _temp_dir1) = create_benchmark_config("multi_endpoint_1");
            let (config2, _temp_dir2) = create_benchmark_config("multi_endpoint_2");

            let endpoints = vec![
                CollectorEndpoint::new("endpoint1".to_owned(), config1.endpoint_path.clone(), 1),
                CollectorEndpoint::new("endpoint2".to_owned(), config2.endpoint_path, 2),
            ];

            let client = ResilientIpcClient::new_with_endpoints(
                &config1,
                endpoints,
                LoadBalancingStrategy::RoundRobin,
            );
            black_box(client)
        });
    });

    group.finish();
}

/// Benchmark client statistics and metrics collection
fn bench_client_statistics(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_statistics");

    let rt = Runtime::new().unwrap();

    group.bench_function("get_stats", |b| {
        let (config, _temp_dir) = create_benchmark_config("stats");
        let client = ResilientIpcClient::new(&config);

        b.iter(|| {
            rt.block_on(async {
                let stats = client.get_stats().await;
                black_box(stats)
            })
        });
    });

    group.bench_function("get_metrics", |b| {
        let (config, _temp_dir) = create_benchmark_config("metrics");
        let client = ResilientIpcClient::new(&config);

        b.iter(|| {
            let metrics = client.metrics();
            black_box(metrics.success_rate())
        });
    });

    group.bench_function("endpoint_management", |b| {
        let (config, temp_dir) = create_benchmark_config("endpoint_mgmt");
        let client = ResilientIpcClient::new(&config);
        let endpoint_path = create_benchmark_endpoint(&temp_dir, "benchmark");

        b.iter(|| {
            rt.block_on(async {
                let endpoint =
                    CollectorEndpoint::new("benchmark".to_owned(), endpoint_path.clone(), 1);
                client.add_endpoint(endpoint).await;
                let removed = client.remove_endpoint("benchmark").await;
                black_box(removed)
            })
        });
    });

    group.finish();
}

/// Benchmark client-server communication throughput
fn bench_client_server_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_server_throughput");
    group.sample_size(20); // Reduce sample size for throughput tests

    let rt = Runtime::new().unwrap();

    // Test different message sizes
    let message_sizes = vec![
        ("tiny", 0, 1),
        ("small", 10, 10),
        ("medium", 100, 100),
        ("large", 1000, 1000),
    ];

    for (size_name, metadata_size, num_processes) in message_sizes {
        group.throughput(Throughput::Elements(num_processes.try_into().unwrap_or(0)));

        group.bench_with_input(
            BenchmarkId::new("resilient_client_throughput", size_name),
            &(metadata_size, num_processes),
            |b, &(bench_metadata_size, bench_num_processes)| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("throughput_{size_name}"));

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());
                        let response_size = bench_num_processes;

                        server.set_handler(move |task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, response_size))
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        // Create resilient client
                        let client = ResilientIpcClient::new(&config);
                        let endpoint = CollectorEndpoint::new(
                            "throughput-test".to_owned(),
                            config.endpoint_path.clone(),
                            1,
                        );
                        client.add_endpoint(endpoint).await;

                        // Send request
                        let task = create_benchmark_task("throughput_test", bench_metadata_size);
                        let start_time = Instant::now();
                        let result = client.send_task(task).await.expect("Request failed");
                        let duration = start_time.elapsed();

                        server
                            .graceful_shutdown()
                            .await
                            .expect("Failed to stop server");

                        black_box((result.success, duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent client operations
fn bench_concurrent_client_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_client_operations");
    group.sample_size(10); // Reduce sample size for concurrent tests

    let rt = Runtime::new().unwrap();

    // Test different concurrency levels
    let concurrency_levels = vec![1, 2, 4, 8, 16];

    for concurrency in concurrency_levels {
        group.bench_with_input(
            BenchmarkId::new("concurrent_requests", concurrency),
            &concurrency,
            |b, &bench_concurrency| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config, _temp_dir) =
                            create_benchmark_config(&format!("concurrent_{bench_concurrency}"));
                        let request_counter = Arc::new(AtomicU32::new(0));
                        let handler_counter = Arc::clone(&request_counter);

                        // Start server
                        let mut server = InterprocessServer::new(config.clone());

                        server.set_handler(move |task: DetectionTask| {
                            let counter = Arc::clone(&handler_counter);
                            async move {
                                let _request_num = counter.fetch_add(1, Ordering::SeqCst);
                                Ok(create_benchmark_result(&task.task_id, 10))
                            }
                        });

                        server.start().await.expect("Failed to start server");
                        tokio::time::sleep(Duration::from_millis(200)).await;

                        // Create resilient client
                        let client = Arc::new(ResilientIpcClient::new(&config));
                        let endpoint = CollectorEndpoint::new(
                            "concurrent-test".to_owned(),
                            config.endpoint_path.clone(),
                            1,
                        );
                        client.add_endpoint(endpoint).await;

                        // Send concurrent requests
                        let mut handles = vec![];
                        let start_time = Instant::now();

                        for i in 0..bench_concurrency {
                            let client_clone = Arc::clone(&client);
                            let handle = tokio::spawn(async move {
                                let task = create_benchmark_task(&format!("concurrent_{i}"), 0);
                                client_clone.send_task(task).await
                            });
                            handles.push(handle);
                        }

                        // Wait for all requests to complete
                        let mut successful_requests: u32 = 0;
                        for handle in handles {
                            if let Ok(Ok(_)) = handle.await {
                                successful_requests = successful_requests.saturating_add(1);
                            }
                        }

                        let total_duration = start_time.elapsed();
                        server
                            .graceful_shutdown()
                            .await
                            .expect("Failed to stop server");

                        black_box((successful_requests, total_duration))
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark load balancing performance
fn bench_load_balancing_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_balancing_performance");
    group.sample_size(15); // Reduce sample size for load balancing tests

    let rt = Runtime::new().unwrap();

    // Test different load balancing strategies
    let strategies = vec![
        ("round_robin", LoadBalancingStrategy::RoundRobin),
        ("weighted", LoadBalancingStrategy::Weighted),
        ("priority", LoadBalancingStrategy::Priority),
    ];

    for (strategy_name, strategy) in strategies {
        group.bench_with_input(
            BenchmarkId::new("load_balancing_strategy", strategy_name),
            &strategy,
            |b, bench_strategy| {
                b.iter(|| {
                    rt.block_on(async {
                        let (config1, _temp_dir1) =
                            create_benchmark_config(&format!("lb1_{strategy_name}"));
                        let (config2, _temp_dir2) =
                            create_benchmark_config(&format!("lb2_{strategy_name}"));

                        // Start two servers
                        let mut server1 = InterprocessServer::new(config1.clone());
                        server1.set_handler(|task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, 5))
                        });

                        let mut server2 = InterprocessServer::new(config2.clone());
                        server2.set_handler(|task: DetectionTask| async move {
                            Ok(create_benchmark_result(&task.task_id, 5))
                        });

                        server1.start().await.expect("Failed to start server1");
                        server2.start().await.expect("Failed to start server2");
                        tokio::time::sleep(Duration::from_millis(200)).await;

                        // Create client with load balancing
                        let endpoints = vec![
                            CollectorEndpoint::new(
                                "server1".to_owned(),
                                config1.endpoint_path.clone(),
                                1,
                            ),
                            CollectorEndpoint::new(
                                "server2".to_owned(),
                                config2.endpoint_path.clone(),
                                2,
                            ),
                        ];

                        let client = ResilientIpcClient::new_with_endpoints(
                            &config1,
                            endpoints,
                            bench_strategy.clone(),
                        );

                        // Send multiple requests to test load balancing
                        let num_requests = 10;
                        let start_time = Instant::now();

                        for i in 0..num_requests {
                            let task = create_benchmark_task(&format!("lb_test_{i}"), 0);
                            let _result = client.send_task(task).await.expect("Request failed");
                        }

                        let duration = start_time.elapsed();

                        server1
                            .graceful_shutdown()
                            .await
                            .expect("Failed to stop server1");
                        server2
                            .graceful_shutdown()
                            .await
                            .expect("Failed to stop server2");

                        black_box(duration)
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark client error handling and recovery
fn bench_error_handling_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("error_handling_performance");
    group.sample_size(15); // Reduce sample size for error handling tests

    let rt = Runtime::new().unwrap();

    group.bench_function("circuit_breaker_performance", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (config, _temp_dir) = create_benchmark_config("circuit_breaker_perf");

                // Start server that fails initially
                let failure_counter = Arc::new(AtomicU32::new(0));
                let mut server = InterprocessServer::new(config.clone());
                let counter = Arc::clone(&failure_counter);
                server.set_handler(move |task: DetectionTask| {
                    let handler_counter = Arc::clone(&counter);
                    async move {
                        let failure_count = handler_counter.fetch_add(1, Ordering::SeqCst);

                        // Fail first 3 requests, then succeed
                        if failure_count < 3 {
                            return Err(daemoneye_lib::ipc::codec::IpcError::Encode(
                                "Simulated failure".to_owned(),
                            ));
                        }

                        Ok(create_benchmark_result(&task.task_id, 1))
                    }
                });

                server.start().await.expect("Failed to start server");
                tokio::time::sleep(Duration::from_millis(100)).await;

                // Test circuit breaker performance
                let client = ResilientIpcClient::new(&config);
                let endpoint = CollectorEndpoint::new(
                    "circuit-test".to_owned(),
                    config.endpoint_path.clone(),
                    1,
                );
                client.add_endpoint(endpoint).await;

                let start_time = Instant::now();

                // Send requests to trigger and recover from circuit breaker
                for i in 0..10 {
                    let task = create_benchmark_task(&format!("circuit_{i}"), 0);
                    let _result = client.send_task(task).await; // May succeed or fail
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }

                let duration = start_time.elapsed();
                server
                    .graceful_shutdown()
                    .await
                    .expect("Failed to stop server");

                black_box(duration)
            })
        });
    });

    group.bench_function("connection_retry_performance", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (config, _temp_dir) = create_benchmark_config("retry_perf");

                // Create client without server (to test retry performance)
                let client = ResilientIpcClient::new(&config);
                let endpoint = CollectorEndpoint::new(
                    "retry-test".to_owned(),
                    config.endpoint_path.clone(),
                    1,
                );
                client.add_endpoint(endpoint).await;

                let start_time = Instant::now();

                // Attempt connection that will fail quickly
                let task = create_benchmark_task("retry_test", 0);
                let _result = client.send_task(task).await; // Will fail

                let duration = start_time.elapsed();

                black_box(duration)
            })
        });
    });

    group.finish();
}

/// Benchmark client memory usage and resource management
fn bench_resource_management(c: &mut Criterion) {
    let mut group = c.benchmark_group("resource_management");

    let rt = Runtime::new().unwrap();

    group.bench_function("client_memory_usage", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (config, _temp_dir) = create_benchmark_config("memory_usage");

                // Create client and add many endpoints
                let client = ResilientIpcClient::new(&config);

                for i in 0..100 {
                    let endpoint = CollectorEndpoint::new(
                        format!("endpoint_{i}"),
                        format!("/tmp/endpoint_{i}.sock"),
                        i,
                    );
                    client.add_endpoint(endpoint).await;
                }

                // Get stats to exercise memory usage
                let stats = client.get_stats().await;
                let metrics = client.metrics();

                black_box((stats.endpoint_stats.len(), metrics.success_rate()))
            })
        });
    });

    group.bench_function("endpoint_management_performance", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (config, _temp_dir) = create_benchmark_config("endpoint_mgmt_perf");
                let client = ResilientIpcClient::new(&config);

                let start_time = Instant::now();

                // Add and remove endpoints rapidly
                for i in 0..50 {
                    let endpoint = CollectorEndpoint::new(
                        format!("temp_endpoint_{i}"),
                        format!("/tmp/temp_{i}.sock"),
                        1,
                    );
                    client.add_endpoint(endpoint).await;
                    client.remove_endpoint(&format!("temp_endpoint_{i}")).await;
                }

                let duration = start_time.elapsed();

                black_box(duration)
            })
        });
    });

    group.finish();
}

/// Benchmark cross-platform performance characteristics
fn bench_cross_platform_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_platform_performance");

    let rt = Runtime::new().unwrap();

    group.bench_function("platform_specific_operations", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (config, _temp_dir) = create_benchmark_config("cross_platform");

                // Start server
                let mut server = InterprocessServer::new(config.clone());
                server.set_handler(|task: DetectionTask| async move {
                    Ok(create_benchmark_result(&task.task_id, 10))
                });

                server.start().await.expect("Failed to start server");
                tokio::time::sleep(Duration::from_millis(100)).await;

                // Test platform-specific performance
                let client = ResilientIpcClient::new(&config);
                let endpoint = CollectorEndpoint::new(
                    "platform-test".to_owned(),
                    config.endpoint_path.clone(),
                    1,
                );
                client.add_endpoint(endpoint).await;

                let start_time = Instant::now();

                // Send requests to measure platform performance
                for i in 0..20 {
                    let task = create_benchmark_task(&format!("platform_{i}"), 0);
                    let _result = client.send_task(task).await.expect("Request failed");
                }

                let duration = start_time.elapsed();
                server
                    .graceful_shutdown()
                    .await
                    .expect("Failed to stop server");

                black_box(duration)
            })
        });
    });

    // Platform-specific socket creation benchmark
    group.bench_function("socket_creation_performance", |b| {
        b.iter(|| {
            let (config, _temp_dir) = create_benchmark_config("socket_creation");

            // Measure socket creation time
            let start_time = Instant::now();
            let _client = InterprocessClient::new(config);
            let creation_time = start_time.elapsed();

            black_box(creation_time)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_resilient_client_creation,
    bench_client_statistics,
    bench_client_server_throughput,
    bench_concurrent_client_operations,
    bench_load_balancing_performance,
    bench_error_handling_performance,
    bench_resource_management,
    bench_cross_platform_performance
);
criterion_main!(benches);
