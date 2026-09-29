//! Rule health, and the re-validation pass a descriptor change triggers (R12).
//!
//! When a collector registers for the first time, or re-registers with a changed descriptor, every
//! enabled rule that reaches the affected tables has to be looked at again. This module does the
//! half that can be done today: it re-checks each rule's table and column references against the
//! current catalog (R11) and marks the ones that no longer resolve unhealthy rather than dropping
//! them.
//!
//! # The seam U7 fills
//!
//! [`ReplanOutcome::to_replan`] is the list of rule identifiers whose *pushed half* must be
//! recomputed. U6 cannot recompute it, because the planner that lowers a rule into a pushdown plan
//! plus a residual is U7's work and does not exist yet. So this module stops at the boundary: it
//! establishes which rules still validate and therefore need re-planning, and which no longer do
//! and must not have a task re-issued. U7 consumes `to_replan()` and calls the planner for each
//! identifier in it; U11 re-issues the resulting tasks. Nothing here silently no-ops — a rule that
//! stops validating is moved to [`RuleHealth::Unhealthy`] and *excluded* from `to_replan()`, which
//! is the observable consequence AE6 asks for.
//!
//! Surfacing unhealthy rules to an operator is T10's CLI work. This module owns the state, not the
//! presentation.

use std::collections::{BTreeMap, BTreeSet};

use super::catalog::{CatalogChange, SchemaCatalog};

/// Which mechanism disabled a rule, so [`RuleHealthRegistry::revalidate`] knows whether catalog
/// re-validation has any standing to clear it (R8, KTD6).
///
/// Catalog re-validation is evidence about the schema; it says nothing about a pattern's observed
/// execution cost. So it may re-heal a cause the catalog can answer for — [`Self::Reference`] and
/// [`Self::TaskExpiry`] — but never [`Self::LatencyBreach`], which only
/// [`super::DetectionEngine::load_rule`] may clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnhealthyCause {
    /// A reference the rule makes no longer resolves against the catalog.
    Reference,
    /// The rule's pushed task expired without renewal (R16). Re-validation re-heals it, because
    /// re-planning is what issues the task again.
    TaskExpiry,
    /// A measured pattern execution exceeded the configured latency budget (R3, R8).
    LatencyBreach,
}

impl UnhealthyCause {
    /// Whether this cause resists automatic re-healing — by [`RuleHealthRegistry::revalidate`] or
    /// by the re-planning [`RuleHealthRegistry::track`] call a registration triggers — and may
    /// only be cleared by a fresh `DetectionEngine::load_rule`.
    ///
    /// Catalog evidence can answer for [`Self::Reference`] and for [`Self::TaskExpiry`]:
    /// re-planning is exactly the fix for both. It says nothing about a pattern's measured
    /// execution cost, so [`Self::LatencyBreach`] resists it.
    ///
    /// Matched exhaustively with no wildcard arm on purpose: a future variant must decide this
    /// explicitly rather than silently defaulting to recoverable (ADR-0009, KTD8).
    #[must_use]
    pub const fn resists_auto_recovery(self) -> bool {
        match self {
            Self::Reference | Self::TaskExpiry => false,
            Self::LatencyBreach => true,
        }
    }
}

/// Whether a rule can still be planned against the current catalog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RuleHealth {
    /// The rule has not been validated against a catalog yet.
    #[default]
    Unknown,
    /// Every reference the rule makes resolves.
    Healthy,
    /// The rule is kept, not dropped, but is not currently plannable.
    Unhealthy {
        /// Why the rule is unhealthy, as an operator reads it.
        reason: String,
        /// Which mechanism disabled it, so re-validation knows whether it may clear this.
        cause: UnhealthyCause,
    },
}

impl RuleHealth {
    /// Whether the rule is currently plannable.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        matches!(*self, Self::Healthy)
    }
}

/// What a re-validation pass concluded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplanOutcome {
    to_replan: Vec<String>,
    newly_unhealthy: Vec<String>,
}

