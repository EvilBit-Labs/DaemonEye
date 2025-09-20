//! IPC Protocol layer with length-delimited protobuf framing, flow control, and rate limiting.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::ipc::{IpcError, IpcResult};

/// Protocol configuration with defaults
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    /// Default credit limit for flow control
    pub default_credit_limit: u32,
    /// Rate limit for queries per minute per rule
    pub rate_limit_per_minute: u32,
    /// Maximum message size in bytes
    pub max_message_size: usize,
    /// CRC32 checksum validation enabled
    pub enable_checksum: bool,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            default_credit_limit: 1000,
            rate_limit_per_minute: 100,
            max_message_size: 1024 * 1024, // 1MB
            enable_checksum: true,
        }
    }
}

/// Envelope for protocol messages with sequence number and checksum
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode)]
pub struct MessageEnvelope {
    /// Sequence number for ordering and flow control
    pub sequence_number: u32,
    /// Payload bytes (protobuf encoded)
    pub payload: Vec<u8>,
    /// CRC32 checksum for integrity verification
    pub checksum: u32,
}

impl MessageEnvelope {
    /// Create a new envelope with computed checksum
    pub fn new(sequence_number: u32, payload: Vec<u8>) -> Self {
        let checksum = if payload.is_empty() {
            0
        } else {
            crc32c::crc32c(&payload)
        };
        Self {
            sequence_number,
            payload,
            checksum,
        }
    }

    /// Verify the checksum of the payload
    pub fn verify_checksum(&self) -> bool {
        if self.payload.is_empty() {
            self.checksum == 0
        } else {
            crc32c::crc32c(&self.payload) == self.checksum
        }
    }
}

/// Flow control state for credit-based management
#[derive(Debug)]
#[allow(dead_code)]
pub struct FlowControlState {
    /// Current available credits
    credits: AtomicU32,
    /// Total credits granted
    total_credits: AtomicU32,
    /// Credits consumed
    consumed_credits: AtomicU32,
    /// Last credit grant time
    last_grant_time: AtomicU64,
}

#[allow(dead_code)]
impl FlowControlState {
    pub fn new(initial_credits: u32) -> Self {
        Self {
            credits: AtomicU32::new(initial_credits),
            total_credits: AtomicU32::new(initial_credits),
            consumed_credits: AtomicU32::new(0),
            last_grant_time: AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        }
    }

    /// Check if credits are available
    pub fn has_credits(&self) -> bool {
        self.credits.load(Ordering::SeqCst) > 0
    }

    /// Consume a credit
    pub fn consume_credit(&self) -> bool {
        let current = self.credits.load(Ordering::SeqCst);
        if current > 0 {
            self.credits.fetch_sub(1, Ordering::SeqCst);
            self.consumed_credits.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Grant additional credits
    pub fn grant_credits(&self, amount: u32) {
        self.credits.fetch_add(amount, Ordering::SeqCst);
        self.total_credits.fetch_add(amount, Ordering::SeqCst);
        self.last_grant_time.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::SeqCst,
        );
    }

    /// Revoke credits
    pub fn revoke_credits(&self, amount: u32) {
        let current = self.credits.load(Ordering::SeqCst);
        let to_revoke = amount.min(current);
        self.credits.fetch_sub(to_revoke, Ordering::SeqCst);
    }

    /// Get current credit status
    pub fn get_status(&self) -> (u32, u32, u32) {
        (
            self.credits.load(Ordering::SeqCst),
            self.total_credits.load(Ordering::SeqCst),
            self.consumed_credits.load(Ordering::SeqCst),
        )
    }
}

/// Rate limiter for per-rule query limiting
#[derive(Debug)]
#[allow(dead_code)]
pub struct RateLimiter {
    /// Rule-based rate limiting state
    rule_limits: Arc<RwLock<HashMap<String, RuleRateLimit>>>,
    /// Default rate limit per minute
    default_limit: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
struct RuleRateLimit {
    /// Number of requests in current window
    requests: AtomicU32,
    /// Window start time
    window_start: AtomicU64,
    /// Requests per minute limit
    limit: u32,
}

#[allow(dead_code)]
impl RateLimiter {
    pub fn new(default_limit: u32) -> Self {
        Self {
            rule_limits: Arc::new(RwLock::new(HashMap::new())),
            default_limit,
        }
    }

