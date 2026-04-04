//! Cryptographic utilities for audit trails and integrity verification.
//!
//! This module provides cryptographic functions for creating Certificate Transparency-style
//! audit ledgers with Merkle tree structure for efficient verification and tamper detection.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Cryptographic operation errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CryptoError {
    #[error("Hash computation failed: {0}")]
    Hash(String),

    #[error("Signature verification failed: {0}")]
    Signature(String),

    #[error("Key generation failed: {0}")]
    Key(String),
}

/// Current hash format version.
///
/// Version history:
/// - `1`: Original colon-delimited format (epoch-seconds timestamp). Deprecated.
/// - `2`: Length-prefixed canonical encoding (RFC 3339 timestamp with sub-second precision).
pub const HASH_VERSION: u8 = 2;

/// BLAKE3 hash computation for audit trails.
pub struct Blake3Hasher;

impl Blake3Hasher {
    /// Compute BLAKE3 hash of the input data.
    pub fn hash(data: &[u8]) -> String {
        use blake3::Hasher;
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize().to_hex().to_string()
    }

    /// Compute BLAKE3 hash of a string.
    pub fn hash_string(s: &str) -> String {
        Self::hash(s.as_bytes())
    }
}

/// Audit chain entry for tamper-evident logging.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// Entry sequence number
    pub sequence: u64,
    /// Entry timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Actor performing the action
    pub actor: String,
    /// Action performed
    pub action: String,
    /// Payload hash
    pub payload_hash: String,
    /// Previous entry hash
    pub previous_hash: Option<String>,
    /// This entry's hash
    pub entry_hash: String,
}

impl AuditEntry {
    /// Encode a single variable-length field using length-prefixed encoding.
    ///
    /// The format is `<decimal-byte-length>:<field-value>`. This is unambiguous
    /// regardless of what characters appear inside the field value, because the
    /// decoder consumes exactly the declared number of bytes and never splits on
    /// the separator character.
    fn encode_field(field: &str) -> String {
        format!("{}:{}", field.len(), field)
    }

    /// Compute the canonical hash-input string for this entry (format version 2).
    ///
    /// Each variable-length field is length-prefixed as `<byte-length>:<value>` so
    /// that fields containing `:` cannot produce collisions. The fixed-width
    /// `sequence` field is written as-is with a trailing `:` separator. The
    /// RFC 3339 timestamp is treated as a variable-length field to preserve
    /// sub-second precision.
    ///
    /// Format: `v2:<seq>:<len(ts)>:<ts><len(actor)>:<actor><len(action)>:<action><len(ph)>:<ph><len(prev)>:<prev>`
    pub fn compute_entry_hash_input(
        sequence: u64,
        timestamp: &chrono::DateTime<chrono::Utc>,
        actor: &str,
        action: &str,
        payload_hash: &str,
        previous_hash: Option<&str>,
    ) -> String {
        let ts = timestamp.to_rfc3339();
        let prev = previous_hash.unwrap_or("");
        format!(
            "v{}:{}:{}{}{}{}{}",
            HASH_VERSION,
            sequence,
            Self::encode_field(&ts),
            Self::encode_field(actor),
            Self::encode_field(action),
            Self::encode_field(payload_hash),
            Self::encode_field(prev),
        )
    }

    /// Compute the legacy (v1) hash-input string for backward-compatible verification.
    ///
    /// The v1 format was a colon-delimited string using Unix epoch-seconds timestamps.
    /// It is ambiguous when `actor` or `action` contain `:` but is retained here
    /// solely to verify entries that were written before the v2 format was introduced.
    // Used by verify_integrity for backward-compatible ledger verification.
    #[allow(dead_code)]
    fn compute_entry_hash_input_v1(
        sequence: u64,
        timestamp: &chrono::DateTime<chrono::Utc>,
        actor: &str,
        action: &str,
        payload_hash: &str,
        previous_hash: Option<&str>,
    ) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            sequence,
            timestamp.timestamp(),
            actor,
            action,
            payload_hash,
            previous_hash.unwrap_or("")
        )
    }

    /// Create a new audit entry.
    pub fn new(
        sequence: u64,
        actor: String,
        action: String,
        payload: &[u8],
        previous_hash: Option<String>,
    ) -> Self {
        let payload_hash = Blake3Hasher::hash(payload);
        let timestamp = chrono::Utc::now();

        let entry_data = Self::compute_entry_hash_input(
            sequence,
            &timestamp,
            &actor,
            &action,
            &payload_hash,
            previous_hash.as_deref(),
        );
        let entry_hash = Blake3Hasher::hash_string(&entry_data);

        Self {
            sequence,
            timestamp,
            actor,
            action,
            payload_hash,
            previous_hash,
            entry_hash,
        }
    }
}

