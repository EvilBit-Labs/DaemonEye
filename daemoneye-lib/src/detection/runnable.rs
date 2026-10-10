//! The single eligibility site: which rules may run, and under which generation (KTD6, R8).
//!
//! A verdict lives in three places (`enabled`, the compiled-plan map, rule health).
//! `DetectionEngine::is_eligible` is the only statement of how they combine, and everything that
//! gates work on it calls it:
//! [`DetectionEngine::runnable_rules`], [`DetectionEngine::is_runnable`], and
//! `is_rule_covered` in `task_renewal`. A new reader of eligibility must call it too, not re-derive
//! it; the table in [`super::execution`] lists every reader reconciled against it.

use std::time::Duration;

use super::generation::Generation;
use super::rule_health::RuleHealth;
use super::{CompiledRule, DetectionEngine};
use crate::models::DetectionRule;
use crate::proto::TableDescriptor;

/// One rule the executor may run this cycle, with everything it needs and nothing it must go back
/// to the engine for (KTD7: the executor holds no engine lock while it runs).
#[derive(Debug, Clone)]
pub struct RunnableRule {
    /// The rule as loaded.
    pub rule: DetectionRule,
    /// The plan lowered from it.
    pub compiled: CompiledRule,
    /// The catalog table the plan reads, as the collector declared it.
    pub descriptor: TableDescriptor,
    /// Which load of the rule this snapshot was taken from. Every latency report and result for
    /// this evaluation must carry it back; see [`DetectionEngine::is_runnable`].
    pub generation: Generation,
    /// The engine's per-pattern latency budget, carried here so the executor can stop a breaching
    /// rule between batches without touching the engine (ADR-0011).
    pub pattern_latency_threshold: Duration,
}

impl DetectionEngine {
    /// Whether `rule_id` is loaded, enabled, holds a plan, and has health that does not resist
    /// auto-recovery.
    ///
    /// The four conditions are independent writes by independent paths, which is why they are
    /// read together here and nowhere else. `compiled` is the load-bearing one: a latency breach
    /// removes the plan and `plan_and_record` refuses to restore it, so a rule that resists
    /// auto-recovery should never hold one. The health check is the second line that makes
    /// that a property of this predicate rather than of statement order in its writers.
    pub(super) fn is_eligible(&self, rule_id: &str) -> bool {
        let Some(rule) = self.rules.get(rule_id) else {
            return false;
        };
        if !rule.enabled || !self.compiled.contains_key(rule_id) {
            return false;
        }
        !self
            .health
            .health(rule_id)
            .is_some_and(RuleHealth::resists_auto_recovery)
    }

    /// Every rule that may run now, snapshotted so the caller can release the engine before
    /// executing (KTD7). The single eligibility site and the single issuer of [`Generation`].
    ///
    /// A rule is listed only if [`Self::is_runnable`]'s predicate holds for its current
    /// generation. Order is by rule id, so a cycle's evaluation order does not depend on hash
    /// iteration.
    #[must_use]
    pub fn runnable_rules(&self) -> Vec<RunnableRule> {
        let mut ids: Vec<&String> = self.compiled.keys().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter(|rule_id| self.is_eligible(rule_id))
            .filter_map(|rule_id| self.snapshot_runnable(rule_id))
            .collect()
    }

    /// Whether `rule_id` is still runnable *and* is still the load `generation` was issued for.
    ///
    /// The agent's result gate calls this after an evaluation returns, under a second short lock,
    /// so a result computed for a rule that was reloaded, disabled or latched mid-cycle is dropped
    /// rather than alerted on.
    #[must_use]
    pub fn is_runnable(&self, rule_id: &str, generation: Generation) -> bool {
        self.generations.current(rule_id) == Some(generation) && self.is_eligible(rule_id)
    }

