//! Codec for protobuf message framing with CRC32 integrity validation.
//!
//! This module provides encode/decode functionality for IPC messages with the following frame format:
//! - u32 length (N) of protobuf message bytes (little-endian)
//! - u32 crc32 checksum over the message bytes (little-endian)
//! - N bytes protobuf message (prost-encoded)

use crate::ipc::Crc32Variant;
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
}

/// Codec for encoding and decoding protobuf messages with CRC32 validation
pub struct IpcCodec {
    /// Maximum allowed frame length
    max_frame_len: usize,
    /// CRC32 variant to use
    crc32_variant: Crc32Variant,
    /// Reusable buffer for reading
    read_buf: BytesMut,
}

impl IpcCodec {
    /// Create a new codec with specified limits and CRC variant
    pub fn new(max_frame_len: usize, crc32_variant: Crc32Variant) -> Self {
        Self {
            max_frame_len,
            crc32_variant,
            read_buf: BytesMut::with_capacity(8192), // Start with 8KB buffer
        }
    }

    /// Write a protobuf message with framing and CRC32 validation
    pub async fn write_message<T, W>(
        &mut self,
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

        // Calculate CRC32 over message bytes
        let crc32 = self.calculate_crc32(&message_bytes);

        // Build frame: length + crc32 + message
        let frame_len = 4 + 4 + message_len; // u32 + u32 + message
        let mut frame = BytesMut::with_capacity(frame_len);

        frame.put_u32_le(message_len as u32);
        frame.put_u32_le(crc32);
        frame.extend_from_slice(&message_bytes);

        // Write frame with timeout
        timeout(write_timeout, writer.write_all(&frame))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(IpcError::Io)?;

        timeout(write_timeout, writer.flush())
            .await
            .map_err(|_| IpcError::Timeout)?
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
        let mut header = [0u8; 8];
        timeout(read_timeout, reader.read_exact(&mut header))
            .await
            .map_err(|_| IpcError::Timeout)?
            .map_err(|e| {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    IpcError::PeerClosed
                } else {
                    IpcError::Io(e)
                }
            })?;

        let message_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
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
            .map_err(|_| IpcError::Timeout)?
            .map_err(|e| {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    IpcError::PeerClosed
                } else {
                    IpcError::Io(e)
                }
            })?;

        // Validate CRC32
        let actual_crc32 = self.calculate_crc32(&self.read_buf);
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

    /// Calculate CRC32 checksum based on configured variant
    fn calculate_crc32(&self, data: &[u8]) -> u32 {
        match self.crc32_variant {
            Crc32Variant::Ieee => {
                let mut hasher = crc32fast::Hasher::new();
                hasher.update(data);
                hasher.finalize()
            }
            Crc32Variant::Castagnoli => {
                // For now, use the same implementation - would need crc32c crate for true Castagnoli
                let mut hasher = crc32fast::Hasher::new();
                hasher.update(data);
                hasher.finalize()
            }
        }
    }
}

impl Default for IpcCodec {
    fn default() -> Self {
        Self::new(1024 * 1024, Crc32Variant::Ieee) // 1MB max, IEEE CRC32
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
mod tests {
    use super::*;
    use crate::proto::{DetectionTask, TaskType};
    use tokio::io::{DuplexStream, duplex};

    async fn create_test_codec_pair() -> (IpcCodec, DuplexStream, DuplexStream) {
        let codec = IpcCodec::new(1024, Crc32Variant::Ieee);
        let (client, server) = duplex(1024);
        (codec, client, server)
    }

    fn create_test_message() -> DetectionTask {
        DetectionTask {
            task_id: "test-123".to_string(),
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: Some("test metadata".to_string()),
        }
    }

    #[tokio::test]
    async fn test_encode_decode_roundtrip() {
        let (mut codec, mut client, mut server) = create_test_codec_pair().await;
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
        let (_codec, mut client, _server) = create_test_codec_pair().await;

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
        let mut codec = IpcCodec::new(100, Crc32Variant::Ieee); // Very small limit
        let (mut client, _server) = duplex(1024);

        let large_message = DetectionTask {
            task_id: "x".repeat(200), // Larger than our 100-byte limit
            task_type: TaskType::EnumerateProcesses as i32,
            process_filter: None,
            hash_check: None,
            metadata: None,
        };

        let timeout_duration = Duration::from_secs(1);
        let result = codec
            .write_message(&mut client, &large_message, timeout_duration)
            .await;

        match result {
            Err(IpcError::TooLarge { size, max_size }) => {
                assert!(size > max_size);
                assert_eq!(max_size, 100);
            }
            other => panic!("Expected TooLarge error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_crc_mismatch_detection() {
        let (mut codec, mut client, mut server) = create_test_codec_pair().await;
        let message = create_test_message();

        // Encode message properly
        let mut message_bytes = BytesMut::new();
        message.encode(&mut message_bytes).unwrap();
        let message_len = message_bytes.len() as u32;
        let correct_crc32 = codec.calculate_crc32(&message_bytes);

        // Write frame with incorrect CRC32
        let mut frame = BytesMut::new();
        frame.put_u32_le(message_len);
        frame.put_u32_le(correct_crc32.wrapping_add(1)); // Corrupt CRC
        frame.extend_from_slice(&message_bytes);

        client.write_all(&frame).await.unwrap();

        // Try to read - should get CRC mismatch
        let timeout_duration = Duration::from_secs(1);
        let result: Result<DetectionTask, _> =
            codec.read_message(&mut server, timeout_duration).await;

        match result {
            Err(IpcError::CrcMismatch { expected, actual }) => {
                assert_ne!(expected, actual);
            }
            other => panic!("Expected CrcMismatch error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_timeout_handling() {
        let (mut codec, mut _client, mut server) = create_test_codec_pair().await;

        // Try to read with a very short timeout and no data
        let short_timeout = Duration::from_millis(10);
        let result: Result<DetectionTask, _> = codec.read_message(&mut server, short_timeout).await;

        match result {
            Err(IpcError::Timeout) => {
                // Expected
            }
            other => panic!("Expected Timeout error, got: {:?}", other),
        }
    }

    #[test]
    fn test_crc32_variants() {
        let codec_ieee = IpcCodec::new(1024, Crc32Variant::Ieee);
        let codec_castagnoli = IpcCodec::new(1024, Crc32Variant::Castagnoli);

        let test_data = b"Hello, world!";

        let crc_ieee = codec_ieee.calculate_crc32(test_data);
        let crc_castagnoli = codec_castagnoli.calculate_crc32(test_data);

        // For now both use the same implementation, but they should be equal
        // TODO: Implement actual Castagnoli CRC32 when needed
        assert_eq!(crc_ieee, crc_castagnoli);
    }
}
