# T6 · U9 — Full-retention memory characterization

**Date:** 2026-10-08 · **Ticket:** T6 · **Governs:** R13, R11, R9 · **Harness:** `daemoneye-lib/tests/detection_execution_memory.rs`, run by `just measure-detection-memory`

R13 asks for a characterization, not a pass/fail number, and this is one. The first pass (page cache at redb's default) tripped R13's stop condition as written: peak RSS at 168 buckets (358.86 MiB) was 1.73x the peak at 84 buckets (207.09 MiB). The cause was configuration, not the executor: `EventStore` opened redb with its default **1 GiB** read cache and nothing in `DatabaseConfig` reached it. This revision adds `database.page_cache_mb` (default **32**, range 4-1024), re-measures across cache sizes and at a run length long enough to reach the plateau, and diagnoses the 426/617 MiB spike. Section "Stop condition" gives the verdict: **resolved, with two findings left open**.

The production-shaped run, the one an operator sizing a deployment cares about, peaks at **31.23 MiB** against the 100 MiB target, and that figure does not move with the cache size (31.2-33.0 MiB from 4 to 1024 MiB).

Every number was measured on **macos/aarch64**, release profile with `overflow-checks = true`, `lto = "thin"`, `codegen-units = 1` (the shipped profile). Linux and Windows are unmeasured. RSS is `sysinfo`'s per-process resident size, sampled from a 5 ms background thread and at every cycle boundary, reporting the larger.

## Which run is which

The five runs are two different shapes, and they answer different questions.

- **Full-retention shape (runs 1-4).** The window spans many buckets, so the provider supplies the target partition count itself and the physical optimizer inserts **no** `RepartitionExec`. This answers R13's question, whether memory tracks the window. Production never executes this shape: R3 puts a cycle's window inside one bucket. Ad-hoc wide-window queries and post-outage catch-up are the only places it could occur.
- **Production shape (run 5).** One bucket, so the provider emits one partition against a target of four and the optimizer inserts a `RepartitionExec` that reserves pool memory (pinned in `detection_execution.rs` by `session_pool_error_needs_a_repartition_and_vanishes_at_one_partition`). The harness asserts the plan does or does not contain `RepartitionExec` for each run, so a run cannot silently measure the wrong shape.

## Results

Three rules per cycle (an index-served `name = 'nc'`, the same with a `pid > 100000` residual, and a `command_line LIKE '%planted%'` full scan that reads every row of the window). Latency is per cycle, all three rules together.

### At the chosen default (`page_cache_mb = 32`), from `just measure-detection-memory`

1,500 cycles per run. Two independent runs of the whole recipe are given, because the run-to-run spread on this host is wide enough that a single figure would mislead: the same configuration gave 120.20 and 145.91 MiB. Peak RSS is the larger of a 5 ms sampling thread and every cycle boundary.

| Run                                          | Buckets | Partitions (scan) | Batch size | Peak RSS (two runs) | p50 latency  | Max latency     | Batches (full-scan rule) | `RepartitionExec` |
| -------------------------------------------- | ------- | ----------------- | ---------- | ------------------- | ------------ | --------------- | ------------------------ | ----------------- |
| 1. Default                                   | 168     | 4                 | 8192       | **120.20, 145.91**  | 156.9, 164.4 | 262.0, 543.8 ms | 188                      | no                |
| 2. Default, half the data                    | 84      | 4                 | 8192       | **118.64, 148.89**  | 78.6, 82.1   | 105.9, 154.5 ms | 96                       | no                |
| 3. Partitions lowered                        | 168     | 2                 | 8192       | **91.52, 111.09**   | 266.5, 273.0 | 290.4, 729.8 ms | 186                      | no                |
| 4. Batch size lowered                        | 168     | 4                 | 2048       | **99.94, 119.91**   | 154.5, 161.9 | 198.6, 227.7 ms | 740                      | no                |
| 5. Default, single bucket (production shape) | 1       | 1 (target 4)      | 8192       | **32.91, 32.78**    | 3.00, 3.11   | 6.00, 6.58 ms   | 2                        | yes               |

Baseline RSS (store open, executor built, nothing run) was 21.6 MiB in every run. The max-latency column is the most host-sensitive number here: the 543.8 and 729.8 ms figures come from the run made while a security scanner held the CPU, and the same runs quiet gave 262.0 and 290.4 ms. Treat the p50 column as the signal and the max column as an upper bound measured under contention.

**Peak RSS moves with the configured bounds, in the direction the structural model predicts.** Lowering `executor_target_partitions` from 4 to 2 moved it from 120.2-145.9 to 91.5-111.1 MiB, and lowering `executor_batch_size` from 8192 to 2048 moved it to 99.9-119.9 MiB. Both are outside the run-to-run spread in the same direction, which is what the decode-memory model expects: a cycle's decode working set is roughly `executor_target_partitions x 2 x executor_batch_max_bytes` (KTD2's per-partition stream capacity) and sits outside the DataFusion pool rather than inside it. An earlier reading of single runs reported these as "inside the spread"; two runs each show otherwise.

