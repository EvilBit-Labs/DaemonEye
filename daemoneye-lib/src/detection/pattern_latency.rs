//! The latency threshold's fail-closed consequence: T6 measures, this disables (R2, R3, R8).
//!
//! `RuleExecutor` observes how long a compiled pattern actually takes to run against real rows.
//! This engine owns the budget and what happens when a measurement busts it.
//! [`DetectionEngine::observe_pattern_latency`] is the single entry point T6 calls with an
//! already-measured [`Duration`]; nothing in this module executes a pattern or starts a timer.
//!
//! Two decisions about that measurement are settled. ADR-0011: the budget is a breach detector
//! checked at `RecordBatch` boundaries, not a per-match execution limit — `regex` exposes no
//! cancellation, so abandoning a match would keep burning the CPU the budget protects, and the
//! overrun is therefore bounded by one batch. ADR-0012: a report must name the rule *instance* it
//! measured, because a rule id survives a reload and a measurement of a superseded instance could
//! otherwise latch the fresh one and defeat the operator's only recovery. The signature below
//! therefore takes the [`Generation`] the measured plan was issued under, which only
//! [`DetectionEngine::runnable_rules`] can produce, and compares it before doing anything else.
//!
//! Disabling a rule here is three unconditional steps, mirroring the pushed-task-expiry sibling
//! in `task_renewal`: flip `enabled`, drop the compiled plan, mark the rule unhealthy. Expiry only
//! needs the second and third steps — dropping the plan and marking the rule unhealthy — because
//! it concerns just the pushed half and never touches `enabled`. A `REGEXP` pattern may sit in
//! either half, so flipping `enabled` is what stops the residual executor and dropping the plan is
//! what stops `renewal_cycle` from re-issuing — neither alone fails closed on its own against an
//! operator flag flip or against T6's residual path (KTD3).
//!
//! The pushed half's fail-closed guarantee is bounded by `PUSHDOWN_TASK_TTL`, not instant: there
//! is no `Revoke` or `Cancel` message on the wire, so a collector already running the predicate
//! keeps evaluating it until its task lapses unrenewed, up to a full TTL after the breach.
//! "Fail-closed" here means "stops within one TTL of the breach", not "stops at it".
//!
//! The health this leaves behind carries [`UnhealthyCause::LatencyBreach`], for which
//! [`UnhealthyCause::resists_auto_recovery`] is `true`. `RuleHealthRegistry::revalidate` and
//! `RuleHealthRegistry::track` both consult it and will not clear this cause or launder it back
//! to healthy (R8, KTD6) — a guard whose failure path could be undone by an unrelated collector
//! registration, or by the re-planning `track` call that registration triggers, would not be a
//! guard at all. [`DetectionEngine::set_rule_enabled`] refuses to re-enable a rule this guard
//! disabled, naming the reason in its error, rather than silently no-opping. Only
//! [`DetectionEngine::load_rule`] restores a rule this guard disabled.

use std::time::Duration;

use super::DetectionEngine;
use super::generation::Generation;
use super::rule_health::{RuleHealth, UnhealthyCause};
use crate::rejection_log::RejectionReason;

/// Why a latency-disabled rule's health reason reads the way it does, beside
/// `task_renewal::TASK_EXPIRED_REASON`'s shape (R3).
const PATTERN_LATENCY_REASON_PREFIX: &str =
    "the pattern's measured execution time breached the configured latency budget";

