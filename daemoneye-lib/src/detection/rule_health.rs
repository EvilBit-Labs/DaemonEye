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

/// Whether a rule can still be planned against the current catalog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RuleHealth {
    /// The rule has not been validated against a catalog yet.
    #[default]
    Unknown,
    /// Every reference the rule makes resolves.
    Healthy,
    /// A reference the rule makes no longer resolves; the rule is kept, not dropped.
    Unhealthy {
        /// The specific reference that stopped resolving.
        reason: String,
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

    /// Track `rule_id` and the `(table, column)` references it reads.
    ///
    /// A rule starts [`RuleHealth::Unknown`]: it has not been judged against a catalog yet, which
    /// is a different state from having been judged and passed.
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
        let _previous = self.rules.insert(
            rule_id.to_owned(),
            TrackedRule {
                references: resolved,
                health: RuleHealth::Unknown,
            },
        );
    }

    /// Stop tracking a rule, for example when the operator disables it.
    pub fn forget(&mut self, rule_id: &str) {
        let _removed = self.rules.remove(rule_id);
    }

    /// Mark a tracked rule unhealthy for a reason no catalog re-validation could find (R12).
    ///
    /// The one caller today is pushed-task expiry: a rule whose task lapsed while it was still
    /// enabled has lost its pushed half, which is a health fact about the rule and belongs with
    /// every other one rather than in a second notion of health beside it.
    ///
    /// An untracked rule is **not** inserted, and `false` is returned. Inserting would create a
    /// rule with no references, which the next [`Self::revalidate`] would launder back to
    /// [`RuleHealth::Healthy`] without checking anything. Every rule that owns a task went through
    /// [`Self::track`] when it was planned, so the untracked case does not arise on the live path.
    pub fn mark_unhealthy(&mut self, rule_id: &str, reason: &str) -> bool {
        let Some(rule) = self.rules.get_mut(rule_id) else {
            return false;
        };
        rule.health = RuleHealth::Unhealthy {
            reason: reason.to_owned(),
        };
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
                if let RuleHealth::Unhealthy { ref reason } = rule.health {
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
    pub fn revalidate(&mut self, catalog: &SchemaCatalog, change: &CatalogChange) -> ReplanOutcome {
        let mut outcome = ReplanOutcome::default();
        if change.is_empty() {
            return outcome;
        }

        for (rule_id, rule) in &mut self.rules {
            if !change.is_first_registration() && !touches(rule, change) {
                continue;
            }
            let was_unhealthy = matches!(rule.health, RuleHealth::Unhealthy { .. });
            if let Some(reason) = first_unresolved(catalog, rule) {
                rule.health = RuleHealth::Unhealthy { reason };
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
