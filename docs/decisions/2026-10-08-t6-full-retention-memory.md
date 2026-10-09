# T6 · U9 — Full-retention memory characterization

**Date:** 2026-10-08 · **Ticket:** T6 · **Governs:** R13, R11, R9 · **Harness:** `daemoneye-lib/tests/detection_execution_memory.rs`, run by `just measure-detection-memory`

R13 asks for a characterization, not a pass/fail number, and this is one. It also trips R13's stop condition as written: at the default configuration, peak RSS at 168 buckets (358.86 MiB) is 1.73x the peak at 84 buckets (207.09 MiB). That growth is real, steady and repeatable. Its cause is not the executor: a one-off experiment that shrank the store's page cache removed most of it. Section "Stop condition" has the evidence and the part I could not explain.

The production-shaped run, the one an operator sizing a deployment cares about, peaks at **32.73 MiB** against the 100 MiB target.

Every number was measured on **macos/aarch64**, release profile with `overflow-checks = true`, `lto = "thin"`, `codegen-units = 1` (the shipped profile). Linux and Windows are unmeasured. RSS is `sysinfo`'s per-process resident size, sampled from a 5 ms background thread and at every cycle boundary, reporting the larger.

## Which run is which

The five runs are two different shapes, and they answer different questions.

- **Full-retention shape (runs 1-4).** The window spans many buckets, so the provider supplies the target partition count itself and the physical optimizer inserts **no** `RepartitionExec`. This answers R13's question, whether memory tracks the window. Production never executes this shape: R3 puts a cycle's window inside one bucket. Ad-hoc wide-window queries and post-outage catch-up are the only places it could occur.
- **Production shape (run 5).** One bucket, so the provider emits one partition against a target of four and the optimizer inserts a `RepartitionExec` that reserves pool memory (pinned in `detection_execution.rs` by `session_pool_error_needs_a_repartition_and_vanishes_at_one_partition`). The harness asserts the plan does or does not contain `RepartitionExec` for each run, so a run cannot silently measure the wrong shape.

## Results

300 cycles per run, three rules per cycle (an index-served `name = 'nc'`, the same with a `pid > 100000` residual, and a `command_line LIKE '%planted%'` full scan that reads every row of the window). Latency is per cycle, all three rules together.

| Run                                          | Buckets | Partitions (scan) | Batch size | Peak RSS       | p50 latency | Max latency | Batches (full-scan rule) | `RepartitionExec` |
| -------------------------------------------- | ------- | ----------------- | ---------- | -------------- | ----------- | ----------- | ------------------------ | ----------------- |
| 1. Default                                   | 168     | 4                 | 8192       | **358.86 MiB** | 133.72 ms   | 175.56 ms   | 188                      | no                |
| 2. Default, half the data                    | 84      | 4                 | 8192       | **207.09 MiB** | 68.59 ms    | 96.02 ms    | 96                       | no                |
| 3. Partitions lowered                        | 168     | 2                 | 8192       | **340.00 MiB** | 230.05 ms   | 283.34 ms   | 186                      | no                |
| 4. Batch size lowered                        | 168     | 4                 | 2048       | **334.88 MiB** | 138.85 ms   | 180.96 ms   | 740                      | no                |
| 5. Default, single bucket (production shape) | 1       | 1 (target 4)      | 8192       | **32.73 MiB**  | 3.06 ms     | 6.15 ms     | 2                        | yes               |

Baseline RSS (store open, executor built, nothing run) was 21.4-21.5 MiB in every run. Max latency in runs 1-4 is the first, cold-page-cache cycle in some runs and a steady-state tail in others; across five repeats of run 1 it ranged 175-531 ms while p50 stayed within 133-140 ms.

**Batches per bucket.** The plan expected at least two batches per bucket. That is not how the scan batches: a partition fills a batch across bucket boundaries, so 168 buckets of 9,000 rows came out as 188 batches (1.1 per bucket), which is 1,512,000 rows over 8,192 plus a partial batch per partition. The harness asserts the floor by rows (`rows_read / batch_size`), not by bucket. In the single-bucket run, 9,000 rows are two batches.

**R9, the byte bound under worst-case rows.** A separate store of 24 rows, each a 255-byte name, 4,096-byte path, 64-byte hash and 1 MiB command line, closed batches on `executor_batch_max_bytes` (4 MiB): 8 batches, where the 8,192-row bound alone would give 1. No row was excluded as oversized. Peak RSS for that run was 146.91 MiB, which includes the 52 MiB store being read through the page cache.

**Planted matches.** Each of the 168 buckets holds about six planted `nc` rows (1,000 in all), so planted matches land in 168 of 168 buckets. Every rule returned exactly the planted count in every run (1,000 at 168 buckets, 500 at 84, 5 in one bucket), and the full-scan rule's `rows_read` equalled `buckets x 9,000` exactly.

## Plateau

For each run the harness records RSS after every cycle and compares the median of the third quarter of cycles with the median of the fourth, failing if the fourth exceeds the third by more than 4 MiB. Observed third-to-fourth deltas: 0.11, 0.11, 0.01, 0.07 and 0.15 MiB for runs 1-5. RSS rose in the first cycles and then stayed flat for the remaining hundreds. The T4 gate's eleven-call sample could not answer this question; 300 cycles per run can, for this fixture and this host.

## Stop condition

> Stop and report if U9's characterization shows peak RSS growing with bucket count rather than tracking the configured partition and batch bounds.

