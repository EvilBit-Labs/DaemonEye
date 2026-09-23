//! Keeping a pushed task alive for as long as its rule is enabled (R16).
//!
//! A pushdown task carries a TTL so a collector orphaned by an agent crash stops collecting rather
//! than running unattended. The other side of that bargain is this module: while the agent is
//! alive and the rule is enabled, it renews the task before the TTL elapses.
//!
//! Three properties are load-bearing:
//!
//! - **Nothing here evaluates anything.** The ledger decides *when* a task is re-sent and *which*
//!   rule a lapsed task belongs to. Sending is the agent's transport concern and evaluating is the
//!   collector's.
//! - **Time arrives as a parameter.** Every decision takes `now`, matching
//!   `collector_core::pushdown`'s collector-side shape, so expiry is exercised without sleeping.
//!   (Plain code span, not an intra-doc link: `collector-core` depends on this crate, not the
//!   reverse, so the path is unresolvable from here.)
//! - **A task's identity is derived, never generated.** [`task_id`] is a pure function of the rule
//!   and the collector it is addressed to, so re-issuing the active set after a collector
//!   re-registers overwrites entries in place — in this ledger *and* in the collector's own
//!   accepted-task map, which is keyed on the same string. A random id per issue would make
//!   re-registration accumulate duplicate tasks on both sides.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::detection::DetectionEngine;
use crate::detection::planner::CompiledRule;
use crate::detection_bounds::PUSHDOWN_TASK_RENEWAL_INTERVAL;
use crate::proto::DetectionTask;

/// The stable identifier of the task carrying `rule_id`'s pushed half to `collector_id`.
///
/// One rule addressed to one collector has exactly one task, for the whole time the rule is
/// enabled. Renewal and re-issue are therefore the same write under the same key.
#[must_use]
pub fn task_id(rule_id: &str, collector_id: &str) -> String {
    format!("{rule_id}@{collector_id}")
}

/// A task the agent owes a collector: either its first issue or a renewal before expiry.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRenewal {
    task_id: String,
    collector_id: String,
    task: DetectionTask,
}

impl PendingRenewal {
    /// The task's stable identifier, which the agent reports back on a successful send.
    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// The collector this task is addressed to.
    #[must_use]
    pub fn collector_id(&self) -> &str {
        &self.collector_id
    }

    /// The wire task to send, carrying the plan and the TTL it was issued under.
    #[must_use]
    pub const fn task(&self) -> &DetectionTask {
        &self.task
    }

    /// Consume this renewal, yielding the wire task.
    #[must_use]
    pub fn into_task(self) -> DetectionTask {
        self.task
    }
}

/// What one pass of the clock concluded: what lapsed, and what to send.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenewalCycle {
    expired_rules: Vec<String>,
    due: Vec<PendingRenewal>,
}

impl RenewalCycle {
    /// Rules whose pushed half expired while the rule was still enabled (R16 into R12).
    #[must_use]
    pub fn expired_rules(&self) -> &[String] {
        &self.expired_rules
    }

    /// Tasks to send now. An entry is either a first issue or a renewal.
    #[must_use]
    pub fn due(&self) -> &[PendingRenewal] {
        &self.due
    }

    /// Consume the cycle, yielding the tasks to send.
    #[must_use]
    pub fn into_due(self) -> Vec<PendingRenewal> {
        self.due
    }

    /// Record a rule whose task lapsed.
    fn push_expired(&mut self, rule_id: String) {
        self.expired_rules.push(rule_id);
    }

    /// Record a task the agent must send.
    fn push_due(&mut self, pending: PendingRenewal) {
        self.due.push(pending);
    }
}

/// One live task: what it carries, who owns it, and when it was last confirmed.
#[derive(Debug, Clone)]
struct Entry {
    rule_id: String,
    collector_id: String,
    task: DetectionTask,
    ttl: Duration,
    /// When the task was last known to be live on the collector: its issue, or its last landed
    /// renewal. A renewal that never lands never moves this, which is what makes the task expire.
    confirmed_at: SystemTime,
    renewals: u64,
}

impl Entry {
    fn pending(&self) -> PendingRenewal {
        PendingRenewal {
            task_id: task_id(&self.rule_id, &self.collector_id),
            collector_id: self.collector_id.clone(),
            task: self.task.clone(),
        }
    }