impl DetectionEngine {
    /// Report a measured pattern execution against this engine's threshold; disable the owning
    /// rule if it breached it (R3).
    ///
    /// `generation` names the load of the rule that was measured. It is compared first, before the
    /// threshold or any state is read: a report whose generation is not the one the engine
    /// currently holds for `rule_id` — a reload superseded it, the rule was removed, or the id was
    /// never loaded — is discarded with an `info` log naming both generations, and returns
    /// `false` having changed nothing (ADR-0012).
    ///
    /// A current report strictly over the threshold runs three unconditional steps, regardless of
    /// the rule's current `enabled` flag: `enabled` is set `false`, the rule's compiled plan is
    /// removed, and the rule is marked [`super::rule_health::RuleHealth::Unhealthy`] with a
    /// reason naming the observed and threshold durations, in that order (KTD3). The durations
    /// are rendered with [`Duration`]'s own `Debug` formatting (`10.4ms`, not a truncated `10`),
    /// so a sub-millisecond observation cannot round down into a reason that reads as a smaller,
    /// less alarming breach than it was. The breach is recorded twice: a `tracing::warn!` and an
    /// entry in [`crate::rejection_log::RejectionLog`] via
    /// [`crate::rejection_log::RejectionReason::rule_other`]. Neither store survives a restart —
    /// the rejection log is a bounded, in-memory window, not durable retention (that is the
    /// audit-ledger's job) — but the health row itself can be cleared by a later `load_rule` or
    /// `remove_rule` (both call `forget`), and nothing in `DetectionEngine` ever calls
    /// `RuleHealthRegistry::unhealthy()`. The rejection log entry is this engine's one queryable
    /// trace of why the rule stopped running that survives a reload of the same rule (R12). If
    /// the health registry could not record the verdict, a `tracing::error!` names the rule id —
    /// a silently unrecorded disable is worse than no guard.
    ///
    /// An observation at or under the threshold changes nothing. Idempotency is keyed on health,
    /// not on `enabled`: a rule already [`super::rule_health::RuleHealth::Unhealthy`] for a cause
    /// that [`UnhealthyCause::resists_auto_recovery`] is left exactly as it is — a second breach
    /// does not overwrite the first breach's reason, and does not re-run the three steps, so an
    /// operator's `set_rule_enabled(id, false)` on a still-healthy rule cannot suppress a later
    /// breach on it.
    ///
    /// Returns `true` exactly when a current-generation observation breached the threshold.
    pub fn observe_pattern_latency(
        &mut self,
        rule_id: &str,
        generation: Generation,
        observed: Duration,
    ) -> bool {
        let current = self.generations.current(rule_id);
        if current != Some(generation) {
            let current_generation = current.map_or_else(|| "none".to_owned(), |g| g.to_string());
            tracing::info!(
                rule_id,
                reported_generation = %generation,
                current_generation = %current_generation,
                "pattern latency report for a superseded rule instance discarded"
            );
            return false;
        }
        if observed <= self.pattern_latency_threshold {
            return false;
        }

        let already_latched = self
            .health
            .health(rule_id)
            .is_some_and(RuleHealth::resists_auto_recovery);
        if already_latched {
            return true;
        }

        let threshold = self.pattern_latency_threshold;
        let reason = format!(
            "{PATTERN_LATENCY_REASON_PREFIX}: {observed:?} observed against a {threshold:?} budget"
        );

        if let Some(rule) = self.rules.get_mut(rule_id) {
            rule.enabled = false;
        }
        let _uncovered = self.compiled.remove(rule_id);
        // `mark_unhealthy` returns `true` on every path when `cause.resists_auto_recovery()` is
        // `true`, which `LatencyBreach` always is, so this branch cannot fire today. It stays as
        // defence-in-depth against a future change to that policy: the rule is disabled either
        // way, and losing the health-registry record of why would be worse than a log line about
        // an unreachable branch that stopped being unreachable.
        let marked = self
            .health
            .mark_unhealthy(rule_id, &reason, UnhealthyCause::LatencyBreach);
        self.rejections
            .record(RejectionReason::rule_other(rule_id, &reason));
        if !marked {
            tracing::error!(
                rule_id,
                "pattern latency breach could not be recorded in rule health; the rule is \
                 disabled with no health record of why"
            );
        }
        tracing::warn!(
            rule_id,
            observed = ?observed,
            threshold = ?threshold,
            "pattern latency breached the configured budget; rule disabled"
        );
        true
    }
}
