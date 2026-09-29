---
title: A Guard Is Only As Wide As The Readers It Audits (T5 latency-breach disable)
category: security-issues
date: 2026-09-28
tags:
  - rust
  - fail-open
  - detection-engine
  - rule-lifecycle
  - guard-scope
  - exhaustive-match
  - review-findings
module: daemoneye-lib (detection::pattern_latency, detection::rule_health, detection::mod)
symptom: |
  A rule the latency guard had just disabled could be brought back three different ways, none of
  them the documented one. An operator-disabled rule skipped the disable entirely; a rule loaded
  before any collector registered had its verdict laundered by the later re-plan; and re-enabling
  a breached rule resumed alerting through the agent's own collection loop. The module doc claimed
  "only `load_rule` restores a rule this guard disabled" and the test suite was green.
root_cause: |-
  The guard wrote its verdict into three places — `rule.enabled`, the compiled-plan map, and the
  rule's health — and was audited against only the reader it was written for, the pushdown renewal
  loop, which reads the compiled plan. Every other reader still treated `rule.enabled` as the
  authority. `enabled` cannot carry a verdict, because it already means "an operator turned this
  off", so a guard that sets it inherits every path that sets or reads it for the other reason.
---

# A Guard Is Only As Wide As The Readers It Audits

## Problem

The detection engine gained a fail-closed guard: when a measured pattern execution exceeds the configured latency budget, `DetectionEngine::observe_pattern_latency` (`daemoneye-lib/src/detection/pattern_latency.rs:59`) disables the rule until an operator reloads it. The disable flipped `rule.enabled`, dropped the rule's compiled plan, and marked it unhealthy.

Three reviewers each found a different way past it, and all three shipped through implementation and a green suite:

1. **The guard's own idempotency check.** It used `rule.enabled` to decide "already disabled, do nothing". But `enabled == false` is also what `set_rule_enabled` leaves behind for an operator disable, which touches neither the plan nor health. A breach on such a rule took the early return and performed neither safety step.
2. **A rule that was never tracked.** A rule loaded before any collector registered sat in the rule map but not in the health registry, so `mark_unhealthy` (`daemoneye-lib/src/detection/rule_health.rs:200`) returned `false` into a discarded binding. The registration that later drained it re-planned it as healthy, with no trace of the breach.
3. **The reader nobody checked.** `execute_rules` (`daemoneye-lib/src/detection/mod.rs:296`) — the path the agent's collection loop calls every cycle (`daemoneye-agent/src/main.rs:454`) — gated on `rule.enabled` and nothing else. Two existing public calls composed today, with no new code, resumed alerting for a breached rule.

## Root Cause

The verdict was written to three fields and audited against one reader.

`enabled` was the wrong place to carry it. That flag already had an owner and a meaning — the operator turned this rule off — so writing a second meaning into it meant inheriting every path that reads or writes it for the first. The guard then had to be correct not only where it was added but in `set_rule_enabled`, in `execute_rules`, in `plan_and_record`, and in the deferred drain, none of which knew a guard now existed.

The health registry was the right place, and was already half-used: `revalidate` (`rule_health.rs:255`) had been taught to skip a latency verdict. Its twin `track` (`rule_health.rs:151`) had not, and `track` is on the re-planning path, so the same laundering the earlier fix closed on one route stayed open on the other.

## Fix

State the policy once, where the compiler can enforce it. `UnhealthyCause::resists_auto_recovery` (`rule_health.rs:58`) is an exhaustive match with no wildcard arm: a cause added later cannot silently default to recoverable, it fails to compile until it declares itself. `revalidate`, `track`, `plan_and_record` (`mod.rs:192`), `execute_rules`, and `set_rule_enabled` (`mod.rs:415`) all consult it. `load_rule` forgets the rule's health before re-planning, which is what makes a reload the single clearing point rather than a claim in a doc comment.

`set_rule_enabled(id, true)` now refuses rather than silently succeeding, which is a deliberate public-API behaviour change: a caller that cannot tell whether its request took effect is how the third bypass stayed invisible.

## Why this generalises

**Audit by reader, not by writer.** Before adding a guard, list every path that *writes* the state it sets and every path that *reads* that state to decide whether work runs. Here a grep for `.enabled` across the crate would have produced all three bypasses in one pass, in under a minute. The guard was reviewed carefully for whether its own three steps were correct, which is the question that was never in doubt.

**A flag that already has a meaning cannot carry a verdict.** If the guard's state is distinguishable from the state it collides with — disabled-by-operator versus disabled-by-guard — it needs its own field, and that field needs a policy every consumer consults. Otherwise the two meanings are merged at exactly the moment they need to differ.

**Distrust a test that asserts what did not change.** A pre-existing test called `set_rule_enabled(true)` after a breach and asserted only that the compiled plan and the health reason were unchanged. It never asserted that `enabled` stayed false — the one thing the call actually altered. It passed while walking straight through the bypass, documenting it instead of catching it. When a test exercises a mutating call and asserts only on the fields that call does not touch, it is describing the code rather than constraining it.

**Apply your own rules to your own code.** This repository already requires that a new `sqlparser` enum variant be a build break rather than a silent default (ADR-0009, and KTD8 of the T5 plan). The same branch that enforced that for the parser introduced its own cause enum behind a `matches!` that would have defaulted a future variant to recoverable. A reviewer caught it as a residual risk before it became a fourth bypass; the rule was already written down, just not applied inward.

## Related

- [`a-guard-that-fails-open-is-worse-than-no-guard.md`](a-guard-that-fails-open-is-worse-than-no-guard.md) — the other half of this pair, from the same ticket. That one is a guard whose *failure path* disabled the control it protected; this one is a guard whose *scope* missed sibling readers. Together: check what happens when the guard cannot run, and check who else decides the thing the guard is deciding.
- [`../best-practices/assert-which-gate-fired-when-error-variants-collide-2026-09-17.md`](../best-practices/assert-which-gate-fired-when-error-variants-collide-2026-09-17.md) — the same family of defect in test assertions: passing for a reason other than the one named.
