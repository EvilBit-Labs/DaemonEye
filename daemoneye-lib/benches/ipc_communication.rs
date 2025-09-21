#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::as_conversions,
    clippy::shadow_unrelated,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures::FutureExt;
use prost::Message;
use daemoneye_lib::ipc::{IpcConfig, ResilientIpcClient, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProtoProcessRecord};
use std::hint::black_box;
use tempfile::TempDir;

/// Benchmark IPC message serialization and deserialization
fn bench_ipc_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_serialization");

    group.bench_function("serialize_detection_task", |b| {
        b.iter(|| {
            let task = DetectionTask::new_enumerate_processes("benchmark_task", None);
            let serialized = prost::Message::encode_to_vec(&task);
            black_box(serialized.len())
        });
    });

    group.bench_function("deserialize_detection_task", |b| {
        let task = DetectionTask::new_enumerate_processes("benchmark_task", None);

        let serialized = prost::Message::encode_to_vec(&task);

        b.iter(|| {
            let deserialized = DetectionTask::decode(&serialized[..]).unwrap();

            black_box(deserialized.task_id)
        });
    });

    group.bench_function("serialize_detection_result", |b| {
        b.iter(|| {
            let result = DetectionResult {
                task_id: "benchmark_task".to_string(),
                success: true,
                error_message: None,
                processes: vec![ProtoProcessRecord {
                    pid: 1234,
                    ppid: Some(1000),
                    name: "test_process".to_string(),
                    executable_path: Some("/usr/bin/test".to_string()),
                    command_line: vec![
                        "test".to_string(),
                        "--arg".to_string(),
                        "value".to_string(),
                    ],
                    start_time: Some(chrono::Utc::now().timestamp()),
                    cpu_usage: Some(25.5),
                    memory_usage: Some(1024 * 1024),
                    executable_hash: Some("abc123def456".to_string()),
                    hash_algorithm: Some("sha256".to_string()),
                    user_id: Some("1000".to_string()),
                    accessible: true,
                    file_exists: true,
                    collection_time: chrono::Utc::now().timestamp_millis(),
                }],
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            };

            let serialized = prost::Message::encode_to_vec(&result);

            black_box(serialized.len())
        });
    });

    group.bench_function("deserialize_detection_result", |b| {
        let result = DetectionResult {
            task_id: "benchmark_task".to_string(),
            success: true,
            error_message: None,
            processes: vec![ProtoProcessRecord {
                pid: 1234,
                ppid: Some(1000),
                name: "test_process".to_string(),
                executable_path: Some("/usr/bin/test".to_string()),
                command_line: vec!["test".to_string(), "--arg".to_string(), "value".to_string()],
                start_time: Some(chrono::Utc::now().timestamp()),
                cpu_usage: Some(25.5),
                memory_usage: Some(1024 * 1024),
                executable_hash: Some("abc123def456".to_string()),
                hash_algorithm: Some("sha256".to_string()),
                user_id: Some("1000".to_string()),
                accessible: true,
                file_exists: true,
                collection_time: chrono::Utc::now().timestamp_millis(),
            }],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        };

        let serialized = prost::Message::encode_to_vec(&result);

        b.iter(|| {
            let deserialized = DetectionResult::decode(&serialized[..]).unwrap();

            black_box(deserialized.task_id)
        });
    });

    group.finish();
}

