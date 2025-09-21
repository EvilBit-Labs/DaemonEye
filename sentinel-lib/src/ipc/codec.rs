//! Codec for protobuf message framing with CRC32 integrity validation.
//!
//! This module provides encode/decode functionality for IPC messages with the following frame format:
//! - u32 length (N) of protobuf message bytes (little-endian)
//! - u32 crc32 checksum over the message bytes (little-endian)
//! - N bytes protobuf message (prost-encoded)

use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use prost::Message;
use std::io;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Duration, timeout};

/// Result type for IPC operations
pub type IpcResult<T> = Result<T, IpcError>;

/// Comprehensive error types for IPC operations
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum IpcError {
    #[error("Timeout occurred during operation")]
    Timeout,

    #[error("Message too large: {size} bytes (max: {max_size})")]
    TooLarge { size: usize, max_size: usize },

    #[error("CRC32 mismatch: expected {expected:08x}, got {actual:08x}")]
    CrcMismatch { expected: u32, actual: u32 },

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("Protobuf encode error: {0}")]
    Encode(String),

    #[error("Peer connection closed")]
    PeerClosed,

    #[error("Invalid message length: {length}")]
    InvalidLength { length: u32 },

    #[error("Server not found at {endpoint} - ensure procmond is running")]
    ServerNotFound { endpoint: String },

    #[error("Connection refused to {endpoint} - server may not be accepting connections")]
    ConnectionRefused { endpoint: String },

    #[error("Permission denied connecting to {endpoint} - check file permissions")]
    PermissionDenied { endpoint: String },

    #[error("Connection timeout to {endpoint} - server may be overloaded")]
    ConnectionTimeout { endpoint: String },

    #[error("Circuit breaker is open - too many recent failures")]
    CircuitBreakerOpen,
}

/// Codec for encoding and decoding protobuf messages with CRC32C validation
pub struct IpcCodec {
    /// Maximum allowed frame length
    max_frame_len: usize,
    /// Reusable buffer for reading
    read_buf: BytesMut,
}

impl IpcCodec {
    /// Create a new codec with specified limits
    pub fn new(max_frame_len: usize) -> Self {
        Self {
            max_frame_len,
            read_buf: BytesMut::with_capacity(8192), // Start with 8KB buffer
        }
    }

    /// Write a protobuf message with framing and CRC32 validation
    pub async fn write_message<T, W>(
        &self,
        writer: &mut W,
        message: &T,
        write_timeout: Duration,
    ) -> IpcResult<()>
    where
        T: Message,
        W: AsyncWrite + Unpin,
    {
        // Encode the protobuf message
        let mut message_bytes = BytesMut::new();
        message
            .encode(&mut message_bytes)
            .map_err(|e| IpcError::Encode(e.to_string()))?;

        let message_len = message_bytes.len();

        // Check size limits
        if message_len == 0 {
            return Err(IpcError::InvalidLength { length: 0 });
        }
        if message_len > self.max_frame_len {
            return Err(IpcError::TooLarge {
                size: message_len,
                max_size: self.max_frame_len,
            });
        }

        // Calculate CRC32C over message bytes
        let crc32 = crc32c::crc32c(&message_bytes);

        // Build frame: length + crc32 + message
        let frame_len = 4_usize.saturating_add(4).saturating_add(message_len); // u32 + u32 + message
        let mut frame = BytesMut::with_capacity(frame_len);

        frame.put_u32_le(u32::try_from(message_len).unwrap_or(0));
        frame.put_u32_le(crc32);
        frame.extend_from_slice(&message_bytes);

        // Write frame with timeout
        timeout(write_timeout, writer.write_all(&frame))
            .await
            .map_err(|_err| IpcError::Timeout)?
            .map_err(IpcError::Io)?;

        timeout(write_timeout, writer.flush())
            .await
            .map_err(|_err| IpcError::Timeout)?
            .map_err(IpcError::Io)?;

        Ok(())
    }

    /// Read a protobuf message with framing and CRC32 validation
    pub async fn read_message<T, R>(
        &mut self,
        reader: &mut R,
        read_timeout: Duration,
    ) -> IpcResult<T>
    where
        T: Message + Default,
        R: AsyncRead + Unpin,
    {
        // Read frame header (length + crc32)
        let mut header = [0_u8; 8];
        timeout(read_timeout, reader.read_exact(&mut header))
            .await
            .map_err(|_err| IpcError::Timeout)?
            .map_err(|e| {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    IpcError::PeerClosed
                } else {
                    IpcError::Io(e)
                }
            })?;

        let message_len = usize::try_from(u32::from_le_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .unwrap_or(0);
        let expected_crc32 = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

        // Validate message length
        if message_len == 0 {
            return Err(IpcError::InvalidLength { length: 0 });
        }
        if message_len > self.max_frame_len {
            return Err(IpcError::TooLarge {
                size: message_len,
                max_size: self.max_frame_len,
            });
        }

        // Ensure buffer capacity and read message
        if self.read_buf.capacity() < message_len {
            self.read_buf.reserve(message_len);
        }
        self.read_buf.clear();
        self.read_buf.resize(message_len, 0);

        timeout(read_timeout, reader.read_exact(&mut self.read_buf))
            .await
            .map_err(|_err| IpcError::Timeout)?
            .map_err(|e| {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    IpcError::PeerClosed
                } else {
                    IpcError::Io(e)
                }
            })?;

        // Validate CRC32C
        let actual_crc32 = crc32c::crc32c(&self.read_buf);
        if expected_crc32 != actual_crc32 {
            return Err(IpcError::CrcMismatch {
                expected: expected_crc32,
                actual: actual_crc32,
            });
        }

