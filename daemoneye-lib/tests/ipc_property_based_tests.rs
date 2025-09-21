//! Property-based tests for IPC codec robustness
//!
//! This test suite uses proptest to generate random inputs and validate
//! codec behavior with malformed inputs, ensuring maximum resilience and security.

#![allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::uninlined_format_args,
    clippy::use_debug,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    clippy::match_same_arms,
    clippy::ignored_unit_patterns
)]

use proptest::prelude::*;
use daemoneye_lib::ipc::codec::{IpcCodec, IpcError};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, ProcessRecord, TaskType};
use std::io::Cursor;
use tokio::io::{DuplexStream, duplex};
use tokio::time::Duration;

/// Strategy for generating valid detection tasks
fn detection_task_strategy() -> impl Strategy<Value = DetectionTask> {
    (
        "[a-zA-Z0-9_-]{1,50}",
        prop::option::of("[a-zA-Z0-9 ]{0,100}"),
    )
        .prop_map(|(task_id, metadata)| DetectionTask {
            task_id,
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        })
}

/// Strategy for generating process records
fn process_record_strategy() -> impl Strategy<Value = ProcessRecord> {
    any::<u32>().prop_map(|pid| ProcessRecord {
        pid,
        ppid: Some(pid.saturating_sub(1)),
        name: format!("test_process_{}", pid),
        executable_path: Some(format!("/usr/bin/test_{}", pid)),
        command_line: vec![format!("test_{}", pid)],
        start_time: Some(chrono::Utc::now().timestamp()),
        cpu_usage: Some(25.5),
        memory_usage: Some(1024 * 1024),
        executable_hash: Some(format!("hash_{:08x}", pid)),
        hash_algorithm: Some("sha256".to_owned()),
        user_id: Some("1000".to_owned()),
        accessible: true,
        file_exists: true,
        collection_time: chrono::Utc::now().timestamp_millis(),
    })
}

/// Strategy for generating detection results
fn detection_result_strategy() -> impl Strategy<Value = DetectionResult> {
    (
        "[a-zA-Z0-9_-]{1,50}",
        any::<bool>(),
        prop::option::of("[a-zA-Z0-9 ]{0,200}"),
        prop::collection::vec(process_record_strategy(), 0..5),
    )
        .prop_map(
            |(task_id, success, error_message, processes)| DetectionResult {
                task_id,
                success,
                error_message,
                processes,
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            },
        )
}

/// Strategy for generating random byte arrays (malformed data)
fn malformed_data_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..1024)
}

/// Strategy for generating corrupted frame headers
fn corrupted_frame_strategy() -> impl Strategy<Value = Vec<u8>> {
    (
        any::<u32>(),
        any::<u32>(),
        prop::collection::vec(any::<u8>(), 0..100),
    )
        .prop_map(|(length, crc32, data)| {
            let mut frame = Vec::new();
            frame.extend_from_slice(&length.to_le_bytes());
            frame.extend_from_slice(&crc32.to_le_bytes());
            frame.extend_from_slice(&data);
            frame
        })
}

/// Create a test codec with default settings
fn create_test_codec() -> IpcCodec {
    IpcCodec::new(1024 * 1024)
}

/// Create a duplex stream pair for testing
fn create_test_streams() -> (DuplexStream, DuplexStream) {
    duplex(8192)
}

