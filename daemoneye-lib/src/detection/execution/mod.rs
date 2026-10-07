//! Execution of detection rules over the event store with `DataFusion` (ADR-0006).
//!
//! This module root currently carries the locked-down session ([`session`]) and the allowlisted
//! SQL functions ([`functions`], [`regexp`]), and plan derivation ([`mod@derive`]). The executor
//! itself arrives in a later unit.
//!
//! # Eligibility audit
//!
//! Whether a rule runs is decided from three stores that different code writes independently:
//! `rule.enabled`, the `compiled` plan map, and the health registry. A guard once added to one of
//! them was bypassed three ways because it was audited against the one reader it was written for
//! (`docs/solutions/security-issues/a-guard-must-be-audited-against-every-reader-of-the-state-it-writes.md`).
//! So this audit goes by **reader**: every path that decides whether work runs is listed and
//! reconciled with the one predicate, `DetectionEngine::is_eligible`
//! (`enabled && has a plan && health does not resist auto-recovery`), which
//! [`DetectionEngine::runnable_rules`](super::DetectionEngine::runnable_rules),
//! [`DetectionEngine::is_runnable`](super::DetectionEngine::is_runnable) and `is_rule_covered`
//! all call. Symbols are cited, not line numbers, which go stale within a branch.
//!
//! ## Readers: paths that decide whether work runs
//!
//! | Reader | What it reads | Reconciliation |
//! | --- | --- | --- |
//! | `runnable_rules`, `is_runnable` | the predicate | The new single site. |
//! | `is_rule_covered` (via `coverable_rule_ids`, `renewal_cycle`, `issue_tasks_for_collector`) | the predicate | Delegates; was `compiled && enabled`, which ignored health. No behaviour change today because a latch always removes the plan, but the health check is now a second line rather than statement order. |
//! | `execute_rules` | `enabled`, health; **not** `compiled` | **Diverges, by design, until U13 deletes it.** It still runs a rule with no plan: one deferred under R18, one whose pushed task expired, one whose references stopped resolving. `runnable_rules` runs none of those. The agent's collection loop still calls it. |
//! | `plan_and_record` | health (to refuse a plan for a latched rule); not `enabled` | Consistent. Plans are kept for operator-disabled rules so re-enabling needs no re-plan; the predicate gates them on `enabled`. |
//! | `set_rule_enabled` | health (refuses to enable a latched rule) | Consistent: a refused enable leaves `enabled == false`. |
//! | `track` | health (preserves a resisting verdict) | Consistent: keeps health in step with the predicate. |
//! | `revalidate` | health (skips a resisting verdict) | Consistent. |
//! | `mark_unhealthy` | health (never downgrades a resisting verdict) | Consistent. |
//! | the agent's result gate (U13) | `is_runnable(rule_id, generation)` | New; the predicate plus a generation match. |
//!
//! ## Writers
//!
//! | State | Writer | Effect on the predicate |
//! | --- | --- | --- |
//! | `rule.enabled` | `set_rule_enabled` | The operator's switch. |
//! | | `observe_pattern_latency` | Sets `false` on a current-generation breach. |
//! | | `load_rule` | Inserts the rule carrying its own flag, so a reload resets an operator disable. |
//! | | `DetectionRule::enable` / `disable` | **No production caller.** Only doc examples and a unit test in `models/rule.rs` call them, and the engine hands out only `&DetectionRule`, so a loaded rule cannot be flipped from outside. `load_rule` is the sole way a flag reaches the engine from elsewhere, and nothing outside `daemoneye-lib` calls it either. |
//! | `compiled` | `plan_and_record` | Inserts, unless health resists. |
//! | | `register_collector` (two removals) | Removes a rule that stopped validating, and one the planner can no longer lower. |
//! | | `reject_rule`, `remove_rule` | Remove. |
//! | | `observe_pattern_latency` | Removes on a breach. |
//! | | `renewal_cycle` | Removes on pushed-task expiry. |
//! | health | `track` | Healthy, or preserves a resisting verdict. |
//! | | `mark_unhealthy` | From `renewal_cycle` (`TaskExpiry`) and `observe_pattern_latency` (`LatencyBreach`). |
//! | | `revalidate` | Re-heals a non-resisting verdict only. |
//! | | `forget` | From `load_rule`, `reject_rule`, `remove_rule`. |
//! | generation | `load_rule` issues; `reject_rule`, `remove_rule` forget | A report is applied only if its generation is the current one. |
//!
//! ## What the audit found beyond the plan's list
//!
//! * `execute_rules` is the one reader that does not agree with the predicate (above), and a
//!   test pins the divergence for a task-expired rule. U13 replaces its call site and deletes it.
//! * `load_rule` is not called outside `daemoneye-lib`, so no production path loads rules into the
//!   agent's engine yet.
//! * A generation cannot be a per-rule counter that restarts on removal: a report for a removed
//!   rule would match a later rule of the same id. Generations come from one engine-wide counter.
//! * `runnable_rules` can list fewer rules than the predicate accepts in exactly one case, an
//!   eligible rule with no catalog table or generation, which no writer produces; it is skipped
//!   with an `error` log and not run.

pub mod derive;
pub mod functions;
pub mod regexp;
pub mod session;