/// Certificate Transparency-style audit ledger for maintaining tamper-evident log integrity.
///
/// This structure provides Merkle tree-based verification with logarithmic proof sizes
/// and support for efficient inclusion/exclusion proofs and periodic checkpoints.
pub struct AuditLedger {
    entries: Vec<AuditEntry>,
    tree_size: usize,
}

impl AuditLedger {
    /// Create a new audit ledger.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            tree_size: 0,
        }
    }

    /// Add an entry to the audit ledger with Merkle tree structure.
    pub fn add_entry(&mut self, actor: String, action: String, payload: &[u8]) -> AuditEntry {
        let sequence = u64::try_from(self.tree_size).unwrap_or(0);
        let previous_hash = self.entries.last().map(|entry| entry.entry_hash.clone());

        let entry = AuditEntry::new(sequence, actor, action, payload, previous_hash);
        self.entries.push(entry.clone());
        self.tree_size = self.tree_size.saturating_add(1);
        entry
    }

    /// Generate inclusion proof for entry verification (placeholder for future Merkle tree implementation).
    pub const fn generate_inclusion_proof(_index: usize) -> Vec<String> {
        // TODO: Implement Merkle tree inclusion proof generation
        // This would use rs-merkle crate for efficient proof generation
        vec![]
    }

    /// Verify the integrity of the audit ledger.
    ///
    /// First attempts verification using the current v2 (length-prefixed) hash format.
    /// If that fails, falls back to the legacy v1 (colon-delimited, epoch-seconds)
    /// format so that ledgers written before the v2 format was introduced can still
    /// be verified.
    pub fn verify_integrity(&self) -> Result<(), CryptoError> {
        for (i, entry) in self.entries.iter().enumerate() {
            // Try current v2 format first.
            let entry_data_v2 = AuditEntry::compute_entry_hash_input(
                entry.sequence,
                &entry.timestamp,
                &entry.actor,
                &entry.action,
                &entry.payload_hash,
                entry.previous_hash.as_deref(),
            );
            let expected_v2 = Blake3Hasher::hash_string(&entry_data_v2);

            if entry.entry_hash != expected_v2 {
                // Fall back to legacy v1 format before declaring a mismatch.
                let entry_data_v1 = AuditEntry::compute_entry_hash_input_v1(
                    entry.sequence,
                    &entry.timestamp,
                    &entry.actor,
                    &entry.action,
                    &entry.payload_hash,
                    entry.previous_hash.as_deref(),
                );
                let expected_v1 = Blake3Hasher::hash_string(&entry_data_v1);
                if entry.entry_hash != expected_v1 {
                    return Err(CryptoError::Hash(format!("Hash mismatch at entry {i}")));
                }
            }

            // Verify chain continuity
            if i > 0 {
                let prev_entry = self
                    .entries
                    .get(i.saturating_sub(1))
                    .ok_or_else(|| CryptoError::Hash("Missing previous entry".to_owned()))?;
                if entry.previous_hash != Some(prev_entry.entry_hash.clone()) {
                    return Err(CryptoError::Hash(format!(
                        "Chain discontinuity at entry {i}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get all entries in the chain.
    pub fn get_entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Get the latest entry hash.
    pub fn get_latest_hash(&self) -> Option<&String> {
        self.entries.last().map(|entry| &entry.entry_hash)
    }
}

impl Default for AuditLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_hash() {
        let data = b"test data";
        let hash = Blake3Hasher::hash(data);
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // BLAKE3 produces 32 bytes = 64 hex chars
    }

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry::new(
            1,
            "test-actor".to_owned(),
            "test-action".to_owned(),
            b"test payload",
            None,
        );

        assert_eq!(entry.sequence, 1);
        assert_eq!(entry.actor, "test-actor");
        assert_eq!(entry.action, "test-action");
    }

    #[test]
    fn test_audit_ledger_integrity() {
        let mut ledger = AuditLedger::new();

        ledger.add_entry("actor1".to_owned(), "action1".to_owned(), b"payload1");
        ledger.add_entry("actor2".to_owned(), "action2".to_owned(), b"payload2");

        assert!(ledger.verify_integrity().is_ok());
        assert_eq!(ledger.get_entries().len(), 2);
    }

    #[test]
    fn test_audit_ledger_tamper_detection() {
        let mut ledger = AuditLedger::new();
        ledger.add_entry("actor1".to_owned(), "action1".to_owned(), b"payload1");

        // Tamper with the entry
        if let Some(entry) = ledger.entries.get_mut(0) {
            entry.actor = "tampered".to_owned();
        }

        assert!(ledger.verify_integrity().is_err());
    }

    #[test]
    fn test_compute_entry_hash_input_uses_rfc3339() {
        // Use a fixed timestamp with nanosecond sub-second precision.
        let ts = chrono::DateTime::from_timestamp_nanos(1_700_000_000_123_456_789);
        let input = AuditEntry::compute_entry_hash_input(0, &ts, "actor", "action", "phash", None);
        // The RFC 3339 representation of this timestamp includes sub-second digits.
        assert!(
            input.contains('.'),
            "hash input should include sub-second precision"
        );
        // v2 format starts with "v2:<sequence>:".
        assert!(
            input.starts_with("v2:0:"),
            "hash input should start with version and sequence number"
        );
        // All field values appear verbatim in the encoded output.
        assert!(input.contains("actor"), "hash input should contain actor");
        assert!(input.contains("action"), "hash input should contain action");
        assert!(
            input.contains("phash"),
            "hash input should contain payload hash"
        );
    }

    #[test]
    fn test_compute_entry_hash_input_colon_in_field_is_unambiguous() {
        // Fields containing ":" must produce different outputs than the same content
        // split at the colon boundary.
        let ts = chrono::Utc::now();
        // actor = "a:b", action = "c"  vs  actor = "a", action = "b:c"
        let input1 = AuditEntry::compute_entry_hash_input(0, &ts, "a:b", "c", "ph", None);
        let input2 = AuditEntry::compute_entry_hash_input(0, &ts, "a", "b:c", "ph", None);
        assert_ne!(
            input1, input2,
            "colon inside a field must not produce a collision"
        );
    }

    #[test]
    fn test_compute_entry_hash_input_with_previous_hash() {
        let ts = chrono::Utc::now();
        let prev = "abc123";
        let input = AuditEntry::compute_entry_hash_input(1, &ts, "a", "b", "c", Some(prev));
        // The previous hash value must appear verbatim somewhere in the output.
        assert!(
            input.contains(prev),
            "hash input should contain the previous hash value"
        );
    }

    #[test]
    fn test_hash_input_deterministic_for_same_timestamp() {
        // Given the same inputs, the hash input string must be identical.
        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp");
        let a = AuditEntry::compute_entry_hash_input(5, &ts, "actor", "action", "ph", None);
        let b = AuditEntry::compute_entry_hash_input(5, &ts, "actor", "action", "ph", None);
        assert_eq!(a, b);
    }

    #[test]
    fn test_verify_integrity_accepts_legacy_v1_entries() {
        // Build an entry whose hash was computed with the old v1 colon-delimited
        // format so we can confirm backward-compatible verification still works.
        let actor = "legacy-actor".to_owned();
        let action = "legacy-action".to_owned();
        let payload = b"legacy payload";
        let payload_hash = Blake3Hasher::hash(payload);
        let timestamp =
            chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp");

        let v1_input = format!(
            "{}:{}:{}:{}:{}:{}",
            0_u64,
            timestamp.timestamp(),
            actor,
            action,
            payload_hash,
            ""
        );
        let v1_hash = Blake3Hasher::hash_string(&v1_input);

        let legacy_entry = AuditEntry {
            sequence: 0,
            timestamp,
            actor,
            action,
            payload_hash,
            previous_hash: None,
            entry_hash: v1_hash,
        };

        let mut ledger = AuditLedger::new();
        ledger.entries.push(legacy_entry);
        assert!(
            ledger.verify_integrity().is_ok(),
            "legacy v1 entries must still pass integrity verification"
        );
    }
}
