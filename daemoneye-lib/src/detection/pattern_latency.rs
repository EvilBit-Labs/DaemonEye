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
//! The health this leaves behind carries [`UnhealthyCause::LatencyBreach`], which
//! `RuleHealthRegistry::revalidate` will not clear (R8, KTD6) — a guard whose failure path could
//! be undone by an unrelated collector registration would not be a guard at all. Only
//! [`DetectionEngine::load_rule`] restores a rule this guard disabled.

use std::time::Duration;

use super::DetectionEngine;
use super::rule_health::UnhealthyCause;

/// Why a latency-disabled rule's health reason reads the way it does, beside
/// `task_renewal::TASK_EXPIRED_REASON`'s shape (R3).
const PATTERN_LATENCY_REASON_PREFIX: &str =
    "the pattern's measured execution time breached the configured latency budget";

impl DetectionEngine {
    /// Report a measured pattern execution against this engine's threshold; disable the owning
    /// rule if it breached it (R3).
    ///
    /// An observation strictly over the threshold runs three unconditional steps: `enabled` is
    /// set `false`, the rule's compiled plan is removed, and the rule is marked
    /// [`super::rule_health::RuleHealth::Unhealthy`] with a reason naming the observed and
    /// threshold milliseconds, in that order (KTD3). A `tracing::warn!` records the same facts.
    ///
    /// An observation at or under the threshold changes nothing. A rule already disabled is left
    /// exactly as it is — a second breach does not overwrite the first breach's reason. A rule id
    /// this engine does not hold changes nothing and is logged at `warn`.
    ///
    /// Returns `true` exactly when the observation breached the threshold for a rule this engine
    /// holds.
    pub fn observe_pattern_latency(&mut self, rule_id: &str, observed: Duration) -> bool {
        let was_enabled = if let Some(rule) = self.rules.get(rule_id) {
            rule.enabled
        } else {
            tracing::warn!(
                rule_id,
                "pattern latency observed for a rule this engine does not hold"
            );
            return false;
        };
        if observed <= self.pattern_latency_threshold {
            return false;
        }
        if !was_enabled {
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
        let _marked = self
            .health
            .mark_unhealthy(rule_id, &reason, UnhealthyCause::LatencyBreach);
        tracing::warn!(
            rule_id,
            observed_ms,
            threshold_ms,
            "pattern latency breached the configured budget; rule disabled"
        );
        true
    }
}
