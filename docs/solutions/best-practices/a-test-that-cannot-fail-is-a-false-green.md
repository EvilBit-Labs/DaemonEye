---
title: A test that cannot fail is a false green, not coverage
date: 2026-10-10
category: docs/solutions/best-practices
module: daemoneye-lib
problem_type: best_practice
component: testing_framework
severity: high
applies_when:
  - writing a test whose assertion is `is_empty()`, `is_ok()`, `is_err()`, or "the value did not change"
  - a test's name claims a property the measured quantity cannot observe
  - gating a host-dependent number (RSS, latency, throughput) with a pass/fail assertion
  - a doc, ADR, or review comment states a behaviour that no test on either side pins
  - reviewing a test that was written after the code it covers, from the code's current output
tags:
  - test-assertions
  - false-positive-tests
  - vacuous-tests
  - positive-control
  - mutation-check
  - measurement-fidelity
---

# A test that cannot fail is a false green, not coverage

## Context

A test is evidence only if some concrete wrong behaviour would make it fail. Branch T6 (the DataFusion detection executor, PR pending) kept finding tests with no such behaviour, in the author's own pass and again in review; the four below are the ones that left a trace in the tree, and the same shape turned up in several earlier units of the branch. None was wrong about the code. Each was written from the implementation's current output, watched go green, and taken as proof. The shapes recurred:

- **The metric could not see the claim.** `an_open_read_snapshot_does_not_lift_the_cap` asserted redb's `used_bytes() <= cap` with a read snapshot held open. `used_bytes` is redb's own cache accounting, so anything a snapshot pinned *outside* the cache could never appear in it; the assertion could not fail for the claim in its name. It is now `the_cap_holds_with_a_reader_open` (`daemoneye-lib/src/storage/page_cache_tests.rs`), which is what it shows, and the pinning argument moved to the decision record on other grounds.
- **The gate could only fail for the case that does not matter.** The memory harness nearly shipped "back-half RSS growth ≤ front-half growth" as a leak gate. The front half contains the warm-up ramp (94→150 MiB in the first 100 cycles), so any leak smaller than the ramp passes and only an *accelerating* leak is caught. Every single-process gate tried either fitted the host or hid sub-ramp leaks. The plateau is now reported, not asserted (`daemoneye-lib/tests/detection_execution_memory.rs`); the assertions that remain are host-independent: planted-match counts, coverage spread, and that the sampler did not fail.
- **No positive control.** The NULL-literal derive test asserted an empty result over one row named `bash`. Mapping `NULL` to `''`, or to any non-matching literal, also returned nothing. It now carries a `bash` control that must return its row and an empty-string row that must not match (`daemoneye-lib/tests/detection_derive.rs`).
- **The test pinned the bug as the contract.** `parser_level_constructs_do_not_reach_the_function_allowlist` asserted that `SUBSTR` *loads*, as if that were wanted. Loading was the defect: the executor had no implementation, so the rule failed on every cycle. The right test asserts the refusal, and the gate now exists (`daemoneye-lib/tests/detection_sql_validation.rs`).
- **The documented behaviour had no test on either side.** "`SUBSTR` loads then fails at execution" was written into three docs from reasoning about the function registry. Adding the test confirmed it, and confirming it made the fix obvious. A claim in a doc is a test not yet written.
- **Asserting what did not change while the change is the bug.** One more in the predecessor PR, recorded in [a-guard-must-be-audited-against-every-reader-of-the-state-it-writes](../security-issues/a-guard-must-be-audited-against-every-reader-of-the-state-it-writes.md): a test called `set_rule_enabled(true)` after a breach and asserted the plan and health were unchanged, never that `enabled` stayed false, and so passed while walking through the bypass.

## Guidance

Before an assertion ships, write the sentence **"this fails when \_\_\_"** and make the blank name a specific wrong behaviour. "When the function returns the wrong thing" is not specific. "When it returns empty" is not a failure if empty is also what a no-op returns.