    /// Whether the TTL elapsed without a renewal landing. A `confirmed_at + ttl` that cannot be
    /// represented is treated as expired: an unrepresentable deadline is not a live task.
    fn is_expired(&self, now: SystemTime) -> bool {
        self.confirmed_at
            .checked_add(self.ttl)
            .is_none_or(|deadline| deadline <= now)
    }

    fn is_due(&self, now: SystemTime) -> bool {
        self.confirmed_at
            .checked_add(PUSHDOWN_TASK_RENEWAL_INTERVAL)
            .is_none_or(|next| next <= now)
    }
}

/// Every task the agent is keeping alive, keyed by [`task_id`].
#[derive(Debug, Default)]
pub struct TaskRenewalLedger {
    entries: BTreeMap<String, Entry>,
}

impl TaskRenewalLedger {
    /// Create an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue `compiled`'s task, replacing any entry under the same identifier.
    ///
    /// This is both the first issue and the re-issue a collector's re-registration triggers. The
    /// renewal counter is deliberately reset: a re-issue is a new lifetime, not a renewal of the
    /// old one, and conflating the two would let a re-registration masquerade as a healthy
    /// renewal cadence.
    pub fn issue(&mut self, compiled: &CompiledRule, now: SystemTime) -> PendingRenewal {
        let plan = compiled.plan().clone();
        let ttl = Duration::from_millis(plan.ttl_ms);
        let id = task_id(compiled.rule_id(), compiled.collector_id());
        let entry = Entry {
            rule_id: compiled.rule_id().to_owned(),
            collector_id: compiled.collector_id().to_owned(),
            task: DetectionTask::new_pushdown(id.clone(), plan),
            ttl,
            confirmed_at: now,
            renewals: 0,
        };
        let pending = entry.pending();
        let _replaced = self.entries.insert(id, entry);
        pending
    }

    /// Tasks whose renewal interval has elapsed, oldest identifier first.
    #[must_use]
    pub fn due(&self, now: SystemTime) -> Vec<PendingRenewal> {
        self.entries
            .values()
            .filter(|entry| entry.is_due(now))
            .map(Entry::pending)
            .collect()
    }

    /// Record that a renewal landed on the collector, extending the task's lifetime.
    ///
    /// Returns whether the task was still tracked. A renewal that lands for a task the ledger
    /// already retired changes nothing: the rule was disabled, or the task already expired.
    pub fn record_renewal(&mut self, task_id: &str, now: SystemTime) -> bool {
        let Some(entry) = self.entries.get_mut(task_id) else {
            return false;
        };
        entry.confirmed_at = now;
        entry.renewals = entry.renewals.saturating_add(1);
        true
    }

    /// How many confirmed deliveries have extended this task's lifetime since it was issued.
    ///
    /// The confirmation of the issue itself counts: it is the same fact — the collector has this
    /// task — arriving for the first time. What the counter proves is that something is landing,
    /// which a task that merely still exists in the ledger does not.
    #[must_use]
    pub fn renewal_count(&self, task_id: &str) -> Option<u64> {
        self.entries.get(task_id).map(|entry| entry.renewals)
    }

    /// Remove every task whose TTL elapsed, reporting the rules they belonged to.
    ///
    /// Callers prune tasks of disabled rules *before* calling this, because the two outcomes
    /// differ: a disabled rule's task is dropped silently, while a rule that is still enabled when
    /// its task lapses has lost coverage and must be marked unhealthy.
    pub fn expire(&mut self, now: SystemTime) -> Vec<String> {
        let mut expired = Vec::new();
        self.entries.retain(|_id, entry| {
            if entry.is_expired(now) {
                expired.push(entry.rule_id.clone());
                return false;
            }
            true
        });
        expired
    }

    /// Stop renewing every task belonging to `rule_id`, silently.
    pub fn forget_rule(&mut self, rule_id: &str) {
        self.entries.retain(|_id, entry| entry.rule_id != rule_id);
    }

    /// Whether a task is currently being kept alive.
    #[must_use]
    pub fn is_tracked(&self, task_id: &str) -> bool {
        self.entries.contains_key(task_id)
    }

