# ADR-0014: A detection cycle evaluates the rows its own window covers

**Date**: 2026-10-10 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

Each detection cycle runs every eligible rule over the event store. The store retains history, so a rule can be run over everything retained or over only what the cycle added.

Run over everything, a row that matched in one cycle matches again in every cycle until retention drops it, and each match is another alert. The cost also grows with retention rather than with the work done in the cycle. The memory characterization in `docs/decisions/2026-10-08-t6-full-retention-memory.md` measures this. A full-retention evaluation of 168 hourly buckets peaked at 118-149 MiB resident against a 100 MiB goal, with a p50 of 157-164 ms against the 100 ms per-rule budget. The same three rules over one bucket peaked at about 33 MiB with a p50 of 3.0 ms.

## Decision

A cycle evaluates each rule against the half-open interval of `collection_time` it covers, `(after_ms, through_ms]`, where `after_ms` is the previous cycle's high-water mark and `through_ms` is this cycle's. Consecutive windows tile the timeline, so a row is seen by one cycle while `collection_time` is monotonic. A row stamped at or before the mark (a clock step-back) widens the next window down to it, and the rows in the overlap are evaluated again: a row is evaluated at least once, never zero times.

## Alternatives Considered

### Alternative 1: Evaluate over all retained history every cycle

- **Pros**: no window bookkeeping; a rule always sees the whole picture
- **Cons**: each match is re-alerted every cycle. Memory and latency track retention, not the cycle's work, and the record shows the full-retention shape missing both the memory goal and the latency budget
- **Why not**: it makes the measured worst case the routine case, and it duplicates alerts

### Alternative 2: Evaluate over a fixed trailing duration

- **Pros**: bounded cost; simple to explain
- **Cons**: windows overlap, so a row matches in several cycles, and a cycle delayed past the duration leaves a gap no cycle covers
- **Why not**: overlap duplicates alerts and a gap loses them. A high-water mark has neither failure

## Consequences

### Positive

- A cycle's cost follows the rows it added. Under the production shape, where a window sits inside one bucket, the characterization measures about 33 MiB and a p50 of 3.0 ms.
- A matching row alerts at least once: exactly once while the clock is monotonic and the agent stays up. A crash between storing a cycle's alerts and persisting its mark re-evaluates that window on restart, and a clock step-back re-evaluates the overlap; `deduplication_key` is what a sink dedups on.

### Negative

- A rule sees only the current window. Anything that needs history must be expressed against the store or the derived tables, not assumed from a rerun.
- The full-retention shape still exists. A wide ad-hoc query or a catch-up window after an outage reads many buckets, and the record shows what that costs. It is a documented limit, not a fixed one.

### Risks

- The high-water mark is the only record of what has been evaluated. A wrong mark skips rows or repeats them. The window type documents which end is exclusive and which inclusive so the tiling holds.
