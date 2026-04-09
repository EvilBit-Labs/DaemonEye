//! Triggered collector framework.
//!
//! Defines the [`TriggerableCollector`] trait — a reactive collector that
//! responds to [`TriggerRequest`] events rather than producing events on its
//! own schedule like a [`crate::source::EventSource`] does. Triggered collectors are the
//! extension point for on-demand analysis collectors such as the binary
//! hasher, YARA scanner, or memory analyzer.
//!
//! # Why not extend `EventSource`?
//!
//! `EventSource::start()` takes an `mpsc::Sender<CollectionEvent>` and a
//! shutdown signal — it models a long-running producer on its own schedule.
//! A triggered collector is reactive: it waits for a `TriggerRequest` and
//! produces an `AnalysisResult`. Forcing an `EventSource` supertrait would
//! make `start()` a dummy sleep loop burning a worker task.
//!
//! Instead, the runtime holds a parallel registry keyed by
//! [`TriggerableCollector::name`]. Dispatch is driven by the existing
//! [`crate::trigger::TriggerManager`] when a matching `TriggerRequest` flows
//! through the bus.
//!
//! # Response routing
//!
//! Responses flow via `oneshot::Sender<Result<AnalysisResult, _>>` held by
//! the dispatcher, **not** as a new [`crate::event::CollectionEvent`] variant. This:
//!
//! - Avoids polluting the fan-out event bus with responses that only matter
//!   to the original requester.
//! - Reuses the existing [`crate::analysis_chain::AnalysisResult`] type
//!   instead of duplicating it.
//! - Matches the precedent set by
//!   [`crate::analysis_chain::AnalysisChainCoordinator`].
//!
//! # Error sanitization
//!
//! Triggered collectors may produce rich internal errors (file paths, OS
//! error messages, syscall failures). When a response crosses any process
//! or serialization boundary, those internals must be mapped to the closed
//! [`TriggerErrorKind`] enum so they cannot leak filesystem layout, user
//! data, or file-existence oracles to unauthorized subscribers. See the
//! security section of
//! `docs/plans/2026-04-07-001-feat-binary-hashing-integrity-plan.md`.

use crate::analysis_chain::AnalysisResult;
use crate::event::{AnalysisType, TriggerRequest};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// TriggerErrorKind (wire-safe, closed enum)
// ─────────────────────────────────────────────────────────────────────────────

/// Wire-safe error kinds for triggered analysis responses.
///
/// Rich internal errors (with paths, OS error text, etc.) are mapped to this
/// closed enum before crossing any serialization or process boundary. The
/// mapping intentionally merges [`TriggerErrorKind::Unavailable`] for both
/// "file not found" and "permission denied" so a caller cannot use
/// differential error responses as a file-existence oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TriggerErrorKind {
    /// The target was not reachable. Covers both permission-denied and
    /// not-found to prevent file-existence oracles.
    Unavailable,
    /// The target exceeded a size limit enforced by the collector.
    TooLarge,
    /// The request deadline expired before completion.
    Timeout,
    /// The target path was not in the collector's allow-list.
    PathNotAllowed,
    /// The collector's resource budget was saturated.
    ResourceExhausted,
    /// The request was invalid (missing fields, bad length, etc.).
    InvalidRequest,
    /// An internal error occurred. Details are logged locally at `warn`
    /// level but not exposed on the wire.
    Internal,
}

impl fmt::Display for TriggerErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Unavailable => "unavailable",
            Self::TooLarge => "too_large",
            Self::Timeout => "timeout",
            Self::PathNotAllowed => "path_not_allowed",
            Self::ResourceExhausted => "resource_exhausted",
            Self::InvalidRequest => "invalid_request",
            Self::Internal => "internal",
        };
        f.write_str(name)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TriggerHandleError (internal, rich context)
// ─────────────────────────────────────────────────────────────────────────────