    /// Every rule that currently has a live task.
    #[must_use]
    pub fn tracked_rule_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .entries
            .values()
            .map(|entry| entry.rule_id.clone())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Number of tasks currently being kept alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no task is being kept alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Why an expired pushed half makes its rule unhealthy, as an operator reads it (R16 into R12).
const TASK_EXPIRED_REASON: &str =
    "the pushed half stopped running: no renewal landed before the task TTL elapsed";

/// The engine's half of renewal: which rules own a live task, and what an expiry costs them.
///
/// This lives beside the ledger rather than in `detection::mod` so the whole clock-driven half of
/// the detection engine reads as one file.
impl DetectionEngine {
    /// Advance the pushed-task clock to `now`: retire what lapsed, and report what to send.
    ///
    /// The order is deliberate. Tasks of rules that are gone or disabled are dropped first and
    /// silently, so only a rule that is *still enabled* can be reported as expired — the case
    /// where the agent would otherwise keep treating a rule as covered while its pushed half had
    /// stopped running.
    ///
    /// Nothing is evaluated here and nothing is sent; the caller owns the transport.
    pub fn renewal_cycle(&mut self, now: SystemTime) -> RenewalCycle {
        for rule_id in self.tasks.tracked_rule_ids() {
            if !self.is_rule_covered(&rule_id) {
                self.tasks.forget_rule(&rule_id);
            }
        }

        let mut cycle = RenewalCycle::default();
        for rule_id in self.tasks.expire(now) {
            let _marked = self.health.mark_unhealthy(&rule_id, TASK_EXPIRED_REASON);
            let _uncovered = self.compiled.remove(&rule_id);
            cycle.push_expired(rule_id);
        }

        for rule_id in self.coverable_rule_ids() {
            let Some(compiled) = self.compiled.get(&rule_id).cloned() else {
                continue;
            };
            let id = task_id(compiled.rule_id(), compiled.collector_id());
            if !self.tasks.is_tracked(&id) {
                cycle.push_due(self.tasks.issue(&compiled, now));
            }
        }
        for pending in self.tasks.due(now) {
            if !cycle
                .due()
                .iter()
                .any(|issued| issued.task_id() == pending.task_id())
            {
                cycle.push_due(pending);
            }
        }
        cycle
    }

    /// Re-issue the whole active task set addressed to `collector_id` (R16).
    ///
    /// Called after a collector registers again: the collector lost its accepted-task map, so
    /// every task it owns is re-sent. Identifiers are derived from the rule and the collector, so
    /// this overwrites entries in place on both sides rather than accumulating duplicates.
    pub fn issue_tasks_for_collector(
        &mut self,
        collector_id: &str,
        now: SystemTime,
    ) -> Vec<PendingRenewal> {
        let owned: Vec<CompiledRule> = self
            .coverable_rule_ids()
            .into_iter()
            .filter_map(|rule_id| self.compiled.get(&rule_id))
            .filter(|compiled| compiled.collector_id() == collector_id)
            .cloned()
            .collect();
        owned
            .iter()
            .map(|compiled| self.tasks.issue(compiled, now))
            .collect()
    }

    /// Record that a renewal landed on its collector, extending the task's lifetime.
    pub fn record_renewal(&mut self, task_id: &str, now: SystemTime) -> bool {
        self.tasks.record_renewal(task_id, now)
    }

    /// How many confirmed deliveries have extended a task's lifetime since it was issued.
    #[must_use]
    pub fn renewal_count(&self, task_id: &str) -> Option<u64> {
        self.tasks.renewal_count(task_id)
    }

    /// Number of pushed tasks the agent is currently keeping alive.
    #[must_use]
    pub fn active_task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Rules that are loaded, enabled and have a plan: the set a task is kept alive for.
    fn coverable_rule_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .compiled
            .keys()
            .filter(|rule_id| self.is_rule_covered(rule_id))
            .cloned()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Whether a rule is still loaded, enabled and planned.
    fn is_rule_covered(&self, rule_id: &str) -> bool {
        self.compiled.contains_key(rule_id)
            && self.rules.get(rule_id).is_some_and(|rule| rule.enabled)
    }
}
