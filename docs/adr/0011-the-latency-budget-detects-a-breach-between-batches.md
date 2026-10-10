# ADR-0011: The latency budget detects a breach between batches; it does not abort a match

**Date**: 2026-10-04 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

`DetectionEngine::observe_pattern_latency` disables a rule whose `REGEXP` pattern was measured over the configured budget, default 10 ms (R3, R7). T5 built that consequence and left T6 the measurement, which raised a question T5 could not answer without the executor: does the budget bound the match that breached it, or only the work that would have followed?

The distinction matters because the two readings give the guard different jobs. A per-match execution limit protects against one evaluation consuming unbounded time. A breach detector protects the remaining scan and every future one. T5's documentation implied immediacy without committing to it, and the module header said only that T6 observes.

## Decision

The budget is a **breach detector, checked at `RecordBatch` boundaries**. A pattern reported over budget disables its rule before the next batch is evaluated. No match, row, or batch in progress is interrupted.

The overrun an operator pays for a breach is therefore bounded by one batch per partition (`executor_target_partitions`, default 4, each finishing the batch it has in flight), not by one scan and not by one match.

## Alternatives Considered

### Alternative 1: Abort the match in progress (a true per-match execution limit)

- **Pros**: the strongest reading of "fail-closed"; the breaching evaluation itself costs nothing beyond the budget
- **Cons**: `regex` 1.13.1 exposes no timeout, deadline, or cancellation API — verified by searching the crate source. The only way to stop waiting is to run the match on another thread and abandon it, which leaves that thread running to completion
- **Why not**: abandoning a thread does not reclaim the CPU, it only stops observing it. The guard exists to protect a sustained-CPU budget, so a mechanism that keeps burning CPU while reporting success is worse than no mechanism. It would also be the second guard in this subsystem whose failure path defeats the control it guards, which is the defect [`a-guard-that-fails-open-is-worse-than-no-guard.md`](../solutions/security-issues/a-guard-that-fails-open-is-worse-than-no-guard.md) records

### Alternative 2: Check after the scan completes

- **Pros**: simplest to implement; one measurement per rule per cycle
- **Cons**: the breach is detected only after every row has been evaluated, so the first detection costs the full scan. At the retention windows T6 targets that is the whole cost the budget exists to prevent
- **Why not**: it makes the budget an observation rather than a control. The rule is disabled for the *next* cycle, having already spent the current one

### Alternative 3: Check per row

- **Pros**: the tightest bound available without cancellation
- **Cons**: a budget check on every row is overhead on the hot path, and the saving over a batch check is one batch's rows
- **Why not**: a 10 ms budget against a default 8192-row batch is roughly a microsecond per row, so per-row checking measures the same thing at a far worse constant. Batch boundaries are a checkpoint the executor already has

## Consequences

### Positive

- The checkpoint is free. A batch boundary is a place the stream already yields, so stopping needs no cancellation primitive and no extra thread.
- The bound is statable without measuring anything: one batch per partition. That matters in a repository whose standing learning is that an unmeasurable ceiling is not a bound.
- A linear-time engine with a bounded program and DFA already bounds a single evaluation, so the thing left unbounded was the *repetition*, and repetition is exactly what a batch-boundary check stops.

### Negative

- A pathological pattern still costs up to one full batch before the rule is disabled. An operator reading "fail-closed" may expect less.
- The guard's precision depends on batch size, which the executor sets. A much larger batch loosens the bound proportionally, with nothing in this decision to stop it.

### Risks

- Nothing in T5's code enforces where T6 measures; this decision lives in prose until the executor exists. T6 should make the measurement site the only place that constructs the report, so the batch boundary is structural rather than remembered.
- "Fail-closed" remains bounded by the TTL on the wire for the pushed half: a task already dispatched to a collector runs out its TTL because no revoke message exists in the protocol. That is a separate limit from this one and is recorded with the learning, not here.
