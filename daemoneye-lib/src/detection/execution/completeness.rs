//! Folds one cycle's signals into the completeness of each rule's evaluation (R14, R15, R16;
//! KTD11).
//!
//! The agent builds a [`CycleSignals`] from what it observed (the collection result, the
//! collector registry's heartbeat status, the ingest counters' deltas) and the executor asks the
//! [`CompletenessTracker`] for each rule. Signals are keyed by collector, so a failure of one
//! collector never degrades a rule over another collector's table.
//!
//! Reasons come out in a fixed order so two runs over the same signals compare equal: collector
//! availability, the collector's own collection error, sequence gaps for that collector, ingest
//! backpressure, then the rule's own execution reasons in the order [`execution_reasons`] lists.

use std::collections::BTreeMap;

use crate::detection::Generation;
use crate::detection::execution::executor::{EvaluationFailure, RuleEvaluation, ScanTotals};
use crate::models::{Completeness, CompletenessReason};

/// A collector's heartbeat health, as the agent's registry reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CollectorHealth {
    /// Heartbeats are arriving.
    Healthy,
    /// Some heartbeats missed, below the failure threshold. Does not degrade evaluations.
    Degraded {
        /// Consecutive missed heartbeats.
        missed_count: u32,
    },
    /// Too many heartbeats missed; the collector is treated as unavailable.
    Failed {
        /// Consecutive missed heartbeats.
        missed_count: u32,
    },
}

impl CollectorHealth {
    const fn is_failed(self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

pub use crate::storage::ingest::SequenceGap;

/// What ingest did this cycle, as deltas since the previous cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSnapshot {
    /// Growth of `IngestMetrics::saturation_alerts`.
    pub saturation_delta: u64,
    /// Gaps detected this cycle.
    pub sequence_gaps: Vec<SequenceGap>,
}

/// Everything the agent observed about one cycle, keyed by collector id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleSignals {
    /// The collection result per collector: `Err` carries the error text.
    pub collection: BTreeMap<String, Result<(), String>>,
    /// The heartbeat health per collector.
    pub heartbeat: BTreeMap<String, CollectorHealth>,
    /// Ingest's deltas.
    pub ingest: IngestSnapshot,
}

/// What the engine keeps of a rule's latest evaluation, for the operator surface to read (T10).
///
/// Alerts are not kept: they are delivered or persisted by the agent. What an operator needs
/// afterwards is whether the rule's last run was complete and, if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationSummary {
    /// The rule evaluated.
    pub rule_id: String,
    /// The generation it was evaluated under, so a reader can tell a summary of a superseded load.
    pub generation: Generation,
    /// Whether the evaluation was complete, and if not, why.
    pub completeness: Completeness,
    /// How many alerts the evaluation produced.
    pub alert_count: usize,
}

impl EvaluationSummary {
    /// The summary of `evaluation`.
    #[must_use]
    pub fn of(evaluation: &RuleEvaluation) -> Self {
        Self {
            rule_id: evaluation.rule_id.clone(),
            generation: evaluation.generation,
            completeness: evaluation.completeness.clone(),
            alert_count: evaluation.alerts.len(),
        }
    }
}

/// Turns [`CycleSignals`] into a [`Completeness`] per rule.
#[derive(Debug, Clone)]
pub struct CompletenessTracker {
    signals: CycleSignals,
}

impl CompletenessTracker {
    /// A tracker over `signals`.
    #[must_use]
    pub fn new(signals: &CycleSignals) -> Self {
        Self {
            signals: signals.clone(),
        }
    }

    /// The completeness of a rule over `table`, owned by `collector_id`, whose own run produced
    /// `execution_reasons`.
    #[must_use]
    pub fn for_rule(
        &self,
        collector_id: &str,
        table: &str,
        execution_reasons: Vec<CompletenessReason>,
    ) -> Completeness {
        let signals = &self.signals;
        let collection_error = signals
            .collection
            .get(collector_id)
            .and_then(|result| result.as_ref().err());
        let heartbeat_failed = signals
            .heartbeat
            .get(collector_id)
            .is_some_and(|health| health.is_failed());

        let mut reasons = Vec::new();
        if collection_error.is_some() || heartbeat_failed {
            reasons.push(CompletenessReason::CollectorUnavailable {
                collector_id: collector_id.to_owned(),
                table: table.to_owned(),
            });
        }
        if let Some(error) = collection_error {
            reasons.push(CompletenessReason::CollectionFailed {
                collector_id: collector_id.to_owned(),
                error: error.clone(),
            });
        }
        reasons.extend(
            signals
                .ingest
                .sequence_gaps
                .iter()
                .filter(|gap| gap.collector_id == collector_id)
                .map(|gap| CompletenessReason::SequenceGapDetected {
                    collector_id: gap.collector_id.clone(),
                    expected_seq: gap.expected_seq,
                    observed_seq: gap.observed_seq,
                }),
        );
        if signals.ingest.saturation_delta > 0 {
            reasons.push(CompletenessReason::Shed {
                discarded: signals.ingest.saturation_delta,
            });
        }
        reasons.extend(execution_reasons);
        Completeness::from_reasons(reasons)
    }
}

