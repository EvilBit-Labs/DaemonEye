//! Secondary indexes for `processes.events` (T3 · U4).
//!
//! Four multimap secondary indexes live inside each time bucket, mapping an
//! index term to the `(ts_ms, seq)` primary-key pointers (not row copies):
//!
//! - `idx:pid@<id>` / `idx:ppid@<id>` — keyed by the raw `u32` pid/ppid.
//! - `idx:name@<id>` — keyed by a 128-bit BLAKE3 hash of the lowercased name.
//! - `idx:exe_hash@<id>` — keyed by the 128-bit prefix of the SHA-256 executable
//!   hash.
//!
//! Hash keys can collide; callers therefore verify candidate postings against
//! the primary row (e.g. exact name match) before returning them. Because redb
//! stores multimap values as a sorted set, postings come back in ascending
//! `(ts_ms, seq)` order for free.

use super::codec::TsSeqKey;
use redb::MultimapTableDefinition;

/// Multimap index keyed by a `u32` term (pid / ppid) → `(ts_ms, seq)`.
pub(super) type U32Index<'a> = MultimapTableDefinition<'a, u32, TsSeqKey>;

/// Multimap index keyed by a 128-bit hash term (name / `exe_hash`) → `(ts_ms, seq)`.
pub(super) type HashIndex<'a> = MultimapTableDefinition<'a, u128, TsSeqKey>;

/// 128-bit BLAKE3 hash of a lowercased process name (the `idx:name` key).
pub(super) fn name_hash128(name: &str) -> u128 {
    let lowered = name.to_lowercase();
    let digest = blake3::hash(lowered.as_bytes());
    let prefix: [u8; 16] = digest.as_bytes()[..16].try_into().unwrap_or([0_u8; 16]);
    u128::from_be_bytes(prefix)
}

/// 128-bit prefix of a hex SHA-256 executable hash (the `idx:exe_hash` key),
/// or `None` when the value is too short or not valid hex.
pub(super) fn exe_hash_prefix(hex: &str) -> Option<u128> {
    hex.get(..32)
        .and_then(|prefix| u128::from_str_radix(prefix, 16).ok())
}

/// `idx:pid` table name for a bucket.
pub(super) fn pid_index_name(bucket_id: u64) -> String {
    format!("idx:pid@{bucket_id}")
}

/// `idx:ppid` table name for a bucket.
pub(super) fn ppid_index_name(bucket_id: u64) -> String {
    format!("idx:ppid@{bucket_id}")
}

/// `idx:name` table name for a bucket.
pub(super) fn name_index_name(bucket_id: u64) -> String {
    format!("idx:name@{bucket_id}")
}

/// `idx:exe_hash` table name for a bucket.
pub(super) fn exe_index_name(bucket_id: u64) -> String {
    format!("idx:exe_hash@{bucket_id}")
}

/// Build a `u32`-keyed multimap index definition from a table name.
pub(super) const fn u32_index_def(name: &str) -> U32Index<'_> {
    MultimapTableDefinition::new(name)
}

/// Build a 128-bit-hash-keyed multimap index definition from a table name.
pub(super) const fn hash_index_def(name: &str) -> HashIndex<'_> {
    MultimapTableDefinition::new(name)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_is_case_insensitive_and_stable() {
        assert_eq!(name_hash128("Bash"), name_hash128("bash"));
        assert_eq!(name_hash128("bash"), name_hash128("bash"));
        assert_ne!(name_hash128("bash"), name_hash128("zsh"));
    }

    #[test]
    fn exe_hash_prefix_parses_leading_128_bits() {
        let sha = "abcdef0123456789abcdef0123456789ffffffffffffffffffffffffffffffff";
        let prefix = exe_hash_prefix(sha).expect("parse prefix");
        assert_eq!(prefix, 0xabcd_ef01_2345_6789_abcd_ef01_2345_6789);
    }

    #[test]
    fn exe_hash_prefix_rejects_short_or_nonhex() {
        assert_eq!(exe_hash_prefix("abc"), None);
        assert_eq!(exe_hash_prefix("zzzz_not_hex_zzzz_not_hex_zzzz_no"), None);
    }

    #[test]
    fn index_table_names_are_per_bucket_and_distinct() {
        assert_eq!(pid_index_name(7), "idx:pid@7");
        assert_eq!(ppid_index_name(7), "idx:ppid@7");
        assert_eq!(name_index_name(7), "idx:name@7");
        assert_eq!(exe_index_name(7), "idx:exe_hash@7");
        assert_ne!(pid_index_name(7), pid_index_name(8));
    }
}