/// Rich internal error from [`TriggerableCollector::handle_trigger`].
///
/// This type carries full context (paths, OS errors, messages) for local
/// `tracing` logs and test assertions. It **must** be converted to
/// [`TriggerErrorKind`] via [`TriggerHandleError::kind`] before any
/// serialization or cross-boundary transmission.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TriggerHandleError {
    /// The request was missing a required field.
    #[error("trigger request missing field: {field}")]
    MissingField {
        /// Name of the missing field.
        field: &'static str,
    },
    /// The request path was longer than the collector's hard limit.
    #[error("trigger request path too long ({len} bytes)")]
    PathTooLong {
        /// Observed path length in bytes.
        len: usize,
    },
    /// The request contained a malformed or rejected path.
    #[error("trigger request path rejected: {reason}")]
    PathRejected {
        /// Why the path was rejected (logged locally, not on the wire).
        reason: String,
    },
    /// The target path was not inside any of the collector's allowed roots.
    #[error("target path not in allowed roots")]
    PathNotAllowed,
    /// The target could not be reached (permission denied or not found).
    /// Merged on the wire to prevent file-existence oracles.
    #[error("target unavailable: {reason}")]
    Unavailable {
        /// Reason (logged locally, not on the wire).
        reason: String,
    },
    /// The target exceeded the collector's size limit.
    #[error("target too large: {size} bytes (limit {limit})")]
    TooLarge {
        /// Observed size.
        size: u64,
        /// Configured limit.
        limit: u64,
    },
    /// The request deadline expired.
    #[error("trigger handler timed out")]
    Timeout,
    /// The collector's resource budget was saturated.
    #[error("collector resource budget exhausted")]
    ResourceExhausted,
    /// An unexpected error occurred inside the collector.
    #[error("internal error: {0}")]
    Internal(String),
}