impl ReplanOutcome {
    /// Rules that still validate and whose pushed half U7 must recompute.
    #[must_use]
    pub fn to_replan(&self) -> &[String] {
        &self.to_replan
    }

    /// Rules this pass moved from healthy to unhealthy. Their tasks are not re-issued.
    #[must_use]
    pub fn newly_unhealthy(&self) -> &[String] {
        &self.newly_unhealthy
    }
}

/// One tracked rule: the references it depends on, and its current health.
#[derive(Debug, Clone)]
struct TrackedRule {
    references: BTreeSet<(String, String)>,
    health: RuleHealth,
}

/// Health state for every enabled rule, and the pass that maintains it.
#[derive(Debug, Default)]
pub struct RuleHealthRegistry {
    rules: BTreeMap<String, TrackedRule>,
}

impl RuleHealthRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Track `rule_id` and the `(table, column)` references it reads, as [`RuleHealth::Healthy`].
    ///
    /// Tracking happens on exactly one path: immediately after the planner lowered the rule against
    /// the current catalog, which it could only do because every reference resolved. So the rule
    /// has already been judged and passed by the time it arrives here, and recording it as
    /// [`RuleHealth::Unknown`] would be false — it also silently undid a `Healthy` set moments
    /// earlier by [`Self::revalidate`], leaving no rule ever observably healthy after a
    /// registration-triggered re-plan.
    ///
    /// [`RuleHealth::Unknown`] remains the default for a rule nothing has judged yet, which is a
    /// real and different state; it is simply not reachable through this method.
    ///
    /// The one exception: if the rule is already tracked with a health whose
    /// [`UnhealthyCause::resists_auto_recovery`], that verdict is preserved rather than reset to
    /// `Healthy`. This method is also the re-planning call a registration drives — draining a
    /// deferred rule, or re-planning a rule `revalidate` let through — and must not be the thing
    /// that launders a latency breach back to healthy. References are still updated in that
    /// case, because re-planning the references is legitimate; only the health verdict is kept.
    pub fn track<I, T, C>(&mut self, rule_id: &str, references: I)
    where
        I: IntoIterator<Item = (T, C)>,
        T: Into<String>,
        C: Into<String>,
    {
        let resolved = references
            .into_iter()
            .map(|(table, column)| (table.into(), column.into()))
            .collect();

        let preserved_health = self.rules.get(rule_id).and_then(|rule| {
            let resists = matches!(
                rule.health,
                RuleHealth::Unhealthy { cause, .. } if cause.resists_auto_recovery()
            );
            resists.then(|| rule.health.clone())
        });

        let _previous = self.rules.insert(
            rule_id.to_owned(),
            TrackedRule {
                references: resolved,
                health: preserved_health.unwrap_or(RuleHealth::Healthy),
            },
        );
    }

    /// Stop tracking a rule, for example when the operator disables it.
    pub fn forget(&mut self, rule_id: &str) {
        let _removed = self.rules.remove(rule_id);
    }

    /// Mark a tracked rule unhealthy for a reason no catalog re-validation could find (R12).
    ///
    /// Callers today are pushed-task expiry, with [`UnhealthyCause::TaskExpiry`] so re-validation
    /// keeps recovering it exactly as before, and the latency guard, with
    /// [`UnhealthyCause::LatencyBreach`] so re-validation may not (R8, KTD6).
    ///
    /// A tracked rule has its health overwritten in place.
    ///
    /// An **untracked** rule is inserted only when `cause.resists_auto_recovery()` is `true`.
    /// Such a cause is never laundered back to healthy by [`Self::track`] or [`Self::revalidate`],
    /// so recording it with no references is safe — this is exactly the case a rule loaded before
    /// the first collector registers reaches, sitting untracked in `deferred` until it is planned.
    /// For a cause that does not resist auto-recovery, inserting an untracked rule would create
    /// one with no references, which the next [`Self::revalidate`] would launder back to
    /// [`RuleHealth::Healthy`] without checking anything, so that case is refused and `false` is
    /// returned instead.
    pub fn mark_unhealthy(&mut self, rule_id: &str, reason: &str, cause: UnhealthyCause) -> bool {
        if let Some(rule) = self.rules.get_mut(rule_id) {
            rule.health = RuleHealth::Unhealthy {
                reason: reason.to_owned(),
                cause,
            };
            return true;
        }
        if !cause.resists_auto_recovery() {
            return false;
        }
        let _previous = self.rules.insert(
            rule_id.to_owned(),
            TrackedRule {
                references: BTreeSet::new(),
                health: RuleHealth::Unhealthy {
                    reason: reason.to_owned(),
                    cause,
                },
            },
        );
        true
    }

    /// Current health of a rule, if it is tracked.
    #[must_use]
    pub fn health(&self, rule_id: &str) -> Option<&RuleHealth> {
        self.rules.get(rule_id).map(|rule| &rule.health)
    }

    /// Every rule currently marked unhealthy, with its reason. T10 renders this.
    #[must_use]
    pub fn unhealthy(&self) -> Vec<(&str, &str)> {
        self.rules
            .iter()
            .filter_map(|(rule_id, rule)| {
                if let RuleHealth::Unhealthy { ref reason, .. } = rule.health {
                    return Some((rule_id.as_str(), reason.as_str()));
                }
                None
            })
            .collect()
    }

    /// Re-validate every rule the registration affected, and report what to re-plan (R12).
    ///
    /// A rule is revisited when the registration was a first registration — which widens the
    /// catalog, so a rule that was waiting on an empty one must now be judged — or when it reads a
    /// table the registration altered.
    ///
    /// A rule whose current health [`UnhealthyCause::resists_auto_recovery`] is skipped
    /// unconditionally, before either of those checks: re-validation is evidence about the
    /// catalog, not about a pattern's cost, so it has no standing to re-heal it or to queue it for
    /// re-planning (R8, KTD6). Any other cause — [`UnhealthyCause::Reference`] or
    /// [`UnhealthyCause::TaskExpiry`] — keeps the blanket re-heal below unchanged.
    pub fn revalidate(&mut self, catalog: &SchemaCatalog, change: &CatalogChange) -> ReplanOutcome {
        let mut outcome = ReplanOutcome::default();
        if change.is_empty() {
            return outcome;
        }

        for (rule_id, rule) in &mut self.rules {
            let resists = matches!(
                rule.health,
                RuleHealth::Unhealthy { cause, .. } if cause.resists_auto_recovery()
            );
            if resists {
                continue;
            }
            if !change.is_first_registration() && !touches(rule, change) {
                continue;
            }
            let was_unhealthy = matches!(rule.health, RuleHealth::Unhealthy { .. });
            if let Some(reason) = first_unresolved(catalog, rule) {
                rule.health = RuleHealth::Unhealthy {
                    reason,
                    cause: UnhealthyCause::Reference,
                };
                if !was_unhealthy {
                    outcome.newly_unhealthy.push(rule_id.clone());
                }
            } else {
                rule.health = RuleHealth::Healthy;
                outcome.to_replan.push(rule_id.clone());
            }
        }
        outcome
    }
}

/// Whether a rule reads any table the registration altered.
fn touches(rule: &TrackedRule, change: &CatalogChange) -> bool {
    rule.references
        .iter()
        .any(|entry| change.affected_tables().contains(&entry.0))
}

/// The first reference this rule makes that the catalog cannot resolve, rendered.
fn first_unresolved(catalog: &SchemaCatalog, rule: &TrackedRule) -> Option<String> {
    rule.references
        .iter()
        .find_map(|entry| catalog.resolve_reference(&entry.0, &entry.1).err())
        .map(|unresolved| unresolved.to_string())
}