proptest! {
    /// Test that valid messages can be encoded and decoded correctly
    #[test]
    fn test_valid_message_roundtrip(task in detection_task_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let (mut client, mut server) = create_test_streams();
            let timeout_duration = Duration::from_secs(1);

            // Write message
            let write_result = codec.write_message(&mut client, &task, timeout_duration).await;
            prop_assert!(write_result.is_ok(), "Failed to write valid message: {:?}", write_result);

            // Read message back
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut server, timeout_duration).await;
            prop_assert!(read_result.is_ok(), "Failed to read valid message: {:?}", read_result);

            let decoded_task = read_result.unwrap();
            prop_assert_eq!(task.task_id, decoded_task.task_id);
            prop_assert_eq!(task.task_type, decoded_task.task_type);
            prop_assert_eq!(task.metadata, decoded_task.metadata);

            Ok(())
        })?;
    }

    /// Test that detection results can be encoded and decoded correctly
    #[test]
    fn test_detection_result_roundtrip(result in detection_result_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let (mut client, mut server) = create_test_streams();
            let timeout_duration = Duration::from_secs(1);

            // Write result
            let write_result = codec.write_message(&mut client, &result, timeout_duration).await;
            prop_assert!(write_result.is_ok(), "Failed to write valid result: {:?}", write_result);

            // Read result back
            let read_result: Result<DetectionResult, _> = codec.read_message(&mut server, timeout_duration).await;
            prop_assert!(read_result.is_ok(), "Failed to read valid result: {:?}", read_result);

            let decoded_result = read_result.unwrap();
            prop_assert_eq!(result.task_id, decoded_result.task_id);
            prop_assert_eq!(result.success, decoded_result.success);
            prop_assert_eq!(result.error_message, decoded_result.error_message);
            prop_assert_eq!(result.processes.len(), decoded_result.processes.len());

            Ok(())
        })?;
    }

    /// Test that malformed data is properly rejected
    #[test]
    fn test_malformed_data_rejection(malformed_data in malformed_data_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let mut cursor = Cursor::new(malformed_data);
            let timeout_duration = Duration::from_millis(100);

            // Try to read malformed data as a DetectionTask
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut cursor, timeout_duration).await;

            // Should fail with appropriate error
            prop_assert!(read_result.is_err(), "Malformed data should be rejected");

            match read_result.unwrap_err() {
                IpcError::InvalidLength { .. } |
                IpcError::TooLarge { .. } |
                IpcError::CrcMismatch { .. } |
                IpcError::Decode(_) |
                IpcError::PeerClosed |
                IpcError::Timeout => {
                    // These are all acceptable error types for malformed data
                }
                other => {
                    prop_assert!(false, "Unexpected error type for malformed data: {:?}", other);
                }
            }

            Ok(())
        })?;
    }

    /// Test that corrupted frames are detected and rejected
    #[test]
    fn test_corrupted_frame_detection(corrupted_frame in corrupted_frame_strategy()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let mut cursor = Cursor::new(corrupted_frame);
            let timeout_duration = Duration::from_millis(100);

            // Try to read corrupted frame
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut cursor, timeout_duration).await;

            // Should fail (corrupted frames should be detected)
            prop_assert!(read_result.is_err(), "Corrupted frame should be rejected");

            Ok(())
        })?;
    }

    /// Test CRC32 calculation consistency
    #[test]
    fn test_crc32_consistency(data in prop::collection::vec(any::<u8>(), 0..1000)) {
        let _codec1 = IpcCodec::new(1024 * 1024);
        let _codec2 = IpcCodec::new(1024 * 1024);

        // Calculate CRC32C with both codecs
        let crc1 = crc32c::crc32c(&data);
        let crc2 = crc32c::crc32c(&data);

        // Should be identical
        prop_assert_eq!(crc1, crc2, "CRC32 calculation should be consistent");

        // Should be deterministic (same input -> same output)
        let crc3 = crc32c::crc32c(&data);
        prop_assert_eq!(crc1, crc3, "CRC32 calculation should be deterministic");
    }

    /// Test frame size limits
    #[test]
    fn test_frame_size_limits(size_limit in 100_usize..10000) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let codec = IpcCodec::new(size_limit);
            let (mut client, _server) = create_test_streams();
            let timeout_duration = Duration::from_secs(1);

            // Create a task with metadata larger than the limit
            let large_metadata = "x".repeat(size_limit + 100);
            let large_task = DetectionTask {
                task_id: "test".to_owned(),
                task_type: TaskType::EnumerateProcesses as i32,
                process_filter: None,
                hash_check: None,
                metadata: Some(large_metadata),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None
            };

            // Should fail with TooLarge error
            let write_result = codec.write_message(&mut client, &large_task, timeout_duration).await;

            match write_result {
                Err(IpcError::TooLarge { size, max_size }) => {
                    prop_assert!(size > max_size, "Size should exceed limit");
                    prop_assert_eq!(max_size, size_limit, "Max size should match configured limit");
                }
                Err(IpcError::Encode(_)) => {
                    // Also acceptable - protobuf encoding might fail first
                }
                other => {
                    prop_assert!(false, "Expected TooLarge error for oversized message, got: {:?}", other);
                }
            }

            Ok(())
        })?;
    }

    /// Test timeout behavior with slow operations
    #[test]
    fn test_timeout_behavior(timeout_ms in 10_u64..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let (_client, mut server) = create_test_streams();
            let timeout_duration = Duration::from_millis(timeout_ms);

            // Try to read from empty stream with short timeout
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut server, timeout_duration).await;

            // Should timeout
            match read_result {
                Err(IpcError::Timeout) => {
                    // Expected
                }
                Err(IpcError::PeerClosed) => {
                    // Also acceptable - stream might be closed
                }
                other => {
                    prop_assert!(false, "Expected timeout or peer closed, got: {:?}", other);
                }
            }

            Ok(())
        })?;
    }

    /// Test that zero-length messages are rejected
    #[test]
    fn test_zero_length_rejection(_dummy in any::<u8>()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let timeout_duration = Duration::from_millis(100);

            // Create a frame with zero length
            let zero_frame = vec![
                0, 0, 0, 0,  // length = 0
                0, 0, 0, 0,  // crc32 = 0
                // no data
            ];

            let mut cursor = Cursor::new(zero_frame);
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut cursor, timeout_duration).await;

            // Should fail with InvalidLength error
            match read_result {
                Err(IpcError::InvalidLength { length }) => {
                    prop_assert_eq!(length, 0, "Should detect zero length");
                }
                other => {
                    prop_assert!(false, "Expected InvalidLength error for zero-length message, got: {:?}", other);
                }
            }

            Ok(())
        })?;
    }

    /// Test CRC32 mismatch detection
    #[test]
    fn test_crc_mismatch_detection(data in prop::collection::vec(any::<u8>(), 1..100)) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut codec = create_test_codec();
            let timeout_duration = Duration::from_millis(100);

        // Calculate correct CRC32C
        let correct_crc = crc32c::crc32c(&data);
            let wrong_crc = correct_crc.wrapping_add(1); // Corrupt the CRC

            // Create frame with wrong CRC
            let mut frame = Vec::new();
            frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
            frame.extend_from_slice(&wrong_crc.to_le_bytes());
            frame.extend_from_slice(&data);

            let mut cursor = Cursor::new(frame);
            let read_result: Result<DetectionTask, _> = codec.read_message(&mut cursor, timeout_duration).await;

            // Should fail with CrcMismatch error
            match read_result {
                Err(IpcError::CrcMismatch { expected, actual }) => {
                    prop_assert_eq!(expected, wrong_crc, "Expected CRC should match frame");
                    prop_assert_eq!(actual, correct_crc, "Actual CRC should be calculated correctly");
                }
                other => {
                    prop_assert!(false, "Expected CrcMismatch error, got: {:?}", other);
                }
            }

            Ok(())
        })?;
    }
}

