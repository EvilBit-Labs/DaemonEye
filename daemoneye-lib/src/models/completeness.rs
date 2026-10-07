//! How completely an evaluation or an alert saw the data it was about (R14; KTD9).
//!
//! A [`Completeness`] says `Complete` or `Degraded`, and a degraded one always says why. The
//! invariant, `Degraded` exactly when the reasons are non-empty, is enforced at every way a value
//! can come into being: the constructors, and deserialization, which goes through the same check.
//! There is deliberately no `Default`: a defaulted completeness would claim `Complete` for a run
//! that never examined anything, the fail-open shape the repo's guard learning warns about.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Whether an evaluation saw everything it was meant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CompletenessStatus {
    /// Nothing known to be missing. Zero matches here means no match.
    Complete,
    /// Something is known to be missing. Zero matches here means "could not fully evaluate".
    Degraded,
}

/// One concrete reason an evaluation is degraded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CompletenessReason {
    /// The collector owning the table failed this cycle's collection or has a failed heartbeat.
    CollectorUnavailable {
        /// The collector that owns the table.
        collector_id: String,
        /// The table the rule reads.
        table: String,
    },
    /// The collector returned an error for this cycle's collection.
    CollectionFailed {
        /// The collector that failed.
        collector_id: String,
        /// The error it reported.
        error: String,
    },
    /// Ingest hit backpressure this cycle.
    Shed {
        /// How many times the channel was found full.
        discarded: u64,
    },
    /// A gap in a collector's `source_seq`: rows between the two values were never ingested.
    SequenceGapDetected {
        /// The collector whose sequence skipped.
        collector_id: String,
        /// The sequence that should have come next.
        expected_seq: u64,
        /// The sequence that arrived instead.
        observed_seq: u64,
    },
    /// The evaluation hit a resource bound and did not read everything.
    ResourceLimit {
        /// What was exhausted.
        detail: String,
    },
    /// More rows matched than the per-rule cap; only the first `cap` became alerts.
    ResultCapped {
        /// The configured cap.
        cap: u32,
    },
    /// The plan could not be built or the run failed.
    ExecutionError {
        /// The engine's diagnostic.
        detail: String,
    },
}

/// A completeness value could not be built because it broke the invariant.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletenessError {
    /// `Degraded` was asked for with no reason.
    #[error("a degraded completeness must carry at least one reason")]
    DegradedWithoutReason,
    /// `Complete` was paired with reasons.
    #[error("a complete completeness must carry no reasons")]
    CompleteWithReasons,
}

/// Whether an evaluation or alert is complete and, if not, exactly why.
///
/// Fields are private so the invariant cannot be bypassed by struct literal or mutation;
/// deserialization is validated through [`TryFrom`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CompletenessWire")]
pub struct Completeness {
    status: CompletenessStatus,
    reasons: Vec<CompletenessReason>,
}

/// The unchecked shape on the wire; only ever turned into a [`Completeness`] by `try_from`.
#[derive(Deserialize)]
struct CompletenessWire {
    status: CompletenessStatus,
    reasons: Vec<CompletenessReason>,
}

impl TryFrom<CompletenessWire> for Completeness {
    type Error = CompletenessError;

    fn try_from(wire: CompletenessWire) -> Result<Self, Self::Error> {
        match (wire.status, wire.reasons.is_empty()) {
            (CompletenessStatus::Degraded, true) => Err(CompletenessError::DegradedWithoutReason),
            (CompletenessStatus::Complete, false) => Err(CompletenessError::CompleteWithReasons),
            (status, _) => Ok(Self {
                status,
                reasons: wire.reasons,
            }),
        }
    }
}

impl Completeness {
    /// Nothing known to be missing.
    #[must_use]
    pub const fn complete() -> Self {
        Self {
            status: CompletenessStatus::Complete,
            reasons: Vec::new(),
        }
    }

    /// Degraded for the given reasons.
    ///
    /// # Errors
    ///
    /// [`CompletenessError::DegradedWithoutReason`] if `reasons` is empty.
    pub fn degraded(reasons: Vec<CompletenessReason>) -> Result<Self, CompletenessError> {
        Self::try_from(CompletenessWire {
            status: CompletenessStatus::Degraded,
            reasons,
        })
    }

    /// `Complete` for an empty list and `Degraded` otherwise: the fold's constructor, for callers
    /// that collect reasons and do not know in advance whether there will be any.
    #[must_use]
    pub const fn from_reasons(reasons: Vec<CompletenessReason>) -> Self {
        let status = if reasons.is_empty() {
            CompletenessStatus::Complete
        } else {
            CompletenessStatus::Degraded
        };
        Self { status, reasons }
    }

    /// The status.
    #[must_use]
    pub const fn status(&self) -> CompletenessStatus {
        self.status
    }

    /// The reasons, empty exactly when the status is `Complete`.
    #[must_use]
    pub fn reasons(&self) -> &[CompletenessReason] {
        &self.reasons
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn shed() -> CompletenessReason {
        CompletenessReason::Shed { discarded: 2 }
    }

    #[test]
    fn degraded_with_no_reason_is_an_error() {
        assert_eq!(
            Completeness::degraded(Vec::new()),
            Err(CompletenessError::DegradedWithoutReason)
        );
    }

    #[test]
    fn degraded_with_a_reason_is_degraded_and_keeps_it() {
        let completeness = Completeness::degraded(vec![shed()]).unwrap();
        assert_eq!(completeness.status(), CompletenessStatus::Degraded);
        assert_eq!(completeness.reasons(), [shed()]);
    }

    #[test]
    fn complete_carries_no_reason() {
        let completeness = Completeness::complete();
        assert_eq!(completeness.status(), CompletenessStatus::Complete);
        assert!(completeness.reasons().is_empty());
    }

    #[test]
    fn from_reasons_follows_the_list() {
        assert_eq!(
            Completeness::from_reasons(Vec::new()),
            Completeness::complete()
        );
        assert_eq!(
            Completeness::from_reasons(vec![shed()]).status(),
            CompletenessStatus::Degraded
        );
    }

    #[test]
    fn a_crafted_payload_cannot_claim_degraded_without_reasons() {
        let payload = r#"{"status":"Degraded","reasons":[]}"#;
        assert!(serde_json::from_str::<Completeness>(payload).is_err());
    }

    #[test]
    fn a_crafted_payload_cannot_claim_complete_with_reasons() {
        let payload = r#"{"status":"Complete","reasons":[{"Shed":{"discarded":1}}]}"#;
        assert!(serde_json::from_str::<Completeness>(payload).is_err());
    }

    #[test]
    fn postcard_rejects_the_same_crafted_payloads_and_round_trips_a_valid_one() {
        #[derive(Serialize)]
        struct Raw {
            status: CompletenessStatus,
            reasons: Vec<CompletenessReason>,
        }
        let bad = postcard::to_allocvec(&Raw {
            status: CompletenessStatus::Degraded,
            reasons: Vec::new(),
        })
        .unwrap();
        assert!(postcard::from_bytes::<Completeness>(&bad).is_err());

        let good =
            Completeness::degraded(vec![shed(), CompletenessReason::ResultCapped { cap: 3 }])
                .unwrap();
        let bytes = postcard::to_allocvec(&good).unwrap();
        assert_eq!(postcard::from_bytes::<Completeness>(&bytes).unwrap(), good);
    }
}