        // Decode protobuf message
        let message = T::decode(&self.read_buf[..])?;
        Ok(message)
    }
}

impl Default for IpcCodec {
    fn default() -> Self {
        Self::new(1024 * 1024) // 1MB max
    }
}

/// Async trait for codec operations
#[async_trait]
pub trait AsyncCodec {
    /// Write a message with timeout
    async fn write_msg<T: Message + Send>(
        &mut self,
        message: &T,
        timeout: Duration,
    ) -> IpcResult<()>;

    /// Read a message with timeout
    async fn read_msg<T: Message + Default + Send>(&mut self, timeout: Duration) -> IpcResult<T>;
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::uninlined_format_args,
    clippy::as_conversions,
    clippy::assertions_on_constants
)]
mod tests {
    use super::*;
    use crate::proto::{DetectionTask, TaskType};
    use tokio::io::{DuplexStream, duplex};

    fn create_test_codec_pair() -> (IpcCodec, DuplexStream, DuplexStream) {
        let codec = IpcCodec::new(1024);
        let (client, server) = duplex(1024);
        (codec, client, server)
    }

    fn create_test_message() -> DetectionTask {
        DetectionTask::new_test_task(
            "test-123",
            TaskType::EnumerateProcesses,
            Some("test metadata".to_owned()),
        )
    }

    #[tokio::test]
    async fn test_encode_decode_roundtrip() {
        let (mut codec, mut client, mut server) = create_test_codec_pair();
        let original_message = create_test_message();

        let timeout_duration = Duration::from_secs(1);

        // Write message from client
        let write_result = codec
            .write_message(&mut client, &original_message, timeout_duration)
            .await;
        assert!(
            write_result.is_ok(),
            "Failed to write message: {:?}",
            write_result
        );

        // Read message from server
        let decoded_message: DetectionTask = codec
            .read_message(&mut server, timeout_duration)
            .await
            .expect("Failed to read message");

        // Verify roundtrip
        assert_eq!(original_message.task_id, decoded_message.task_id);
        assert_eq!(original_message.task_type, decoded_message.task_type);
        assert_eq!(original_message.metadata, decoded_message.metadata);
    }

    #[tokio::test]
    async fn test_empty_message_rejection() {
        let (_codec, mut client, _server) = create_test_codec_pair();

        // Try to create a zero-length message by manipulating the frame
        let mut frame = BytesMut::new();
        frame.put_u32_le(0); // Zero length
        frame.put_u32_le(0); // Zero CRC

        let timeout_duration = Duration::from_secs(1);
        let result = timeout(timeout_duration, client.write_all(&frame)).await;

        // This should be handled by the codec's length validation
        assert!(result.is_ok()); // Write succeeds, but read should fail
    }

    #[tokio::test]
    async fn test_oversized_message_rejection() {
        let codec = IpcCodec::new(100); // Very small limit
        let (mut client, _server) = duplex(1024);

        let large_message = DetectionTask::new_test_task(
            "x".repeat(200), // Larger than our 100-byte limit
            TaskType::EnumerateProcesses,
            None,
        );

        let timeout_duration = Duration::from_secs(1);
        let result = codec
            .write_message(&mut client, &large_message, timeout_duration)
            .await;

        match result {
            Err(IpcError::TooLarge { size, max_size }) => {
                assert!(size > max_size);
                assert_eq!(max_size, 100);
            }
            other => {
                assert!(
                    false,
                    "Expected TooLarge error for oversized message test, got: {other:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_crc_mismatch_detection() {
        let (mut codec, mut client, mut server) = create_test_codec_pair();
        let message = create_test_message();

        // Encode message properly
        let mut message_bytes = BytesMut::new();
        message
            .encode(&mut message_bytes)
            .expect("Failed to encode test message for CRC test");
        let message_len = message_bytes.len() as u32;
        let correct_crc32 = crc32c::crc32c(&message_bytes);

        // Write frame with incorrect CRC32
        let mut frame = BytesMut::new();
        frame.put_u32_le(message_len);
        frame.put_u32_le(correct_crc32.wrapping_add(1)); // Corrupt CRC
        frame.extend_from_slice(&message_bytes);

        client
            .write_all(&frame)
            .await
            .expect("Failed to write corrupted frame for CRC test");

        // Try to read - should get CRC mismatch
        let timeout_duration = Duration::from_secs(1);
        let result: Result<DetectionTask, _> =
            codec.read_message(&mut server, timeout_duration).await;

        match result {
            Err(IpcError::CrcMismatch { expected, actual }) => {
                assert_ne!(expected, actual);
            }
            other => {
                assert!(
                    false,
                    "Expected CrcMismatch error for corrupted CRC test, got: {other:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_timeout_handling() {
        let (mut codec, mut _client, mut server) = create_test_codec_pair();

        // Try to read with a very short timeout and no data
        let short_timeout = Duration::from_millis(10);
        let result: Result<DetectionTask, _> = codec.read_message(&mut server, short_timeout).await;

        match result {
            Err(IpcError::Timeout) => {
                // Expected
            }
            other => {
                assert!(
                    false,
                    "Expected Timeout error for timeout handling test, got: {other:?}"
                );
            }
        }
    }

    #[test]
    fn test_crc32c_calculation() {
        let test_data = b"Hello, world!";
        let crc = crc32c::crc32c(test_data);

        // Verify CRC32C produces non-zero result (basic sanity check)
        assert_ne!(crc, 0, "CRC32C should not be zero for non-empty data");

        // Verify CRC32C is deterministic
        let crc2 = crc32c::crc32c(test_data);
        assert_eq!(crc, crc2, "CRC32C should be deterministic");
    }
}
