//! redb key/value codecs for the event store (T3 · U2).
//!
//! Two concerns live here:
//!
//! 1. [`TsSeqKey`] — the primary-key codec: a fixed-width 16-byte big-endian
//!    encoding of `(ts_ms: u64, seq: u32)` plus 4 reserved bytes (per spec
//!    §11.7.8 `RowKey`). Big-endian layout makes a raw byte comparison equal a
//!    numeric comparison, so [`redb::Key::compare`] is a cheap slice compare.
//!    The key is unique (via `seq`) and roughly time-ordered, not globally
//!    strictly increasing — a clock step-back stores the true `ts_ms` without
//!    violating uniqueness.
//!
//! 2. Value codec helpers ([`encode_value`] / [`decode_value`]) — version-tagged
//!    postcard encoding. The redb value column itself is a plain byte slice
//!    (`&[u8]`), so decoding stays in our code where it can return a
//!    [`StorageError`] rather than in redb's infallible `Value::from_bytes`.
//!    Evolution is additive: a leading version byte selects the decoder.
//!
//! `from_bytes` implementations are written to never `panic`/`unwrap` (the
//! workspace denies those outside tests); a malformed fixed-width key decodes
//! to a defensive zero rather than aborting the daemon.

use crate::storage::StorageError;
use redb::{Key, TypeName, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::cmp::Ordering;
use std::fmt::Debug;

/// Width of the primary key in bytes: `u64` ts + `u32` seq + 4 reserved.
pub(super) const KEY_WIDTH: usize = 16;

/// Current value-codec version. New versions append optional fields and bump
/// this; [`decode_value`] keeps dispatching on the leading byte.
pub(super) const VALUE_VERSION_V1: u8 = 1;

/// Read the first 8 bytes of `data` as a big-endian `u64`, defaulting to 0 when
/// `data` is too short (cannot happen for a fixed-width key; defensive only).
fn read_u64_be(data: &[u8]) -> u64 {
    data.get(0..8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map_or(0, u64::from_be_bytes)
}

/// Read bytes `8..12` of `data` as a big-endian `u32`, defaulting to 0 when
/// `data` is too short.
fn read_u32_be(data: &[u8]) -> u32 {
    data.get(8..12)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map_or(0, u32::from_be_bytes)
}

/// Primary-key codec for `(ts_ms, seq)`.
///
/// This is a zero-sized marker; the actual key value handled by redb is the
/// `(u64, u32)` tuple ([`redb::Value::SelfType`]).
#[derive(Debug)]
pub(super) struct TsSeqKey;

impl Value for TsSeqKey {
    type SelfType<'a>
        = (u64, u32)
    where
        Self: 'a;
    type AsBytes<'a>
        = [u8; KEY_WIDTH]
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        Some(KEY_WIDTH)
    }

    fn from_bytes<'a>(data: &'a [u8]) -> (u64, u32)
    where
        Self: 'a,
    {
        (read_u64_be(data), read_u32_be(data))
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a (u64, u32)) -> [u8; KEY_WIDTH]
    where
        Self: 'b,
    {
        let mut out = [0_u8; KEY_WIDTH];
        let (ts, seq) = *value;
        out[0..8].copy_from_slice(&ts.to_be_bytes());
        out[8..12].copy_from_slice(&seq.to_be_bytes());
        // bytes 12..16 stay zero (reserved for a future shard discriminator)
        out
    }

    fn type_name() -> TypeName {
        TypeName::new("daemoneye::storage::TsSeqKey")
    }
}

impl Key for TsSeqKey {
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering {
        // Big-endian layout => lexicographic byte order equals numeric order.
        data1.cmp(data2)
    }
}

/// Encode a value with the current version tag.
///
/// Layout: `[version: u8][postcard payload]`.
pub(super) fn encode_value<T: Serialize>(value: &T) -> Result<Vec<u8>, StorageError> {
    let payload = postcard::to_allocvec(value)?;
    let mut out = Vec::with_capacity(payload.len().saturating_add(1));
    out.push(VALUE_VERSION_V1);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decode a value previously written by [`encode_value`], dispatching on the
/// leading version byte. Unknown versions are an explicit error, never a panic.
pub(super) fn decode_value<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StorageError> {
    let (version, payload) = bytes
        .split_first()
        .ok_or_else(|| StorageError::Codec(postcard::Error::DeserializeUnexpectedEnd))?;
    match *version {
        VALUE_VERSION_V1 => Ok(postcard::from_bytes(payload)?),
        other => Err(StorageError::Bucket {
            bucket: "<value>".to_owned(),
            message: format!("unknown value codec version {other}"),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips_and_is_16_bytes() {
        for &(ts, seq) in &[
            (0_u64, 0_u32),
            (1, 2),
            (u64::MAX, u32::MAX),
            (1_700_000_000_000, 42),
        ] {
            let bytes = TsSeqKey::as_bytes(&(ts, seq));
            assert_eq!(bytes.len(), KEY_WIDTH);
            assert_eq!(TsSeqKey::from_bytes(&bytes), (ts, seq));
            // reserved bytes are zero
            assert_eq!(&bytes[12..16], &[0_u8; 4]);
        }
    }

    #[test]
    fn key_ordering_matches_numeric_order() {
        // Ordering across the ts_ms boundary.
        let a = TsSeqKey::as_bytes(&(100, 5));
        let b = TsSeqKey::as_bytes(&(101, 0));
        assert_eq!(TsSeqKey::compare(&a, &b), Ordering::Less);

        // Within equal ts_ms, seq disambiguates.
        let c = TsSeqKey::as_bytes(&(100, 4));
        let d = TsSeqKey::as_bytes(&(100, 5));
        assert_eq!(TsSeqKey::compare(&c, &d), Ordering::Less);
        assert_eq!(TsSeqKey::compare(&d, &c), Ordering::Greater);
        assert_eq!(TsSeqKey::compare(&a, &a), Ordering::Equal);
    }

    #[test]
    fn key_fixed_width_is_declared() {
        assert_eq!(TsSeqKey::fixed_width(), Some(16));
    }

    #[test]
    fn key_from_bytes_is_defensive_on_short_input() {
        // Never panics on a malformed (too-short) slice.
        assert_eq!(TsSeqKey::from_bytes(&[]), (0, 0));
        assert_eq!(TsSeqKey::from_bytes(&[1, 2, 3]), (0, 0));
    }

    #[test]
    fn value_round_trips_with_version_byte() {
        let original = ("process".to_owned(), 1234_u32, vec![1_u8, 2, 3]);
        let encoded = encode_value(&original).unwrap();
        assert_eq!(encoded.first(), Some(&VALUE_VERSION_V1));
        let decoded: (String, u32, Vec<u8>) = decode_value(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn value_decode_rejects_unknown_version() {
        let bytes = vec![99_u8, 0, 0];
        let result: Result<(String,), StorageError> = decode_value(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn value_decode_rejects_empty() {
        let result: Result<u32, StorageError> = decode_value(&[]);
        assert!(result.is_err());
    }
}