It does grow: 207.09 MiB at 84 buckets, 358.86 MiB at 168. Over the 21.5 MiB baseline that is 185.6 versus 337.4 MiB, 1.82x for 2x the data. Three repeats of runs 1 and 2 at 100 cycles each gave 358.28-359.14 MiB and 206.83-210.91 MiB, so the figure is stable. By the plan's own test this is the stop condition, and I did not tune the configuration around it.

What I could establish about the cause, with a temporary patch to `EventStore::open` that set redb's `cache_size` from an environment variable (reverted; nothing from it is in the tree):

| redb cache | 168 buckets                      | 84 buckets       |
| ---------- | -------------------------------- | ---------------- |
| default    | 358.86 MiB                       | 207.09 MiB       |
| 64 MiB     | 158.92 MiB                       | 138.47 MiB       |
| 4 MiB      | 68.52, 76.16, 425.97, 616.72 MiB | 69.36, 78.27 MiB |

At the default cache, which redb sizes at 1 GiB, resident memory scales with the data a scan touches, and a smaller cache removes most of the growth in the runs that behaved. `EventStore` opens redb with its defaults and no `DetectionConfig` field reaches it, so none of the executor's knobs can bound it. That points at the store's page cache rather than at the executor reading more than one bucket at a time, and the provider's two-slot channel per partition is consistent with that.

What I could not explain: two of four 4 MiB runs at 168 buckets peaked at 426 and 617 MiB and stayed there, with nothing in the scan counters that differed. They are not a fixture or sampler artifact that I can find, and they are not reproducible on demand. Until that is understood, the claim that executor memory is bounded by its partition and batch settings is **not established** by this measurement. With the cache at 4 MiB and the runs that did not spike, the knobs did show an effect:

| redb cache 4 MiB, 300 cycles | Peak RSS            |
| ---------------------------- | ------------------- |
| 168 buckets, default         | 616.72 MiB (spiked) |
| 84 buckets, default          | 78.27 MiB           |
| 168 buckets, partitions 2    | 56.44 MiB           |
| 168 buckets, batch size 2048 | 56.19 MiB           |
| 1 bucket, default            | 31.75 MiB           |

## The 100 MiB target

The target is a deployment-sized goal, not an invariant, so a miss is a finding.

- **Production shape: met.** 32.73 MiB peak, 11.3 MiB above the 21.5 MiB baseline, p50 3.06 ms.
- **Full-retention shape: not met at any configuration tried.** 334.88-358.86 MiB at the default store cache. Lowering partitions to 2 saves 18.9 MiB (5%) and costs 1.7x latency; lowering the batch size to 2048 saves 24.0 MiB (7%) at no latency cost. Neither moves the result near 100 MiB, because the store's page cache dominates it.

**Recommended default: keep `executor_target_partitions = 4` and `executor_batch_size = 8192`.** The evidence does not support changing them, since they are worth 5-7% against a store-level effect that is several times larger. The decision this measurement does put to you is whether `EventStore` should open redb with an explicit, smaller read cache; that is a store change outside this unit, and the spiking runs need explaining before relying on it.

## How the fixture avoids a silent empty scan

`PostingsCache` keeps posting lists for buckets older than the current wall-clock bucket, so a fixture that wrote closed-bucket rows after a reader had cached their lists would read zero rows and report a flatteringly small peak (U13 hit this). Here every bucket is fixed in 2023, so all are closed and the cache is live, and the store is written completely, in its own process, before any executor exists. Nothing can be cached before it is written. Each run also asserts, on its first cycle, `rows_read == buckets x 9,000`, the expected match count for all three rules, and that planted matches span at least 90% of the window's buckets. A scan that read nothing fails the run.

## What was measured

- **Fixture.** 168 hourly buckets x 9,000 rows (1,512,000 rows), deterministic from index arithmetic, one `name = 'nc'` row planted every 1,512 rows. Rows are realistic, not maximum-sized: a 4-16 byte name, a roughly 16-byte path and a roughly 60-90 byte command line. The store is **514 MiB on disk** (about 356 bytes per row including the pid, ppid, name and hash indexes). The plan's wording, "every row's variable-length fields filled to their maximum admitted size", would need about 1.5 TB at the 1,053,119-byte worst case and cannot be built, so maximum-sized rows are exercised in a separate 24-row bucket (52 MiB on disk) for R9.
- **Rules.** The plan's `name = 'nc'` and its `pid > 100000` residual variant are both served by the store's indexes and read about 1,000 rows, not the window. `pid > 100000` alone is index-served too: the pid index handles range predicates. The scan-shaped rule is therefore `command_line LIKE '%planted%'`. The first harness run read 1,000 rows instead of 1,512,000, and the `rows_read` assertion is what caught it.
- **One process per run.** A process's resident-set peak cannot be reset, so each run is its own test and, under nextest, its own process. The fixture is built by a separate first step so building never inflates a measured peak.
- **Cap.** `max_matches_per_rule = 100_000` per the plan; every other field is `DetectionConfig::default()`.

## Reproducing

```bash
just measure-detection-memory
```

This builds the fixtures under `target/tmp/detection-memory/` (about 566 MiB, gitignored) and runs the six measured tests with `--test-threads 1` under `set -euo pipefail` with no `tee`. `DETECTION_MEMORY_CYCLES=<n>` shortens the runs for development. `.config/nextest.toml` carries a slow-timeout override for this binary, since the default profile would kill a test at 120 seconds.

## What this does not answer

- **The spiking runs.** Why a 4 MiB store cache sometimes yields 426-617 MiB is unexplained.
- **Linux and Windows.** Page-cache behaviour and RSS accounting differ by OS.
- **Concurrent ingest.** Nothing wrote to the store during these runs.
- **Rule mix.** Three rule shapes only; no aggregation, join or `REGEXP`.