    /// Build one snapshot for an id that already passed [`Self::is_eligible`].
    ///
    /// `None` only if the id has no generation, no catalog table, or no plan: states the engine's
    /// own writers do not produce for an eligible rule. Such a rule is skipped loudly rather than
    /// run without the identity its results would need, which makes this the one place
    /// `runnable_rules` can list fewer rules than `is_eligible` accepts.
    fn snapshot_runnable(&self, rule_id: &str) -> Option<RunnableRule> {
        let rule = self.rules.get(rule_id)?;
        let compiled = self.compiled.get(rule_id)?;
        let issued = self.generations.current(rule_id);
        let table = self.catalog.table(&compiled.plan().table);
        let (Some(generation), Some(descriptor)) = (issued, table) else {
            tracing::error!(
                rule_id,
                has_generation = issued.is_some(),
                has_descriptor = table.is_some(),
                "an eligible rule lacks the identity or table needed to run it; skipped"
            );
            return None;
        };
        Some(RunnableRule {
            rule: rule.clone(),
            compiled: compiled.clone(),
            descriptor: descriptor.clone(),
            generation,
            pattern_latency_threshold: self.pattern_latency_threshold,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::detection::catalog::verify_spawn_token;
    use crate::detection::rule_health::UnhealthyCause;
    use crate::models::AlertSeverity;
    use crate::proto::{ColumnDescriptor, ColumnType, PredicateOp, SchemaDescriptor};

    const RULE_ID: &str = "a";

    /// What a rule's health row holds, as a table axis. `Untracked` is a rule nothing has judged.
    #[derive(Debug, Clone, Copy)]
    enum HealthCell {
        Untracked,
        Healthy,
        TaskExpiry,
        LatencyBreach,
    }

    const HEALTH_CELLS: [HealthCell; 4] = [
        HealthCell::Untracked,
        HealthCell::Healthy,
        HealthCell::TaskExpiry,
        HealthCell::LatencyBreach,
    ];

    fn engine_with_one_planned_rule() -> DetectionEngine {
        let token = "a".repeat(64);
        let verified = verify_spawn_token("procmond", Some(&token), Some(&token)).unwrap();
        let mut engine = DetectionEngine::new();
        engine
            .register_collector(
                &verified,
                SchemaDescriptor {
                    collector_id: "procmond".to_owned(),
                    descriptor_version: "v1".to_owned(),
                    tables: vec![crate::proto::TableDescriptor {
                        name: "processes".to_owned(),
                        columns: vec![ColumnDescriptor {
                            name: "cpu_usage".to_owned(),
                            column_type: i32::from(ColumnType::Int),
                            nullable: false,
                            supported_ops: vec![i32::from(PredicateOp::Gt)],
                        }],
                    }],
                    conformance_results: Vec::new(),
                },
            )
            .unwrap();
        engine
            .load_rule(DetectionRule::new(
                RULE_ID.to_owned(),
                "Rule".to_owned(),
                "Eligibility table".to_owned(),
                "SELECT cpu_usage FROM processes WHERE cpu_usage > 80".to_owned(),
                "test".to_owned(),
                AlertSeverity::Low,
            ))
            .unwrap();
        engine
    }

    /// Drive the three independent stores into one cell, writing them directly: the point is to
    /// reach combinations the public API deliberately cannot, such as an enabled, planned rule
    /// whose health is latched, because that is the combination a regression would exploit.
    fn engine_in_cell(is_enabled: bool, has_plan: bool, health: HealthCell) -> DetectionEngine {
        let mut engine = engine_with_one_planned_rule();
        engine.rules.get_mut(RULE_ID).unwrap().enabled = is_enabled;
        if !has_plan {
            let _plan = engine.compiled.remove(RULE_ID);
        }
        engine.health.forget(RULE_ID);
        match health {
            HealthCell::Untracked => {}
            HealthCell::Healthy => engine.health.track(RULE_ID, [("processes", "cpu_usage")]),
            HealthCell::TaskExpiry => {
                engine.health.track(RULE_ID, [("processes", "cpu_usage")]);
                assert!(engine.health.mark_unhealthy(
                    RULE_ID,
                    "expired",
                    UnhealthyCause::TaskExpiry
                ));
            }
            HealthCell::LatencyBreach => assert!(engine.health.mark_unhealthy(
                RULE_ID,
                "breached",
                UnhealthyCause::LatencyBreach
            )),
        }
        engine
    }

    /// The agreement the plan asks for: for every `enabled x plan x health` cell the three readers
    /// of eligibility give one answer, and it is the one the policy states. A failure names the
    /// cell, so a disagreement says which combination diverged and in which reader.
    #[test]
    fn every_eligibility_reader_agrees_on_every_cell() {
        let mut failures = Vec::new();
        for is_enabled in [true, false] {
            for has_plan in [true, false] {
                for health in HEALTH_CELLS {
                    let engine = engine_in_cell(is_enabled, has_plan, health);
                    let latched = matches!(health, HealthCell::LatencyBreach);
                    let expected = is_enabled && has_plan && !latched;

                    let covered = engine.is_rule_covered(RULE_ID);
                    let listed = engine.runnable_rules();
                    let is_listed = listed.iter().any(|r| r.rule.id.raw() == RULE_ID);
                    let generation = engine.generations.current(RULE_ID).unwrap();
                    let is_runnable = engine.is_runnable(RULE_ID, generation);

                    for (reader, got) in [
                        ("is_rule_covered", covered),
                        ("runnable_rules", is_listed),
                        ("is_runnable", is_runnable),
                    ] {
                        if got != expected {
                            failures.push(format!(
                                "cell enabled={is_enabled} plan={has_plan} health={health:?}: \
                                 {reader} said {got}, policy says {expected}"
                            ));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn a_rule_that_is_not_loaded_is_not_eligible() {
        let engine = DetectionEngine::new();
        assert!(!engine.is_eligible("missing"));
        assert!(engine.runnable_rules().is_empty());
    }
}
