//! The latency threshold's fail-closed consequence: T6 measures, this disables (R2, R3, R8).
//!
//! T6 owns observing how long a compiled pattern actually takes to run against real rows — that
//! needs the `DataFusion` executor, which does not exist yet (session-settled: user-approved). This
//! engine owns the budget and what happens when a measurement busts it.
//! [`DetectionEngine::observe_pattern_latency`] is the single entry point T6 calls with an
//! already-measured [`Duration`]; nothing in this module executes a pattern or starts a timer.
//!
//! Disabling a rule here is three unconditional steps, mirroring the pushed-task-expiry sibling
//! in `task_renewal`: flip `enabled`, drop the compiled plan, mark the rule unhealthy. Expiry
//! only needs the second step, because it concerns just the pushed half. A `REGEXP` pattern may
//! sit in either half, so flipping `enabled` is what stops the residual executor and dropping the
//! plan is what stops `renewal_cycle` from re-issuing — neither alone fails closed on its own
//! against an operator flag flip or against T6's residual path (KTD3).
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
use super::rule_health::{RuleHealth, UnhealthyCause};

/// Why a latency-disabled rule's health reason reads the way it does, beside
/// `task_renewal::TASK_EXPIRED_REASON`'s shape (R3).
const PATTERN_LATENCY_REASON_PREFIX: &str =
    "the pattern's measured execution time breached the configured latency budget";

impl DetectionEngine {
    /// Report a measured pattern execution against this engine's threshold; disable the owning
    /// rule if it breached it (R3).
    ///
    /// An observation strictly over the threshold runs three unconditional steps, regardless of
    /// the rule's current `enabled` flag: `enabled` is set `false`, the rule's compiled plan is
    /// removed, and the rule is marked [`super::rule_health::RuleHealth::Unhealthy`] with a
    /// reason naming the observed and threshold milliseconds, in that order (KTD3). A
    /// `tracing::warn!` records the same facts. If the health registry could not record the
    /// verdict, a `tracing::error!` names the rule id — a silently unrecorded disable is worse
    /// than no guard.
    ///
    /// An observation at or under the threshold changes nothing. Idempotency is keyed on health,
    /// not on `enabled`: a rule already [`super::rule_health::RuleHealth::Unhealthy`] for a cause
    /// that [`UnhealthyCause::resists_auto_recovery`] is left exactly as it is — a second breach
    /// does not overwrite the first breach's reason, and does not re-run the three steps. Keying
    /// on `enabled` instead was the bug: an operator's `set_rule_enabled(id, false)` on a
    /// still-healthy rule also sets `enabled` to `false` without touching health or the compiled
    /// plan, and used to take this same early return on a subsequent breach — leaving the plan
    /// live for the operator to re-enable straight back into it. A rule id this engine does not
    /// hold changes nothing and is logged at `warn`.
    ///
    /// Returns `true` exactly when the observation breached the threshold for a rule this engine
    /// holds.
    pub fn observe_pattern_latency(&mut self, rule_id: &str, observed: Duration) -> bool {
        if !self.rules.contains_key(rule_id) {
            tracing::warn!(
                rule_id,
                "pattern latency observed for a rule this engine does not hold"
            );
            return false;
        }
        if observed <= self.pattern_latency_threshold {
            return false;
        }

        let already_latched = matches!(
            self.health.health(rule_id),
            Some(&RuleHealth::Unhealthy { cause, .. }) if cause.resists_auto_recovery()
        );
        if already_latched {
            return true;
        }

        let observed_ms = u64::try_from(observed.as_millis()).unwrap_or(u64::MAX);
        let threshold_ms =
            u64::try_from(self.pattern_latency_threshold.as_millis()).unwrap_or(u64::MAX);
        let reason = format!(
            "{PATTERN_LATENCY_REASON_PREFIX}: {observed_ms} ms observed against a {threshold_ms} ms budget"
        );

        if let Some(rule) = self.rules.get_mut(rule_id) {
            rule.enabled = false;
        }
        let _uncovered = self.compiled.remove(rule_id);
        let marked = self
            .health
            .mark_unhealthy(rule_id, &reason, UnhealthyCause::LatencyBreach);
        if !marked {
            tracing::error!(
                rule_id,
                "pattern latency breach could not be recorded in rule health; the rule is \
                 disabled with no health record of why"
            );
        }
        tracing::warn!(
            rule_id,
            observed_ms,
            threshold_ms,
            "pattern latency breached the configured budget; rule disabled"
        );
        true
    }
}
