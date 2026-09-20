---
title: A measurement harness can pass its own correctness gate while measuring the wrong thing
date: 2026-09-19
category: docs/solutions/best-practices
module: workspace-wide
problem_type: best_practice
component: testing_framework
severity: high
applies_when:
  - building a benchmark, spike, or harness whose numbers will decide something
  - a harness has both a cost signal (time, memory, size) and a correctness signal (a known answer it must reproduce)
  - a generated fixture plants a known set of matches that the harness asserts on
  - two implementations are compared against each other and agreement is treated as proof the run is valid
  - a query or scan is bounded by a range, and the fixture's data is not uniformly distributed across that range
  - reviewing recorded performance numbers before acting on them
tags:
  - benchmarking
  - test-fixtures
  - measurement-fidelity
  - false-positive-tests
  - criterion
  - partitioned-storage
related_docs:
  - docs/solutions/best-practices/assert-which-gate-fired-when-error-variants-collide-2026-09-17.md
---

# A measurement harness can pass its own correctness gate while measuring the wrong thing

## Context

The T4 feasibility spike existed to produce one number: what a candidate query engine costs in memory and latency over the event store, against a control arm doing the same reads with no engine. That number decides an architectural commitment, so the spike was built with a correctness gate — both arms run the same workload and must return the same result set, or no measurement counts.

The gate looked strong. Two independently written implementations, one in SQL and one in plain Rust. A planted constant recomputed from raw store rows rather than trusted from a struct. Real storage on disk, a real query engine, no mocks. It still could not see the defect it most needed to catch.

## The shape of the problem

The fixture generated 120,000 events spread evenly across 26 hourly partitions, and planted 500 parent/child pairs the join had to find. It emitted those pairs as the **first** `2 * planted` rows. At 780 ms between rows, the last planted row landed 0.22 hours in — so all 500 matches sat inside partition 0 of 26.

The measured query bounded the full 26-hour span. So:

- the **cost** signal — every recorded memory and latency number — came from decoding all 26 partitions;
- the **correctness** signal — "both arms found 500" — came from one of them.

A pruning or decode defect anywhere after the first hour would have changed every recorded number and still reported exactly 500 matches. The gate was structurally incapable of noticing, and it would have looked rigorous the whole time.

Nothing about the assertion was weak. The blind spot was that nobody checked whether the data exercising correctness overlapped the data driving cost.

## The check that catches it

For any harness with both signals, ask one question before trusting a number:

> Does the data my correctness check exercises cover the data my cost measurement touches?

Make it mechanical, because eyeballing a generator does not surface a 780 ms step size. Assert the distribution, not just the total:

```rust
// Not enough: proves the count, says nothing about where the matches live.
assert_eq!(total_matches, spec.planted);

// The guard: matches must reach nearly every partition the query scans.
assert!(
    buckets_with_matches >= 20,
    "planted matches must reach nearly every bucket, not cluster in one: \
     only {buckets_with_matches} of {} hold a match",
    stats.buckets
);
```

The fix in the generator was to plant a pair every `rows / planted` indices instead of packing them at the front, so matches land in at least 20 of 26 partitions. See `plant_stride` and `row_for` in `spikes/datafusion-gate/src/fixture.rs`, and the guard test `planted_matches_are_spread_across_the_whole_span_not_clustered_in_one_bucket` in `spikes/datafusion-gate/tests/fixture.rs`.

## Why agreement between two implementations is not the guarantee it looks like

Two arms agreeing bounds one class of defect: one arm being wrong on its own. It says nothing about defects both arms inherit.

In this harness both arms read through the same storage calls and the same generated fixture. A defect in either would move both arms identically, and the equivalence check would stay green. The same review pass surfaced a second version of this: the equivalence test ran a 6,000-row spec while the recorded numbers came from a 120,000-row one, so the "both arms agree" claim behind the published figures was a human comparing two lines of program output, not a test. Run the gate against the spec the numbers come from.

## Prevention

- Name both signals explicitly when designing a harness, then state which data feeds each. If the answer differs, that gap is the bug.
- Assert where planted data lives, not only how much of it there is.
- Run the correctness gate on the same configuration the recorded numbers come from. A gate that only runs against a smaller fixture is not guarding the published result.
- Treat cross-implementation agreement as covering independent defects only. Enumerate what both sides share — the fixture, the storage layer, the clock — and accept that agreement is silent about all of it.
- When a harness is disposable, remember its guard tests die with it. The reasoning has to live somewhere that outlasts the crate.
