//! Non-cryptographic fuzzy (ssdeep/CTPH) hashing for binary-change detection.
//!
//! This module is deliberately **separate** from
//! [`MultiAlgorithmHasher`](super::MultiAlgorithmHasher). ssdeep is a
//! *similarity* hash, not a cryptographic identity hash: it is attacker-malleable
//! and variable-length, so it must never enter the cryptographic
//! [`HashResult::hashes`](super::HashResult) map (which is asserted to hold only
//! cryptographically secure digests). Fuzzy hashing **supplements** — never
//! replaces — the SHA-256 identity hash: a SHA-256 identity change is the
//! authoritative executable-substitution signal; the fuzzy-hash observation is a
//! heuristic for detecting *substantial* binary modification.
//!
//! Computation is gated behind the `fuzzy-hashes` feature. When the feature is
//! disabled the public API still compiles, but every compute/compare call
//! returns [`FuzzyHashError::FeatureDisabled`] so callers need no `cfg` of their
//! own.

use std::io::Read;
use thiserror::Error;

/// Default ssdeep similarity threshold (0–100 scale).
///
/// When a process executable's fuzzy-hash similarity to its previously recorded
/// value falls *below* this threshold, a binary-change observation is emitted
/// (R2 AC7). Chosen as a conservative value: high enough that routine metadata
/// churn rarely trips it, low enough that a substantial binary replacement is
/// caught.
pub const DEFAULT_SSDEEP_SIMILARITY_THRESHOLD: u8 = 80;

/// Inclusive lower bound for a valid [`FuzzyConfig::similarity_threshold`].
pub const MIN_SSDEEP_SIMILARITY_THRESHOLD: u8 = 1;

/// Inclusive upper bound for a valid [`FuzzyConfig::similarity_threshold`].
pub const MAX_SSDEEP_SIMILARITY_THRESHOLD: u8 = 99;

/// Errors from fuzzy-hash computation, comparison, or configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FuzzyHashError {
    /// The `fuzzy-hashes` feature is not enabled in this build.
    #[error("fuzzy hashing is not enabled (build with the `fuzzy-hashes` feature)")]
    FeatureDisabled,

    /// I/O error while reading the input for hashing.
    #[error("failed to read input for fuzzy hashing: {0}")]
    Io(String),

    /// The fuzzy-hash engine failed to compute or compare a digest.
    #[error("fuzzy hash engine error: {0}")]
    Engine(String),

    /// A configuration value was outside its accepted range.
    #[error("invalid fuzzy-hash configuration: {0}")]
    InvalidConfig(String),
}

/// Configuration for fuzzy-hash binary-change detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyConfig {
    /// Similarity score (0–100) *below* which a binary-change observation fires.
    pub similarity_threshold: u8,
}

impl Default for FuzzyConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: DEFAULT_SSDEEP_SIMILARITY_THRESHOLD,
        }
    }
}

impl FuzzyConfig {
    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`FuzzyHashError::InvalidConfig`] when `similarity_threshold` is
    /// outside the inclusive range `[1, 99]`. A threshold of `0` would never
    /// fire (no similarity is below 0%) and `100` would fire on every
    /// non-identical hash, so both extremes are rejected to prevent a
    /// misconfiguration from silently disabling or saturating the observation.
    pub fn validate(&self) -> Result<(), FuzzyHashError> {
        if !(MIN_SSDEEP_SIMILARITY_THRESHOLD..=MAX_SSDEEP_SIMILARITY_THRESHOLD)
            .contains(&self.similarity_threshold)
        {
            return Err(FuzzyHashError::InvalidConfig(format!(
                "similarity_threshold {} must be within [{MIN_SSDEEP_SIMILARITY_THRESHOLD}, \
                 {MAX_SSDEEP_SIMILARITY_THRESHOLD}]",
                self.similarity_threshold,
            )));
        }
        Ok(())
    }
}

/// Compute an ssdeep/CTPH fuzzy hash over an in-memory byte buffer.
///
/// # Errors
///
/// Returns [`FuzzyHashError::FeatureDisabled`] when the `fuzzy-hashes` feature
/// is not compiled in.
#[cfg(feature = "fuzzy-hashes")]
pub fn compute_ssdeep_from_bytes(bytes: &[u8]) -> Result<String, FuzzyHashError> {
    Ok(fuzzyhash::FuzzyHash::new(bytes).to_string())
}

/// Stub used when the `fuzzy-hashes` feature is disabled.
///
/// # Errors
///
/// Always returns [`FuzzyHashError::FeatureDisabled`].
#[cfg(not(feature = "fuzzy-hashes"))]
pub fn compute_ssdeep_from_bytes(bytes: &[u8]) -> Result<String, FuzzyHashError> {
    let _unused: &[u8] = bytes;
    Err(FuzzyHashError::FeatureDisabled)
}

/// Compute an ssdeep/CTPH fuzzy hash by streaming a reader to completion.
///
/// This is the preferred entry point for hashing executables: it streams the
/// already-authorized file handle rather than buffering the whole file.
///
/// # Errors
///
/// Returns [`FuzzyHashError::Io`] if the reader fails, or
/// [`FuzzyHashError::FeatureDisabled`] when the `fuzzy-hashes` feature is not
/// compiled in.
#[cfg(feature = "fuzzy-hashes")]
pub fn compute_ssdeep_from_reader<R: Read>(reader: &mut R) -> Result<String, FuzzyHashError> {
    let hash =
        fuzzyhash::FuzzyHash::read(reader).map_err(|err| FuzzyHashError::Io(err.to_string()))?;
    Ok(hash.to_string())
}