/// The reasons one rule's own run was incomplete, in a fixed order: the failure, a latency stop,
/// oversized rows, the result cap.
///
/// A latency stop has no reason of its own. The guard stops reading batches once a pattern is too
/// slow, so the run saw less than a complete scan and a "no match" would be false; the limit that
/// was hit is a time budget, so it is reported as a `ResourceLimit`.
#[must_use]
pub fn execution_reasons(
    failure: Option<&EvaluationFailure>,
    stopped_on_latency: bool,
    scan: &ScanTotals,
    result_capped: Option<u32>,
) -> Vec<CompletenessReason> {
    let mut reasons = Vec::new();
    if let Some(cause) = failure {
        reasons.push(match *cause {
            EvaluationFailure::Execution(ref detail) => CompletenessReason::ExecutionError {
                detail: detail.clone(),
            },
            EvaluationFailure::ResourceLimit(ref detail) => CompletenessReason::ResourceLimit {
                detail: detail.clone(),
            },
        });
    }
    if stopped_on_latency {
        reasons.push(CompletenessReason::ResourceLimit {
            detail: format!(
                "pattern latency threshold breached scanning `{}`; remaining batches were not read",
                scan.table
            ),
        });
    }
    if scan.oversized_rows > 0 {
        reasons.push(CompletenessReason::ResourceLimit {
            detail: format!(
                "{} row(s) of `{}` exceeded the batch byte bound and were not evaluated",
                scan.oversized_rows, scan.table
            ),
        });
    }
    if let Some(cap) = result_capped {
        reasons.push(CompletenessReason::ResultCapped { cap });
    }
    reasons
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::models::CompletenessStatus;

    fn signals() -> CycleSignals {
        CycleSignals {
            collection: BTreeMap::new(),
            heartbeat: BTreeMap::new(),
            ingest: IngestSnapshot::default(),
        }
    }

    fn fold(signals: &CycleSignals) -> Completeness {
        CompletenessTracker::new(signals).for_rule("procmond", "processes", Vec::new())
    }

    fn scan(oversized_rows: u64) -> ScanTotals {
        ScanTotals {
            table: "processes".to_owned(),
            rows_read: 0,
            oversized_rows,
            batches: 0,
        }
    }

    #[test]
    fn a_failed_heartbeat_with_three_missed_is_unavailable() {
        let mut s = signals();
        s.heartbeat.insert(
            "procmond".to_owned(),
            CollectorHealth::Failed { missed_count: 3 },
        );
        assert_eq!(
            fold(&s).reasons(),
            [CompletenessReason::CollectorUnavailable {
                collector_id: "procmond".to_owned(),
                table: "processes".to_owned(),
            }]
        );
    }

    #[test]
    fn a_degraded_heartbeat_with_one_missed_is_not() {
        let mut s = signals();
        s.heartbeat.insert(
            "procmond".to_owned(),
            CollectorHealth::Degraded { missed_count: 1 },
        );
        assert_eq!(fold(&s).status(), CompletenessStatus::Complete);
    }

    #[test]
    fn a_collection_error_and_a_failed_heartbeat_name_the_collector_once() {
        let mut s = signals();
        s.heartbeat.insert(
            "procmond".to_owned(),
            CollectorHealth::Failed { missed_count: 5 },
        );
        s.collection
            .insert("procmond".to_owned(), Err("x".to_owned()));
        let unavailable = fold(&s)
            .reasons()
            .iter()
            .filter(|r| matches!(r, CompletenessReason::CollectorUnavailable { .. }))
            .count();
        assert_eq!(unavailable, 1);
    }

    #[test]
    fn a_failed_heartbeat_for_another_collector_leaves_this_one_complete() {
        let mut s = signals();
        s.heartbeat.insert(
            "other".to_owned(),
            CollectorHealth::Failed { missed_count: 3 },
        );
        s.collection.insert("other".to_owned(), Err("x".to_owned()));
        assert_eq!(fold(&s), Completeness::complete());
    }

    #[test]
    fn a_saturation_delta_of_two_sheds_two() {
        let mut s = signals();
        s.ingest.saturation_delta = 2;
        assert_eq!(
            fold(&s).reasons(),
            [CompletenessReason::Shed { discarded: 2 }]
        );
    }

    #[test]
    fn a_run_with_nothing_wrong_yields_no_execution_reason() {
        assert!(execution_reasons(None, false, &scan(0), None).is_empty());
    }

    #[test]
    fn a_latency_stop_is_a_resource_limit_naming_the_table() {
        let reasons = execution_reasons(None, true, &scan(0), None);
        assert_eq!(reasons.len(), 1);
        assert!(matches!(
            &reasons[0],
            CompletenessReason::ResourceLimit { detail } if detail.contains("processes")
        ));
    }

    #[test]
    fn failure_kinds_map_to_their_own_reasons_before_the_cap() {
        let execution = execution_reasons(
            Some(&EvaluationFailure::Execution("e".to_owned())),
            false,
            &scan(0),
            Some(4),
        );
        assert_eq!(
            execution,
            [
                CompletenessReason::ExecutionError {
                    detail: "e".to_owned()
                },
                CompletenessReason::ResultCapped { cap: 4 },
            ]
        );
        let limit = execution_reasons(
            Some(&EvaluationFailure::ResourceLimit("m".to_owned())),
            false,
            &scan(0),
            None,
        );
        assert_eq!(
            limit,
            [CompletenessReason::ResourceLimit {
                detail: "m".to_owned()
            }]
        );
    }
}