### The cache-size dimension (full-retention shape, release profile, RSS, 300 cycles, three repeats unless noted)

| redb cache                 | 168 buckets: peak RSS                              | p50 / worst max latency                    | 84 buckets: peak RSS                    | p50 / worst max latency  | Production shape: peak RSS | p50 / max      |
| -------------------------- | -------------------------------------------------- | ------------------------------------------ | --------------------------------------- | ------------------------ | -------------------------- | -------------- |
| 1024 MiB (the old default) | 359.7-363.8                                        | 134.8-138.9 ms / 249.5 ms                  | 212.1-213.2                             | 67.3-68.5 ms / 96.9 ms   | 32.81                      | 3.10 / 5.55 ms |
| 256 MiB                    | 358.9-363.1                                        | 135.4-136.7 ms / 268.6 ms                  | 208.4-217.3                             | 67.9-68.7 ms / 152.0 ms  | 32.88                      | 3.11 / 6.90 ms |
| 64 MiB                     | 193.5, 223.6, 258.0                                | 161.7-163.3 ms / 280.9 ms                  | 252.7, 334.3, 398.9                     | 80.8-81.9 ms / 163.4 ms  | 33.03                      | 3.13 / 5.04 ms |
| **32 MiB (default)**       | 114.9 (also 112.2-115.8, five runs at 100 cycles)  | 154.2 ms / 209.3 ms (155 ms at 100 cycles) | 134.4 (135.3 and 144.9 at 1,500 cycles) | 77.8 ms / 154.1 ms       | 31.23                      | 2.99 / 4.36 ms |
| 16 MiB                     | 120.1-129.5 (eight runs at 100 cycles: 92.8-166.6) | 166.6-231.7 ms / 335.9 ms                  | 98.2-131.6                              | 83.4-98.1 ms / 203.99 ms | 31.36                      | 3.07 / 4.55 ms |
| 4 MiB                      | 80.2-84.1 (but see below)                          | 164.1-171.9 ms / 337.4 ms                  | 82.2-92.4                               | 84.5-85.2 ms / 174.8 ms  | 31.64                      | 3.20 / 5.10 ms |

How to read it.