/// Stub used when the `fuzzy-hashes` feature is disabled.
///
/// # Errors
///
/// Always returns [`FuzzyHashError::FeatureDisabled`].
#[cfg(not(feature = "fuzzy-hashes"))]
pub fn compute_ssdeep_from_reader<R: Read>(reader: &mut R) -> Result<String, FuzzyHashError> {
    let _unused: &mut R = reader;
    Err(FuzzyHashError::FeatureDisabled)
}

/// Compute the similarity (0–100) between two ssdeep digest strings.
///
/// A score of `100` means the digests are identical; `0` means no detectable
/// similarity.
///
/// # Errors
///
/// Returns [`FuzzyHashError::Engine`] if the digests cannot be compared or the
/// score is out of range, or [`FuzzyHashError::FeatureDisabled`] when the
/// `fuzzy-hashes` feature is not compiled in.
#[cfg(feature = "fuzzy-hashes")]
pub fn similarity(first: &str, second: &str) -> Result<u8, FuzzyHashError> {
    let score = fuzzyhash::FuzzyHash::compare(first, second)
        .map_err(|err| FuzzyHashError::Engine(err.to_string()))?;
    u8::try_from(score).map_err(|err| {
        FuzzyHashError::Engine(format!("similarity score {score} out of range: {err}"))
    })
}

/// Stub used when the `fuzzy-hashes` feature is disabled.
///
/// # Errors
///
/// Always returns [`FuzzyHashError::FeatureDisabled`].
#[cfg(not(feature = "fuzzy-hashes"))]
pub fn similarity(first: &str, second: &str) -> Result<u8, FuzzyHashError> {
    let _unused: (&str, &str) = (first, second);
    Err(FuzzyHashError::FeatureDisabled)
}

#[cfg(all(test, feature = "fuzzy-hashes"))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    /// A buffer large enough that ssdeep produces a meaningful (non-degenerate)
    /// digest. ssdeep needs a few KiB before its block hashes carry signal.
    fn make_buffer(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(u8::try_from(i % 251).unwrap_or(0)))
            .collect()
    }

    #[test]
    fn computes_nonempty_digest_for_real_input() {
        let digest =
            compute_ssdeep_from_bytes(&make_buffer(7, 8192)).expect("compute should succeed");
        assert!(!digest.is_empty(), "digest should not be empty");
        // ssdeep digests are `<blocksize>:<b64>:<b64>`.
        assert_eq!(
            digest.matches(':').count(),
            2,
            "ssdeep digest has two colons: {digest}"
        );
    }

    #[test]
    fn identical_inputs_have_maximum_similarity() {
        let buf = make_buffer(3, 16384);
        let a = compute_ssdeep_from_bytes(&buf).expect("compute a");
        let b = compute_ssdeep_from_bytes(&buf).expect("compute b");
        assert_eq!(similarity(&a, &b).expect("compare"), 100);
    }

    #[test]
    fn unrelated_inputs_have_low_similarity() {
        let a = compute_ssdeep_from_bytes(&make_buffer(1, 16384)).expect("compute a");
        let b = compute_ssdeep_from_bytes(&make_buffer(200, 16384)).expect("compute b");
        let score = similarity(&a, &b).expect("compare");
        assert!(
            score < 50,
            "unrelated buffers should score low, got {score}"
        );
    }

    #[test]
    fn small_modification_keeps_high_similarity() {
        let base = make_buffer(5, 32768);
        let mut modified = base.clone();
        if let Some(byte) = modified.get_mut(100) {
            *byte = byte.wrapping_add(1);
        }
        let a = compute_ssdeep_from_bytes(&base).expect("compute base");
        let b = compute_ssdeep_from_bytes(&modified).expect("compute modified");
        let score = similarity(&a, &b).expect("compare");
        assert!(
            score > 80,
            "one-byte change should stay similar, got {score}"
        );
    }

    #[test]
    fn empty_input_does_not_panic() {
        // Degenerate input must not panic; either a digest or a typed error.
        let _result = compute_ssdeep_from_bytes(&[]);
    }

    #[test]
    fn reader_and_bytes_agree() {
        let buf = make_buffer(9, 16384);
        let from_bytes = compute_ssdeep_from_bytes(&buf).expect("bytes");
        let from_reader =
            compute_ssdeep_from_reader(&mut std::io::Cursor::new(&buf)).expect("reader");
        assert_eq!(from_bytes, from_reader);
    }

    #[test]
    fn config_validate_accepts_default() {
        assert!(FuzzyConfig::default().validate().is_ok());
        assert_eq!(
            FuzzyConfig::default().similarity_threshold,
            DEFAULT_SSDEEP_SIMILARITY_THRESHOLD
        );
    }

    #[test]
    fn config_validate_rejects_out_of_range() {
        assert!(
            FuzzyConfig {
                similarity_threshold: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            FuzzyConfig {
                similarity_threshold: 100
            }
            .validate()
            .is_err()
        );
        assert!(
            FuzzyConfig {
                similarity_threshold: 200
            }
            .validate()
            .is_err()
        );
        assert!(
            FuzzyConfig {
                similarity_threshold: 1
            }
            .validate()
            .is_ok()
        );
        assert!(
            FuzzyConfig {
                similarity_threshold: 99
            }
            .validate()
            .is_ok()
        );
    }
}
