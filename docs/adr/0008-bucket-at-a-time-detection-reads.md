# ADR-0008: Read detection windows bucket-at-a-time

**Date**: 2026-09-20 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

The event store partitions process events into per-hour tables (`processes.events@<bucket-id>`). `EventStore::scan_range(start, end)` resolves a time range to the buckets it covers and returns every decoded `ProcessRecord` in one `Vec` — the obvious way to read a detection window, and the shape the T4 spike's control arm used.

Measured, it is expensive. Reading a 26-hour window of 120,000 events that way retained roughly **481 MiB** resident across eleven repeated queries, against a whole-system budget of `<100 MiB`. A partitioned reader over the identical fixture and query stayed under **89 MiB**, because it decodes one bucket as its partition runs rather than materializing the range. The gap is not the query engine — both arms computed the same answer — it is when the records exist in memory.

Detection runs this query shape every collection cycle, so the cost is recurring, and `<100 MiB` is a hard budget rather than a target.

## Decision

Detection reads its time windows **one bucket at a time**, decoding each bucket as it is consumed and releasing it before the next. No detection path issues a single whole-range `EventStore::scan_range` call.

## Alternatives Considered

### Alternative 1: Whole-range `scan_range`, then filter in memory

- **Pros**: one call, no partition bookkeeping, the simplest code by a wide margin; `scan_range` already exists and is tested
- **Cons**: peak resident set scales with window size, not with working set; measured at ~481 MiB for a 26-hour window
- **Why not**: it alone consumes roughly five times the entire process budget on a window detection runs continuously

### Alternative 2: Cap the window instead of streaming it

- **Pros**: keeps the simple call, bounds memory by bounding the query
- **Cons**: caps what a rule can express; an operator asking for a 24-hour lineage view gets a silently truncated answer, or an error they cannot act on
- **Why not**: it trades a product capability for an implementation convenience, and the streaming alternative costs neither

### Alternative 3: Rely on the query engine's memory pool to bound it

- **Pros**: no change to the read path; DataFusion supports a configurable memory limit
- **Cons**: the pool bounds what the engine allocates, not what the store already materialized before handing rows over
- **Why not**: the 481 MiB was allocated by the read path, upstream of the engine — the pool never sees it

## Consequences

### Positive

- Peak resident set tracks bucket size rather than window size, so a longer detection window costs time instead of memory
- Partitioned reads map cleanly onto the query engine's own partitioning, so pruning a bucket also skips its decode
- The per-bucket boundary is a natural place to apply a time predicate, which the T4 spike's provider already demonstrated

### Negative

- More bookkeeping than one call: a reader must enumerate buckets, derive each window, and handle the empty-bucket case
- The store exposes no accessor for its bucket granularity, so a partitioned reader has to assume it and verify the assumption against real data

### Risks

- **Partition count is unbounded.** The T4 provider planned one partition per surviving bucket with no cap. Seven-day hourly retention holds up to 168 buckets against the 26 measured, and the engine's `target_partitions` bounds execution concurrency rather than scan partitioning. Cap the partition count, or measure at full retention, before relying on the headroom this ADR's numbers show.
- **Allocator retention is not fully characterized.** The 481 MiB figure is a high-water mark across eleven identical scans and grows from 66 MiB after one, which is consistent with fragmentation across many small per-record allocations rather than one call's working set. Bucket-at-a-time reads shrink each burst; whether they bound a long-running agent's steady state needs a longer run than the spike's eleven calls.

## Evidence

Measured by the T4 · M3 feasibility spike on macos/aarch64, release profile. Full numbers, method, and limitations: [docs/decisions/2026-09-19-t4-datafusion-gate.md](../decisions/2026-09-19-t4-datafusion-gate.md). The harness is preserved at commit `bfa6782`.