- **The growth with bucket count is a page-cache effect.** At 1024 and 256 MiB the 168-bucket peak is 1.7x the 84-bucket peak. At 64 MiB and below the 84-bucket peak is no longer the smaller of the two.
- **Latency pays for it, and only in the full-retention shape.** Bounding the cache costs 14% (32 MiB, 154 vs 135 ms) to 27% (4 MiB, 171 ms) at 168 buckets, and 15% to 25% at 84. The production shape is unaffected at every size (2.99-3.20 ms), because its one-bucket working set already fits. The full-retention shape was already outside the 100 ms alert-latency budget at the old default (134.8-138.9 ms) and stays outside it at every size; lowering partitions to 2 costs a further 1.7x.
- **Caveats on the latency column.** The 514 MiB fixture sits in the macOS file cache, so a redb "miss" here is a copy from kernel memory, not a disk read. The small-cache latency cost measured is a floor; a host under memory pressure would pay more. Part of the RSS saved is also memory that moved into the reclaimable kernel file cache. A cold-cache run needs `sudo purge` and was not done. The host also had a security scanner (Moonlock, ~78% CPU) running throughout and a load average near 6.4, so single latency figures such as 190 and 232 ms at 16 MiB are noisier than the p50 spread suggests.
- **The harness itself did not move the answer much.** daemoneye-lib's dev build enables redb's `cache_metrics` (the wiring tests need it), which adds relaxed atomic increments on every cache access in the measurement binary. The 1024 MiB p50 here (134.8-138.9 ms) matches the first pass's 133.72 ms, whose repeats ranged 133-140 ms, so any overhead is inside the run-to-run spread.
- **Write path.** Building the 1,512,000-row fixture through a 16 MiB cache ran at 373,426 rows/s and through 1024 MiB at 369,553 rows/s (32 MiB: 365,063), against the 1,000 records/s budget. The page cache does not throttle ingest in this fixture.

**Batches per bucket.** The plan expected at least two batches per bucket. That is not how the scan batches: a partition fills a batch across bucket boundaries, so 168 buckets of 9,000 rows came out as 188 batches (1.1 per bucket), which is 1,512,000 rows over 8,192 plus a partial batch per partition. The harness asserts the floor by rows (`rows_read / batch_size`), not by bucket. In the single-bucket run, 9,000 rows are two batches.

**R9, the byte bound under worst-case rows.** A separate store of 24 rows, each a 255-byte name, 4,096-byte path, 64-byte hash and 1 MiB command line, closed batches on `executor_batch_max_bytes` (4 MiB): 8 batches, where the 8,192-row bound alone would give 1. No row was excluded as oversized. Peak RSS for that run was 132.86 MiB at the 32 MiB cache (146.91 MiB at the old default), which includes the 52 MiB store being read through the page cache.

**Planted matches.** Each of the 168 buckets holds about six planted `nc` rows (1,000 in all), so planted matches land in 168 of 168 buckets. Every rule returned exactly the planted count in every run (1,000 at 168 buckets, 500 at 84, 5 in one bucket), and the full-scan rule's `rows_read` equalled `buckets x 9,000` exactly.

## Plateau

**The harness reports the shape of the series; it does not assert on it.** Each `RUN` line carries the median of each of ten equal blocks of cycles (`rss_mib_blocks`), plus the third- and fourth-quarter medians.

From the official run at the default:

| Run          | Block medians (MiB, 150 cycles each)    |
| ------------ | --------------------------------------- |
| default-168  | 110 116 116 116 116 116 116 120 120 120 |
| default-84   | 107 115 115 116 119 119 119 119 119 119 |
| partitions-2 | 91 91 91 91 91 91 91 92 92 92           |
| batch-2048   | 86 89 96 96 96 100 100 100 100 100      |
| production   | 31 33 33 33 33 33 33 33 33 33           |

Resident size is a **staircase whose treads lengthen**: a warm-up ramp over roughly the first 100-900 cycles, then 4-6 MiB steps that grow rarer, then flat. Every shape is flat across its last three or more blocks, and the production shape never leaves 33 MiB. A 1,500-cycle trace of `default-168` sampled per cycle shows the same thing in detail: 94 to 150 MiB in the first 100 cycles, flat to cycle 500, one step to 155, flat, 156 by cycle 900, then unchanged for the last 600 cycles.

**Why there is no plateau assertion.** An earlier revision asserted that the fourth-quarter median stayed within 4 MiB of the third's. At 300 cycles that gate tripped on 3 of 5 runs, which read as unbounded growth; it was in fact measuring the warm-up ramp, because at 300 cycles both of the compared quarters sit inside it. At 1,500 cycles it still tripped on the two shapes where a single late step happened to land in the last quarter. The gate cannot distinguish one 4-6 MiB allocator step from a slow leak, and neither can any threshold over one process's resident size at a run length worth paying for:

- Widening the slack until the steps fit is fitting the test to the host.
- An absolute MiB ceiling is the pass/fail gate this number is explicitly not (see AGENTS.md, Performance Budgets).
- Comparing back-half growth against front-half growth fails too, and more subtly: the front half contains the warm-up ramp, so any leak smaller than the ramp passes and only an *accelerating* leak is caught.

So the plateau is reported and read, and `DEFAULT_CYCLES` is 1,500 so that what is reported is the plateau rather than the ramp. The assertions that can still fail in these tests are the validity ones: `rows_read == buckets x 9,000`, the exact planted-match count per rule, planted matches spanning at least 90% of the window's buckets, the presence or absence of `RepartitionExec` per shape, R9's byte-bound batch count, and the sampler erroring rather than reporting zero. Those are what caught the first harness run reading 1,000 rows instead of 1,512,000.

## Stop condition

> Stop and report if U9's characterization shows peak RSS growing with bucket count rather than tracking the configured partition and batch bounds.

**Verdict: resolved, with two findings left open.**

- **Growth with bucket count: resolved.** It had one cause, the page cache. At the old default the 168-bucket peak was 1.7x the 84-bucket peak (358.9 vs 207.1 MiB, reproduced at 359.7-363.8 vs 212.1-213.2). At 32 MiB the two shapes overlap: 120.2-145.9 MiB at 168 buckets against 118.6-148.9 at 84. The same holds at 64, 16 and 4 MiB.
- **Tracking the configured bounds: shown in direction, not in proportion.** Peak RSS falls when `executor_target_partitions` or `executor_batch_size` falls, outside the run-to-run spread both times (see the results table). It does not scale proportionally with either, and it should not be expected to: the page cache, the allocator's retained pages and the baseline all sit outside those bounds. The bounds themselves are structural rather than measured. `GreedyMemoryPool` *refuses* a reservation past `executor_memory_pool_bytes` — `session_pool_error_needs_a_repartition_and_vanishes_at_one_partition` pins that a 2-partition plan over a tiny pool errors where 1 partition succeeds — and decode memory outside the pool is bounded by the per-partition stream capacity. Neither bound is evidenced by RSS, and RSS is not the instrument for them.

Left open, as findings rather than as the stop condition:

1. **The within-run staircase.** Treads lengthen and every run ends flat, but 1,500 cycles cannot prove the last step has been taken. See "Plateau".
2. **The full-retention shape misses both the memory goal and the latency budget.** 118-149 MiB peak RSS against the 100 MiB goal, and a live footprint of 132 MiB in the one run sampled for it, so the excess is not only retained allocator pages. p50 is 157-164 ms against a 100 ms per-rule budget. Production does not execute this shape (R3 puts a cycle's window inside one bucket); it is reachable through ad-hoc wide-window queries and post-outage catch-up.

### Diagnosis of the 426/617 MiB spike

The spike is mostly freed memory the allocator has not returned, not live data. Evidence:

- **`footprint(1)` at the moment of peak RSS.** macOS `footprint -p` splits memory into Dirty and Reclaimable and excludes Reclaimable from its total. In eight 4 MiB runs sampled at 1 Hz, the normal runs had RSS 76-81 MiB with a `Footprint:` of 19-26 MiB and 34-45 MiB of Malloc Small reclaimable. The two spiking runs had RSS 291 and 210 MiB, a `Footprint:` of 50 and 49 MiB, and **218 and 138 MiB** of Malloc Small reclaimable. A third spike in the first batch of repeats showed RSS 209 MiB against a footprint of 19 MB and 168 MB reclaimable. In every sampled run, RSS was approximately footprint plus reclaimable.
- **It scales with scan concurrency.** At a 4 MiB cache, partitions 4 gave spikes in about 2 runs of 8 (and the U9 probe saw 426 and 617 MiB). Partitions 2 gave 49-53 MiB in all 13 runs I made, with 11-18 MiB reclaimable. That fits freed pages piling up across four threads' malloc magazines. A plausible contributor, which I read in redb's source but did not isolate: each cache miss allocates a fresh 4 KiB page buffer and each eviction frees one, which falls in macOS's small-allocation range, and a small cache means constant churn of those.
- **Long-lived read transactions are not the cause.** `BucketReader` holds one `ReadTransaction` for a partition scan. A held snapshot keeps old page versions alive in the file, which is disk, not RAM, and nothing writes during these runs. The no-writes argument is what rules it out; `the_cap_holds_with_a_reader_open` only shows the cap holding with a reader open, and could not have shown otherwise, because `used_bytes` is redb's own cache accounting and anything pinned outside the cache would never appear in it.

What this does not explain. Live footprint also rose in the spiking runs, from about 20 to about 50 MB, and a 20 ms in-harness `footprint` sampler saw 97-147 MiB footprint peaks in some 4 MiB runs. `footprint` cannot tell live data from memory that was freed but not yet marked reusable, so that part is unsettled. **Why a given run spikes and its neighbour does not is unexplained.** The spike rate also is not monotonic in cache size: 64 MiB was the noisiest bounded size in the sweep and 32 MiB the steadiest, with no mechanism I can offer. A counting allocator would settle live versus retained, but `unsafe_code = "forbid"` rules one out of the tree.

The better metric on macOS is `phys_footprint` (what `footprint` reports and what jetsam acts on). RSS stays the cross-platform metric and an upper bound. The harness reads footprint when `DETECTION_MEMORY_FOOTPRINT=1` is set on a macOS host. It is opt-in because spawning `footprint` every 20 ms perturbs the RSS peak: the same 4 MiB configuration read 70-80 MiB without it and 108-232 MiB with it. Footprint runs are a diagnostic, not the record. Linux and Windows have not been measured at all.

## The 100 MiB target

The target is a deployment-sized goal, not an invariant, so a miss is a finding.

- **Production shape: met, at every cache size.** 32.9 MiB peak at the default, 11.3 MiB above the 21.6 MiB baseline, p50 3.00 ms, max 6.00 ms, and flat at 33 MiB from the second block of cycles onward.
- **Full-retention shape: not met at the default.** 120.2-145.9 MiB at 168 buckets and 118.6-148.9 at 84. Live footprint was 132 MiB in the run sampled for it, so this is not only reclaimable allocator memory. Lowering partitions to 2 brings peak RSS to 91.5-111.1 MiB at a 1.7x latency cost; lowering the batch size to 2048 gives 99.9-119.9 MiB at no p50 cost, which is the cheaper of the two if an operator needs this shape to fit.

**Chosen default: `database.page_cache_mb = 32` (range 4-1024); `executor_target_partitions = 4` and `executor_batch_size = 8192` unchanged.**

Why 32. The criteria were: remove the bucket-count growth, keep the worst peak I observed low, and pay the least latency. 1024 and 256 MiB fail the first. 64 MiB removed the growth but gave the worst and noisiest peaks (up to 398.9 MiB at 84 buckets). 4 MiB gave the lowest typical peak (80-92 MiB) but spiked to 205-617 MiB in about one run in four at 100 cycles and cost the most latency. 16 MiB had a worst peak of 166.6 MiB and 190-232 ms p50 in two of three 300-cycle runs. 32 MiB had the lowest worst case (148.9 MiB across every 32 MiB run, 100 to 1,500 cycles) and the smallest latency cost of the bounded sizes (+14% at 168 buckets). This is a judgement among noisy options. It is not a measured optimum, 16 MiB is defensible, and the 4 MiB floor is available to an operator who values typical memory over worst case.

The trade this puts to an operator: the default leaves the full-retention shape at 118-149 MiB and about 14% slower in exchange for removing the 360 MiB figure, and costs nothing in the production shape. 4 MiB gives 80-92 MiB typical at the cost of a roughly one-in-four spike risk and 27% latency.

## How the fixture avoids a silent empty scan

`PostingsCache` keeps posting lists for buckets older than the current wall-clock bucket, so a fixture that wrote closed-bucket rows after a reader had cached their lists would read zero rows and report a flatteringly small peak (U13 hit this). Here every bucket is fixed in 2023, so all are closed and the cache is live, and the store is written completely, in its own process, before any executor exists. Nothing can be cached before it is written. Each run also asserts, on its first cycle, `rows_read == buckets x 9,000`, the expected match count for all three rules, and that planted matches span at least 90% of the window's buckets. A scan that read nothing fails the run.

## What was measured

- **Fixture.** 168 hourly buckets x 9,000 rows (1,512,000 rows), deterministic from index arithmetic, one `name = 'nc'` row planted every 1,512 rows. Rows are realistic, not maximum-sized: a 4-16 byte name, a roughly 16-byte path and a roughly 60-90 byte command line. The store is **514 MiB on disk** (about 356 bytes per row including the pid, ppid, name and hash indexes). The plan's wording, "every row's variable-length fields filled to their maximum admitted size", would need about 1.5 TB at the 1,053,119-byte worst case and cannot be built, so maximum-sized rows are exercised in a separate 24-row bucket (52 MiB on disk) for R9.
- **Rules.** The plan's `name = 'nc'` and its `pid > 100000` residual variant are both served by the store's indexes and read about 1,000 rows, not the window. `pid > 100000` alone is index-served too: the pid index handles range predicates. The scan-shaped rule is therefore `command_line LIKE '%planted%'`. The first harness run read 1,000 rows instead of 1,512,000, and the `rows_read` assertion is what caught it.
- **One process per run.** A process's resident-set peak cannot be reset, so each run is its own test and, under nextest, its own process. The fixture is built by a separate first step so building never inflates a measured peak.
- **Cap.** `max_matches_per_rule = 100_000` per the plan; every other field is `DetectionConfig::default()`. The page cache is `DatabaseConfig::default()` unless `DETECTION_MEMORY_PAGE_CACHE_MB` overrides it.

## Reproducing

```bash
just measure-detection-memory
DETECTION_MEMORY_PAGE_CACHE_MB=4 just measure-detection-memory   # another cache size
DETECTION_MEMORY_FOOTPRINT=1 just measure-detection-memory       # macOS: live footprint beside RSS
```

This builds the fixtures under `target/tmp/detection-memory/` (about 566 MiB, gitignored) and runs the six measured tests with `--test-threads 1 --no-fail-fast` under `set -euo pipefail` with no `tee`. Each test prints one `RUN` line carrying its peak, latency percentiles and ten block medians of the per-cycle series; the series is reported, not asserted on (see "Plateau"). `DETECTION_MEMORY_CYCLES=<n>` shortens the runs for development, at the cost of reporting the warm-up ramp rather than the plateau, and `DETECTION_MEMORY_TRACE=1` prints the full per-cycle RSS series. `.config/nextest.toml` carries a slow-timeout override for this binary, since the default profile would kill a test at 120 seconds.

## What this does not answer

- **Why some runs spike.** Mostly retained allocator memory, but the run-to-run selection is unexplained; see the diagnosis.
- **How wide the run-to-run spread really is.** Two full recipe runs at identical settings differed by up to 25 MiB per shape (120.2 vs 145.9 MiB at 168 buckets). Two runs bound it loosely; the spread's distribution is unmeasured, and every peak here should be read as a sample, not a constant.
- **Whether the RSS steps stop.** 1,500 cycles is the longest run.
- **Cold-cache latency.** The fixture is in the OS file cache throughout.
- **Linux and Windows.** Allocator behaviour, page-cache behaviour and RSS accounting differ by OS.
- **Other redb opens.** `storage::schema` (the export and rebuild path) still opens redb with the 1 GiB default; it runs offline, not per cycle.
- **Concurrent ingest.** Nothing wrote to the store during the scans.
- **Rule mix.** Three rule shapes only; no aggregation, join or `REGEXP`.