1. **Pair every negative with a positive control over the same fixture.** A "matches nothing" assertion needs a sibling "matches this row" over the same rows, or the fixture may simply be unreachable. If the negative and the positive share a helper, the control costs two lines.
2. **Check that the measured quantity can observe the claim.** Before asserting on a counter, a cache statistic or a metric, ask who increments it. A number that is defined to exclude the thing the test name mentions proves nothing about it.
3. **Measurement is not assertion.** A number that depends on the host (resident memory, wall-clock latency) is reported; the test asserts only invariants that do not (counts, coverage, "the sampler returned an error, not zero"). A gate on a host number either fits the host it was tuned on or hides the case it was built for. See [measurement-fixtures-must-exercise-correctness-where-they-measure-cost](measurement-fixtures-must-exercise-correctness-where-they-measure-cost.md) for the fixture half of that rule.
4. **Treat a behaviour claim in a doc, ADR or review comment as a test you have not written.** More than once on T6 a documented behaviour was wrong or unverified until a test forced the question, and the test's passing then changed what the fix should be: the `SUBSTR` claim above went from "document the asymmetry" to "refuse at load" the moment the test confirmed it.
5. **Mutation-check as the cheapest proof.** Break the code the test guards — invert the predicate, return empty, skip the branch — and watch the test fail. Seconds, no framework. If it still passes, it was never a test. A reviewer who cannot run mutation testing says so; the author can.

A green run is a *necessary* condition for a test to be right. It is not evidence, because a vacuous test is green by construction.

## Why This Matters

A hollow test is worse than no test. No test is an honest gap that a coverage report shows. A green vacuous test claims the property is held, survives every refactor that breaks it, and tells the next reviewer the area is covered. The ones on this branch guarded the security gate (what SQL loads), the memory bound, the NULL semantics of the executor, and the fail-closed latch — each a place where a silent wrong answer is the failure mode the daemon exists to avoid. Every one was caught by a second reader asking the one question this document is about; none was caught by running the suite.

The siblings cover the neighbouring shapes: [assert-mechanism-not-outcome-in-resilience-tests](assert-mechanism-not-outcome-in-resilience-tests-2026-06-13.md) is the test whose outcome is reachable by the wrong path, [assert-which-gate-fired-when-error-variants-collide](assert-which-gate-fired-when-error-variants-collide-2026-09-17.md) is the test sitting on the wrong branch behind a shared error variant. This one is the test with no branch at all: the assertion is satisfied by the failure it exists to catch.

## When to Apply

- The assertion is `is_empty()`, `is_ok()`, `is_err()`, `== 0`, or "unchanged", and the test has no sibling asserting the positive case over the same data.
- The test name states a property ("does not lift the cap", "matches no row", "stays disabled") and the body asserts a different quantity.
- The gate is a number that varies with the machine.
- The test was written after the implementation, from what it currently returns.
- A sentence in a doc or ADR describes runtime behaviour and you cannot name the test that would fail if it were false.

Do not apply it by adding assertions for their own sake: an assertion that cannot fail is noise whether it is the only one or the fifth. One named failure per test is the bar.

## Examples

**Vacuous (before):** the metric cannot see the claim, and the negative has no control.

```rust
#[test]
fn an_open_read_snapshot_does_not_lift_the_cap() {
    let snapshot = store.db.begin_read().unwrap();
    let stats = churn(&store);
    // `used_bytes` counts the cache; a snapshot pins pages elsewhere.
    assert!(stats.used_bytes() <= SMALL_CACHE_BYTES);
}

#[tokio::test]
async fn a_null_literal_matches_no_row() {
    let provider = mem_table(&named_rows(&[(1, "bash", 10)]));
    let df = filter_by(provider, null_literal());
    assert!(pids(df).await.is_empty()); // so does NULL -> "" and NULL -> "zzz"
}
```

**Falsifiable (after):** the name says what the metric shows; the negative has a positive control and the degraded case it would hide.

```rust
#[test]
fn the_cap_holds_with_a_reader_open() { /* same body; the claim now matches the metric */ }

#[tokio::test]
async fn derive_a_null_literal_matches_no_row_as_sql_null_does() {
    let provider = mem_table(&named_rows(&[(1, "bash", 10), (2, "", 11)]));
    assert_eq!(pids(filter_by(&provider, string_literal("bash"))).await, vec![1]);
    assert!(pids(filter_by(&provider, null_literal())).await.is_empty());
}
```

**The question, applied:** "this fails when NULL is lowered to an empty string" — the second row makes that true. "This fails when the snapshot pins pages outside the cache" — nothing in the test can make that true, so the test does not claim it.