/// Additional manual tests for edge cases
#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_partial_frame_handling() {
        let mut codec = create_test_codec();
        let (mut client, mut server) = create_test_streams();
        let timeout_duration = Duration::from_millis(100);

        // Write partial frame header (only 4 bytes instead of 8)
        let partial_header = vec![1, 0, 0, 0]; // length = 1, but missing CRC
        client
            .write_all(&partial_header)
            .await
            .expect("Failed to write partial header");

        // Try to read - should timeout or fail
        let read_result: Result<DetectionTask, _> =
            codec.read_message(&mut server, timeout_duration).await;
        assert!(read_result.is_err(), "Partial frame should be rejected");
    }

    #[tokio::test]
    async fn test_incomplete_message_handling() {
        let mut codec = create_test_codec();
        let (mut client, mut server) = create_test_streams();
        let timeout_duration = Duration::from_millis(100);

        // Write frame header claiming 100 bytes but only provide 50
        let mut frame = Vec::new();
        frame.extend_from_slice(&100_u32.to_le_bytes()); // claim 100 bytes
        frame.extend_from_slice(&0_u32.to_le_bytes()); // CRC = 0
        frame.extend_from_slice(&[0_u8; 50]); // only 50 bytes

        client
            .write_all(&frame)
            .await
            .expect("Failed to write incomplete frame");

        // Try to read - should timeout waiting for remaining bytes
        let read_result: Result<DetectionTask, _> =
            codec.read_message(&mut server, timeout_duration).await;
        assert!(
            read_result.is_err(),
            "Incomplete message should be rejected"
        );
    }

    #[tokio::test]
    async fn test_maximum_frame_size_boundary() {
        let max_size = 1024;
        let codec = IpcCodec::new(max_size);
        let (mut client, _server) = create_test_streams();
        let timeout_duration = Duration::from_secs(1);

        // Create task exactly at the boundary
        let boundary_metadata = "x".repeat(max_size - 100); // Leave room for other fields
        let boundary_task = DetectionTask {
            task_id: "boundary_test".to_owned(),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some(boundary_metadata),
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        // This might succeed or fail depending on exact serialized size
        let write_result = codec
            .write_message(&mut client, &boundary_task, timeout_duration)
            .await;

        // Either way, it should not panic or cause undefined behavior
        match write_result {
            Ok(_) => {
                // Message was within limits
            }
            Err(IpcError::TooLarge { .. }) => {
                // Message exceeded limits
            }
            Err(other) => {
                panic!("Unexpected error at boundary: {:?}", other);
            }
        }
    }

    #[tokio::test]
    async fn test_crc32c_consistency() {
        let data = b"test data for CRC32C consistency";

        let crc1 = crc32c::crc32c(data);
        let crc2 = crc32c::crc32c(data);

        // CRC32C should be deterministic
        assert_eq!(crc1, crc2, "CRC32C should produce consistent results");

        // Should be non-zero for non-empty data
        if !data.is_empty() {
            assert_ne!(crc1, 0, "CRC32C should not be zero for non-empty data");
        }
    }
}