    /// Check if a rule is within rate limits
    pub async fn check_rate_limit(&self, rule_id: &str) -> bool {
        let mut limits = self.rule_limits.write().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let limit = limits
            .entry(rule_id.to_string())
            .or_insert_with(|| RuleRateLimit {
                requests: AtomicU32::new(0),
                window_start: AtomicU64::new(now),
                limit: self.default_limit,
            });

        // Reset window if more than a minute has passed
        let window_start = limit.window_start.load(Ordering::SeqCst);
        if now - window_start >= 60 {
            limit.requests.store(0, Ordering::SeqCst);
            limit.window_start.store(now, Ordering::SeqCst);
        }

        // Check if we're within the limit
        let current_requests = limit.requests.load(Ordering::SeqCst);
        if current_requests < limit.limit {
            limit.requests.fetch_add(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Set custom rate limit for a specific rule
    pub async fn set_rule_limit(&self, rule_id: &str, limit: u32) {
        let mut limits = self.rule_limits.write().await;
        if let Some(rule_limit) = limits.get_mut(rule_id) {
            rule_limit.limit = limit;
        } else {
            limits.insert(
                rule_id.to_string(),
                RuleRateLimit {
                    requests: AtomicU32::new(0),
                    window_start: AtomicU64::new(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    ),
                    limit,
                },
            );
        }
    }
}

/// Protocol encoder for outgoing messages
#[derive(Debug)]
pub struct ProtocolEncoder {
    sequence_number: AtomicU32,
    config: ProtocolConfig,
}

impl ProtocolEncoder {
    pub fn new(config: ProtocolConfig) -> Self {
        Self {
            sequence_number: AtomicU32::new(0),
            config,
        }
    }

    /// Encode a protobuf message with length-delimited framing
    pub fn encode_message<T: prost::Message>(&self, message: &T) -> IpcResult<Vec<u8>> {
        let payload = message.encode_to_vec();

        if payload.len() > self.config.max_message_size {
            return Err(IpcError::invalid_message(format!(
                "Message too large: {} bytes (max: {})",
                payload.len(),
                self.config.max_message_size
            )));
        }

        let sequence_number = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        let envelope = MessageEnvelope::new(sequence_number, payload);

        // Encode with varint length prefix
        let mut encoded = Vec::new();
        let envelope_bytes = bincode::encode_to_vec(&envelope, bincode::config::standard())
            .map_err(|e| IpcError::invalid_message(format!("Serialization failed: {}", e)))?;

        // Add varint length prefix
        let mut length_bytes = Vec::new();
        let mut length = envelope_bytes.len() as u64;
        while length >= 0x80 {
            length_bytes.push((length as u8) | 0x80);
            length >>= 7;
        }
        length_bytes.push(length as u8);

        encoded.extend_from_slice(&length_bytes);
        encoded.extend_from_slice(&envelope_bytes);

        Ok(encoded)
    }
}

/// Protocol decoder for incoming messages
#[derive(Debug)]
pub struct ProtocolDecoder {
    config: ProtocolConfig,
}

impl ProtocolDecoder {
    pub fn new(config: ProtocolConfig) -> Self {
        Self { config }
    }

    /// Decode a length-delimited message
    pub fn decode_message<T: prost::Message + Default>(&self, data: &[u8]) -> IpcResult<(T, u32)> {
        if data.is_empty() {
            return Err(IpcError::invalid_message("Empty message data"));
        }

        // Decode varint length prefix
        let (length, offset) = self.decode_varint(data)?;
        if offset + length as usize > data.len() {
            return Err(IpcError::invalid_message("Incomplete message data"));
        }

        // Deserialize envelope
        let envelope_bytes = &data[offset..offset + length as usize];
        let envelope: MessageEnvelope =
            bincode::decode_from_slice(envelope_bytes, bincode::config::standard())
                .map_err(|e| IpcError::invalid_message(format!("Deserialization failed: {}", e)))?
                .0;

        // Verify checksum if enabled
        if self.config.enable_checksum && !envelope.verify_checksum() {
            return Err(IpcError::invalid_message("Checksum verification failed"));
        }

        // Decode protobuf message
        let message = T::decode(&envelope.payload[..])
            .map_err(|e| IpcError::invalid_message(format!("Protobuf decode failed: {}", e)))?;

        Ok((message, envelope.sequence_number))
    }

    /// Decode varint from bytes
    fn decode_varint(&self, data: &[u8]) -> IpcResult<(u64, usize)> {
        let mut result = 0u64;
        let mut shift = 0;
        let mut offset = 0;

        for &byte in data {
            offset += 1;
            result |= ((byte & 0x7F) as u64) << shift;
            if (byte & 0x80) == 0 {
                return Ok((result, offset));
            }
            shift += 7;
            if shift >= 64 {
                return Err(IpcError::invalid_message("Varint too large"));
            }
        }

        Err(IpcError::invalid_message("Incomplete varint"))
    }
}

/// Protocol manager that coordinates encoding, decoding, flow control, and rate limiting
#[derive(Debug)]
#[allow(dead_code)]
pub struct ProtocolManager {
    encoder: ProtocolEncoder,
    decoder: ProtocolDecoder,
    flow_control: Arc<FlowControlState>,
    rate_limiter: Arc<RateLimiter>,
    config: ProtocolConfig,
}

#[allow(dead_code)]
impl ProtocolManager {
    pub fn new(config: ProtocolConfig) -> Self {
        Self {
            encoder: ProtocolEncoder::new(config.clone()),
            decoder: ProtocolDecoder::new(config.clone()),
            flow_control: Arc::new(FlowControlState::new(config.default_credit_limit)),
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limit_per_minute)),
            config,
        }
    }

    /// Encode a message with protocol framing
    pub fn encode_message<T: prost::Message>(&self, message: &T) -> IpcResult<Vec<u8>> {
        self.encoder.encode_message(message)
    }

    /// Decode a message with protocol validation
    pub fn decode_message<T: prost::Message + Default>(&self, data: &[u8]) -> IpcResult<(T, u32)> {
        self.decoder.decode_message(data)
    }

    /// Check if flow control allows sending
    pub fn can_send(&self) -> bool {
        self.flow_control.has_credits()
    }

    /// Consume a credit for sending
    pub fn consume_credit(&self) -> bool {
        self.flow_control.consume_credit()
    }

    /// Grant additional credits
    pub fn grant_credits(&self, amount: u32) {
        self.flow_control.grant_credits(amount);
    }

    /// Revoke credits
    pub fn revoke_credits(&self, amount: u32) {
        self.flow_control.revoke_credits(amount);
    }

    /// Check rate limit for a rule
    pub async fn check_rate_limit(&self, rule_id: &str) -> bool {
        self.rate_limiter.check_rate_limit(rule_id).await
    }

    /// Set custom rate limit for a rule
    pub async fn set_rule_rate_limit(&self, rule_id: &str, limit: u32) {
        self.rate_limiter.set_rule_limit(rule_id, limit).await;
    }

    /// Get flow control status
    pub fn get_flow_control_status(&self) -> (u32, u32, u32) {
        self.flow_control.get_status()
    }

    /// Get protocol configuration
    pub fn config(&self) -> &ProtocolConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_lib::proto::DetectionTask;

    #[test]
    fn test_message_envelope_creation() {
        let payload = b"test payload".to_vec();
        let envelope = MessageEnvelope::new(123, payload.clone());

        assert_eq!(envelope.sequence_number, 123);
        assert_eq!(envelope.payload, payload);
        assert!(envelope.verify_checksum());
    }

    #[test]
    fn test_message_envelope_checksum_verification() {
        let payload = b"test payload".to_vec();
        let mut envelope = MessageEnvelope::new(123, payload);

        // Corrupt the payload
        envelope.payload[0] = b'X';

        assert!(!envelope.verify_checksum());
    }

    #[test]
    fn test_flow_control_credits() {
        let flow_control = FlowControlState::new(10);

        assert!(flow_control.has_credits());
        assert!(flow_control.consume_credit());
        assert_eq!(flow_control.get_status().0, 9);

        // Consume all credits
        for _ in 0..9 {
            assert!(flow_control.consume_credit());
        }

        assert!(!flow_control.has_credits());
        assert!(!flow_control.consume_credit());
    }

    #[test]
    fn test_flow_control_grant_revoke() {
        let flow_control = FlowControlState::new(5);

        // Consume all credits
        for _ in 0..5 {
            flow_control.consume_credit();
        }

        assert!(!flow_control.has_credits());

        // Grant more credits
        flow_control.grant_credits(3);
        assert!(flow_control.has_credits());
        assert_eq!(flow_control.get_status().0, 3);

        // Revoke some credits
        flow_control.revoke_credits(2);
        assert_eq!(flow_control.get_status().0, 1);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let rate_limiter = RateLimiter::new(2);

        // Should allow first two requests
        assert!(rate_limiter.check_rate_limit("rule1").await);
        assert!(rate_limiter.check_rate_limit("rule1").await);

        // Should reject third request
        assert!(!rate_limiter.check_rate_limit("rule1").await);

        // Different rule should be allowed
        assert!(rate_limiter.check_rate_limit("rule2").await);
    }

    #[test]
    fn test_protocol_encoder() {
        let config = ProtocolConfig::default();
        let encoder = ProtocolEncoder::new(config);

        let task = DetectionTask {
            task_id: "test".to_string(),
            task_type: 1,
            process_filter: None,
            hash_check: None,
            metadata: Some("test".to_string()),
        };

        let encoded = encoder.encode_message(&task).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_protocol_decoder() {
        let config = ProtocolConfig::default();
        let encoder = ProtocolEncoder::new(config.clone());
        let decoder = ProtocolDecoder::new(config);

        let task = DetectionTask {
            task_id: "test".to_string(),
            task_type: 1,
            process_filter: None,
            hash_check: None,
            metadata: Some("test".to_string()),
        };

        let encoded = encoder.encode_message(&task).unwrap();
        let (decoded_task, seq_num) = decoder.decode_message::<DetectionTask>(&encoded).unwrap();

        assert_eq!(decoded_task.task_id, task.task_id);
        assert_eq!(seq_num, 0); // First message has sequence 0
    }

    #[test]
    fn test_protocol_manager() {
        let config = ProtocolConfig::default();
        let manager = ProtocolManager::new(config);

        let task = DetectionTask {
            task_id: "test".to_string(),
            task_type: 1,
            process_filter: None,
            hash_check: None,
            metadata: Some("test".to_string()),
        };

        // Test encoding
        let encoded = manager.encode_message(&task).unwrap();
        assert!(!encoded.is_empty());

        // Test decoding
        let (decoded_task, _seq_num) = manager.decode_message::<DetectionTask>(&encoded).unwrap();
        assert_eq!(decoded_task.task_id, task.task_id);

        // Test flow control
        assert!(manager.can_send());
        assert!(manager.consume_credit());

        // Test credit management
        manager.grant_credits(100);
        let (available, total, consumed) = manager.get_flow_control_status();
        assert!(available > 0);
        assert!(total > 0);
        assert!(consumed > 0);
    }

    #[tokio::test]
    async fn test_protocol_manager_rate_limiting() {
        let config = ProtocolConfig {
            rate_limit_per_minute: 2,
            ..Default::default()
        };
        let manager = ProtocolManager::new(config);

        // Should allow first two requests
        assert!(manager.check_rate_limit("rule1").await);
        assert!(manager.check_rate_limit("rule1").await);

        // Should reject third request
        assert!(!manager.check_rate_limit("rule1").await);

        // Set custom limit
        manager.set_rule_rate_limit("rule2", 1).await;
        assert!(manager.check_rate_limit("rule2").await);
        assert!(!manager.check_rate_limit("rule2").await);
    }
}