impl TriggerHandleError {
    /// Map this rich internal error to a wire-safe closed-enum kind.
    ///
    /// `PermissionDenied`/`NotFound`-style variants both map to
    /// [`TriggerErrorKind::Unavailable`] to prevent file-existence oracles.
    #[must_use]
    pub const fn kind(&self) -> TriggerErrorKind {
        match *self {
            Self::MissingField { .. } | Self::PathRejected { .. } => {
                TriggerErrorKind::InvalidRequest
            }
            Self::PathTooLong { .. } => TriggerErrorKind::InvalidRequest,
            Self::PathNotAllowed => TriggerErrorKind::PathNotAllowed,
            Self::Unavailable { .. } => TriggerErrorKind::Unavailable,
            Self::TooLarge { .. } => TriggerErrorKind::TooLarge,
            Self::Timeout => TriggerErrorKind::Timeout,
            Self::ResourceExhausted => TriggerErrorKind::ResourceExhausted,
            Self::Internal(_) => TriggerErrorKind::Internal,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TriggerableCollector trait
// ─────────────────────────────────────────────────────────────────────────────

/// A reactive collector that produces [`AnalysisResult`]s in response to
/// [`TriggerRequest`]s.
///
/// Unlike [`crate::source::EventSource`], a `TriggerableCollector` does not
/// own a long-running task. The runtime's
/// [`crate::trigger::TriggerManager`] dispatches a `TriggerRequest` to the
/// collector whose [`Self::name`] matches
/// [`TriggerRequest::target_collector`], then delivers the result via a
/// `oneshot::Sender` held by the dispatcher.
///
/// Workspace convention: use native `async fn` with
/// `#[allow(async_fn_in_trait)]` (matches [`crate::source::EventSource`] and
/// [`crate::monitor_collector::MonitorCollector`]).
#[allow(async_fn_in_trait)]
pub trait TriggerableCollector: Send + Sync {
    /// Stable name of this collector. Must match
    /// [`TriggerRequest::target_collector`] exactly for the dispatcher to
    /// route to this instance.
    fn name(&self) -> &'static str;

    /// Analysis types this collector can handle. The dispatcher consults
    /// this list for validation before calling [`Self::handle_trigger`].
    fn supported_analysis_types(&self) -> &[AnalysisType];

    /// Handle a single trigger request. Implementations must respect the
    /// request's deadline (derived from the `TriggerManager`'s
    /// configuration) and produce a well-formed [`AnalysisResult`] on
    /// success or a [`TriggerHandleError`] on failure.
    ///
    /// # Errors
    ///
    /// Returns [`TriggerHandleError`] for authorization failures, resource
    /// exhaustion, I/O errors, timeout, or invalid input. Rich errors are
    /// logged locally; only the result of [`TriggerHandleError::kind`]
    /// crosses any serialization boundary.
    async fn handle_trigger(
        &self,
        request: &TriggerRequest,
    ) -> Result<AnalysisResult, TriggerHandleError>;

    /// Health check for this collector. Default implementation returns
    /// `Ok(())`.
    ///
    /// # Errors
    ///
    /// Implementations may return [`TriggerHandleError::Internal`] to
    /// signal unhealthy state.
    async fn health_check(&self) -> Result<(), TriggerHandleError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::event::{AnalysisType, TriggerPriority, TriggerRequest};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    // ── TriggerErrorKind serialization stability ────────────────────────

    #[test]
    fn trigger_error_kind_display_is_stable() {
        assert_eq!(TriggerErrorKind::Unavailable.to_string(), "unavailable");
        assert_eq!(TriggerErrorKind::TooLarge.to_string(), "too_large");
        assert_eq!(TriggerErrorKind::Timeout.to_string(), "timeout");
        assert_eq!(
            TriggerErrorKind::PathNotAllowed.to_string(),
            "path_not_allowed"
        );
        assert_eq!(
            TriggerErrorKind::ResourceExhausted.to_string(),
            "resource_exhausted"
        );
        assert_eq!(
            TriggerErrorKind::InvalidRequest.to_string(),
            "invalid_request"
        );
        assert_eq!(TriggerErrorKind::Internal.to_string(), "internal");
    }

    #[test]
    fn trigger_error_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&TriggerErrorKind::PathNotAllowed).unwrap();
        assert_eq!(json, "\"path_not_allowed\"");
    }

    // ── PermissionDenied + NotFound merge to Unavailable ────────────────

    #[test]
    fn permission_denied_and_not_found_both_map_to_unavailable() {
        // Both "file not found" style and "permission denied" style errors
        // MUST map to the same wire kind. This is the file-existence oracle
        // defense required by the security review (H3) in the plan.
        let not_found = TriggerHandleError::Unavailable {
            reason: "file not found".to_owned(),
        };
        let perm_denied = TriggerHandleError::Unavailable {
            reason: "permission denied".to_owned(),
        };
        assert_eq!(not_found.kind(), TriggerErrorKind::Unavailable);
        assert_eq!(perm_denied.kind(), TriggerErrorKind::Unavailable);
        assert_eq!(not_found.kind(), perm_denied.kind());
    }

    #[test]
    fn error_kinds_round_trip_through_wire_mapping() {
        let cases: Vec<(TriggerHandleError, TriggerErrorKind)> = vec![
            (
                TriggerHandleError::MissingField { field: "path" },
                TriggerErrorKind::InvalidRequest,
            ),
            (
                TriggerHandleError::PathTooLong { len: 9999 },
                TriggerErrorKind::InvalidRequest,
            ),
            (
                TriggerHandleError::PathNotAllowed,
                TriggerErrorKind::PathNotAllowed,
            ),
            (
                TriggerHandleError::TooLarge {
                    size: 2_000,
                    limit: 1_000,
                },
                TriggerErrorKind::TooLarge,
            ),
            (TriggerHandleError::Timeout, TriggerErrorKind::Timeout),
            (
                TriggerHandleError::ResourceExhausted,
                TriggerErrorKind::ResourceExhausted,
            ),
            (
                TriggerHandleError::Internal("boom".to_owned()),
                TriggerErrorKind::Internal,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.kind(), expected);
        }
    }

    // ── Trait contract test using an in-memory mock ─────────────────────

    fn sample_trigger(target: &str) -> TriggerRequest {
        TriggerRequest {
            trigger_id: "t-1".to_owned(),
            target_collector: target.to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority: TriggerPriority::Normal,
            target_pid: Some(1234),
            target_path: Some("/usr/bin/ls".to_owned()),
            correlation_id: "c-1".to_owned(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        }
    }

    struct CountingCollector {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl TriggerableCollector for CountingCollector {
        fn name(&self) -> &'static str {
            self.name
        }

        fn supported_analysis_types(&self) -> &[AnalysisType] {
            const TYPES: &[AnalysisType] = &[AnalysisType::BinaryHash];
            TYPES
        }

        async fn handle_trigger(
            &self,
            request: &TriggerRequest,
        ) -> Result<AnalysisResult, TriggerHandleError> {
            let _ = self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(AnalysisResult {
                stage_id: "mock-stage".to_owned(),
                analysis_type: request.analysis_type.clone(),
                collector_id: self.name.to_owned(),
                result_data: serde_json::json!({"mock": true}),
                metadata: HashMap::new(),
                completed_at: SystemTime::now(),
                execution_duration: Duration::from_millis(1),
                confidence: 1.0,
            })
        }
    }

    #[tokio::test]
    async fn collector_handles_trigger_and_increments_counter() {
        let calls = Arc::new(AtomicUsize::new(0));
        let collector = CountingCollector {
            name: "mock-hasher",
            calls: Arc::clone(&calls),
        };
        assert_eq!(collector.name(), "mock-hasher");
        assert_eq!(
            collector.supported_analysis_types(),
            &[AnalysisType::BinaryHash]
        );

        let req = sample_trigger("mock-hasher");
        let result = collector.handle_trigger(&req).await.unwrap();
        assert_eq!(result.collector_id, "mock-hasher");
        assert_eq!(result.analysis_type, AnalysisType::BinaryHash);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn default_health_check_is_ok() {
        let collector = CountingCollector {
            name: "mock",
            calls: Arc::new(AtomicUsize::new(0)),
        };
        collector.health_check().await.unwrap();
    }
}