/// Benchmark CRC32 checksum calculation
fn bench_crc32_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("crc32_calculation");

    let test_data = b"This is test data for CRC32 calculation benchmark";

    group.bench_function("crc32c", |b| {
        b.iter(|| {
            let checksum = crc32c::crc32c(test_data);
            black_box(checksum)
        });
    });

    // Test with different data sizes
    let data_sizes = vec![100, 1000, 10000, 100_000];

    for data_size in data_sizes {
        group.bench_with_input(
            BenchmarkId::new("crc32_data_size", data_size),
            &data_size,
            |b, &data_size| {
                let test_data_bytes = vec![0_u8; data_size];

                b.iter(|| {
                    let checksum = crc32c::crc32c(&test_data_bytes);
                    black_box(checksum)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark IPC client creation and connection
fn bench_ipc_client_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_client_operations");

    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    group.bench_function("create_ipc_client", |b| {
        b.iter(|| {
            let config = IpcConfig {
                endpoint_path: socket_path.to_string_lossy().to_string(),
                max_frame_bytes: 1024 * 1024,
                max_connections: 4,
                accept_timeout_ms: 1000,
                read_timeout_ms: 5000,
                write_timeout_ms: 5000,
                transport: TransportType::Interprocess,
                panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
            };

            let client = ResilientIpcClient::new(config);
            black_box(client)
        });
    });

    group.bench_function("get_client_stats", |b| {
        let config = IpcConfig {
            endpoint_path: socket_path.to_string_lossy().to_string(),
            max_frame_bytes: 1024 * 1024,
            max_connections: 4,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            transport: TransportType::Interprocess,
            panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
        };

        let client = ResilientIpcClient::new(config);

        b.iter(|| {
            let stats = client.get_stats();
            black_box(stats)
        });
    });

    group.finish();
}

/// Benchmark message framing and unframing
fn bench_message_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_framing");

    let test_message = b"This is a test message for framing benchmark";

    group.bench_function("frame_message", |b| {
        b.iter(|| {
            // Simulate message framing with length prefix and CRC32C
            let length = test_message.len() as u32;
            let crc = crc32c::crc32c(test_message);

            let mut framed = Vec::new();
            framed.extend_from_slice(&length.to_le_bytes());
            framed.extend_from_slice(&crc.to_le_bytes());
            framed.extend_from_slice(test_message);

            black_box(framed.len())
        });
    });

    group.bench_function("unframe_message", |b| {
        // Create framed message
        let length = test_message.len() as u32;
        let crc = crc32c::crc32c(test_message);

        let mut framed = Vec::new();
        framed.extend_from_slice(&length.to_le_bytes());
        framed.extend_from_slice(&crc.to_le_bytes());
        framed.extend_from_slice(test_message);

        b.iter(|| {
            // Simulate message unframing
            let length_bytes = [framed[0], framed[1], framed[2], framed[3]];
            let crc_bytes = [framed[4], framed[5], framed[6], framed[7]];
            let length = u32::from_le_bytes(length_bytes);
            let crc = u32::from_le_bytes(crc_bytes);
            let message = &framed[8..8 + length as usize];

            // Verify CRC32C
            let calculated_crc = crc32c::crc32c(message);

            black_box(crc == calculated_crc)
        });
    });

    group.finish();
}

/// Benchmark concurrent IPC operations
fn bench_concurrent_ipc_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_ipc_operations");

    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    group.bench_function("concurrent_client_creation", |b| {
        b.iter(|| {
            let config = IpcConfig {
                endpoint_path: socket_path.to_string_lossy().to_string(),
                max_frame_bytes: 1024 * 1024,
                max_connections: 4,
                accept_timeout_ms: 1000,
                read_timeout_ms: 5000,
                write_timeout_ms: 5000,
                transport: TransportType::Interprocess,
                panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
            };

            // Create multiple clients concurrently using current runtime handle
            let handle = tokio::runtime::Handle::current();
            let clients: Vec<_> = (0..10)
                .map(|_| {
                    let config = config.clone();
                    handle.spawn(async move { ResilientIpcClient::new(config) })
                })
                .collect();

            // Wait for all clients to be created
            let _results = futures::future::join_all(clients).now_or_never();

            black_box(10) // Return the number of clients created
        });
    });

    group.finish();
}

/// Benchmark IPC message throughput
fn bench_ipc_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_throughput");

    // Test with different message sizes
    let message_sizes = vec![100, 1000, 10000, 100_000];

    for message_size in message_sizes {
        // Set throughput measurement based on message size
        // This tells Criterion to measure throughput in bytes per second
        group.throughput(criterion::Throughput::Bytes(message_size as u64));

        group.bench_with_input(
            BenchmarkId::new("message_throughput", message_size),
            &message_size,
            |b, &message_size| {
                b.iter(|| {
                    // Create test message
                    let test_message = vec![0_u8; message_size];

                    // Simulate full IPC message processing
                    let length = test_message.len() as u32;
                    let crc = crc32c::crc32c(&test_message);

                    // Frame message
                    let mut framed = Vec::new();
                    framed.extend_from_slice(&length.to_le_bytes());
                    framed.extend_from_slice(&crc.to_le_bytes());
                    framed.extend_from_slice(&test_message);

                    // Unframe message
                    let unframed_length_bytes = [framed[0], framed[1], framed[2], framed[3]];
                    let unframed_crc_bytes = [framed[4], framed[5], framed[6], framed[7]];
                    let unframed_length = u32::from_le_bytes(unframed_length_bytes);
                    let unframed_crc = u32::from_le_bytes(unframed_crc_bytes);
                    let unframed_message = &framed[8..8 + unframed_length as usize];

                    // Verify CRC32C
                    let calculated_crc = crc32c::crc32c(unframed_message);

                    black_box(calculated_crc == unframed_crc)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_ipc_serialization,
    bench_crc32_calculation,
    bench_ipc_client_operations,
    bench_message_framing,
    bench_concurrent_ipc_operations,
    bench_ipc_throughput
);
criterion_main!(benches);
