# T4 · M3 — DataFusion feasibility gate: **GO**

**Date:** 2026-09-19 · **Ticket:** [T4 · M3](<../../spec/full/tickets/T4_%C2%B7_M3_%E2%80%94_DataFusion_feasibility_spike_(gate).md>) · **Governs:** ADR-0006, T6

DataFusion clears all three measured criteria. T6 proceeds on DataFusion; the hand-rolled executor contingency is not invoked. Two findings below qualify that verdict and one of them changes a stated constraint.

## Verdict against the gate

| Criterion                             | Threshold           | Measured                           | Result                                              |
| ------------------------------------- | ------------------- | ---------------------------------- | --------------------------------------------------- |
| Absolute peak RSS                     | `< 100 MiB`         | **88.72 MiB**                      | pass, 11.28 MiB headroom                            |
| Per-rule latency                      | `< 100 ms`          | **10.15 ms** p50, **12.33 ms** max | pass, ~10x headroom                                 |
| Cross-arm equivalence (R18)           | both arms identical | **500 = 500**, enforced by test    | pass                                                |
| Marginal RSS over the control ceiling | `<= 40 MiB`         | −391.78 MiB                        | see below: an upper bound, not a test that can fail |
| Release binary size delta             | no threshold        | **+62.66 MiB** stripped (54.2x)    | maintainer's judgment, below                        |

**The absolute peak is the real memory gate.** The marginal criterion compares DataFusion against the control arm's 480.81 MiB ceiling, and the control arm materializes the whole 26-hour range in one call while the provider reads one bucket at a time. Those are different memory strategies at very different sizes, so the marginal number bounds DataFusion from above and cannot fail. It is reported because it is informative, not because passing it means anything.

Every number was measured on **macos/aarch64**, release profile with `overflow-checks = true`, `lto = "thin"`, `codegen-units = 1` — the same profile DaemonEye ships. Linux and Windows are unmeasured.

## What was measured

A disposable crate at `spikes/datafusion-gate/`, excluded from the workspace with its own lockfile. It generates a real T3 event store — **120,000 process events across 26 hourly `processes.events@<id>` partitions**, deterministic from a fixed seed — and runs one representative detection query two ways:

- **DataFusion arm** — `SessionContext` (`target_partitions = 4`, `batch_size = 8192`) over a redb-backed `TableProvider` that maps one bucket to one partition and prunes on a `ts_ms` range predicate. It answers the join with DataFusion's own hash join and does not consult the MRC parent map.
- **Control arm** — the same partitions decoded into memory through `EventStore::scan_range` with no engine on top. It is the materialization ceiling, the latency floor, the reference answer, and the binary-size baseline.

The query is a ShadowHunt parent/child lineage self-join: a shell process whose parent is a long-running service, bounded by a time predicate.

### Full measurements

|                                         | control arm      | DataFusion arm  | delta                |
| --------------------------------------- | ---------------- | --------------- | -------------------- |
| Baseline RSS (store open, nothing else) | 6.53 MiB         | 9.87 MiB        | **+3.34 MiB**        |
| After building the `SessionContext`     | —                | 17.44 MiB       | —                    |
| RSS after one query                     | 66.00 MiB        | 82.09 MiB       | **+16.09 MiB**       |
| Peak RSS over 11 queries                | 480.75 MiB       | 88.72 MiB       | −392.03 MiB          |
| Latency p50 / max                       | 24.64 / 24.93 ms | 9.75 / 10.63 ms | **2.5x faster**      |
| Decode-only pass                        | 23.41 ms         | —               | —                    |
| Release binary, stripped                | 1.18 MiB         | 63.84 MiB       | **+62.66 MiB**       |
| Release binary, unstripped              | 1.43 MiB         | 80.16 MiB       | +78.73 MiB           |
| Crates in lockfile                      | —                | 349             | workspace today: 352 |

**DataFusion's own resident footprint is +3.34 MiB** — the baseline difference with both arms sampled at the same point, after opening the store and before either does any work. That is the cleanest answer to "what does the engine cost in memory," and it is under a tenth of the 40 MiB carve-out. Building the `SessionContext` and registering the provider takes the arm to 17.44 MiB, though most of that step is the provider's granularity check decoding one bucket rather than DataFusion itself.

Peak RSS is sampled two ways and the larger is reported: at call boundaries, and continuously from a background thread every 5 ms across the measured section. The continuous sampler exists because a peak that rises and falls inside one query would otherwise be invisible, and 11.28 MiB of headroom cannot absorb one. For the DataFusion arm both methods report the same value — reassurance rather than proof, since a spike shorter than the 5 ms interval could still slip between samples, but nothing longer-lived is hiding.

Both arms reported `rss_failed_reads=0` and `rss_watch_complete=true`, so every peak above rests on a complete sampling trace. Those two fields exist because a review pass found that a total `sysinfo` failure would previously have printed `peak_rss_mib=0.00` — a spectacularly favourable number — with no signal at all. A failed read is now counted rather than folded in as a zero, and a watcher thread that dies before it is asked to stop reports its trace as incomplete.

Latency is reported as p50 and max. At ten samples the nearest-rank p95 is arithmetically the maximum, so quoting both would imply tail information the sample size does not carry.

## Binary size — the maintainer's call

`+62.66 MiB` stripped, a 54x increase over a binary that links `daemoneye-lib` and the fixture code but not DataFusion. Unstripped it is `+78.73 MiB`.

That delta is an over-estimate of what DataFusion costs `daemoneye-agent`. The control binary is synchronous and the DataFusion binary is `#[tokio::main]`, so the delta also carries the multi-threaded Tokio runtime — 1,228 Tokio symbols in the DataFusion binary against 6 in the control. `daemoneye-agent` already links Tokio, so the true marginal cost there is smaller than this figure by whatever the runtime contributes. The dependency graph roughly doubles: 349 crates for the spike against 352 for the entire existing six-crate workspace.

R8 sets no threshold deliberately, so this is not a pass or a fail. It is the price of not writing a join and cost engine, and it is the number to weigh against that.

## How much to trust these numbers

An adversarial review of the spike found that the original fixture planted every match inside the first hourly bucket: 780 ms between rows and 1,000 planted rows put the last one 0.22 hours in, while the measured query spanned all 26. The cost signal covered the whole fixture and the correctness signal covered one twenty-sixth of it, so a pruning or decode defect anywhere after the first hour would have moved every recorded number while leaving the match count at exactly 500. The numbers above come from a rebuilt fixture that plants a pair every `rows / planted` indices, so matches land in at least 20 of 26 buckets, and a test now fails if they cluster again.

Three other things were wrong before that review and are fixed here: an inclusive `<=` upper bound pruned one bucket too few, which would have silently dropped rows the re-filter could never recover; a pruned-to-nothing scan still planned one partition; and R18's cross-arm check ran only against a 6,000-row spec, so the "500 = 500" in this document was a human comparing two stdout lines. A test now enforces the equivalence at the measured 120,000-row scale.

The gate is worth no more than the fixture behind it, and this is the fixture it now rests on: 120,000 events, 26 hourly buckets, 500 matches distributed across them, 43 tests covering generation, both arms, pruning, and the measurement harness itself.

## Two findings that qualify the verdict

**DataFusion and Arrow are not FFI-free.** The Tech Plan's cross-cutting constraints say new deps must be "FFI-free where feasible (DataFusion/Arrow, `caps`, `windows-service`, `fuzzyhash`)". They are not. `zstd-sys` — which compiles C through a build script — reaches the normal build graph via `arrow-ipc` → `arrow` → `datafusion`, unconditionally. Building with `default-features = false` and a minimal feature set (`sql`, `recursive_protection`, `datetime_expressions`, `string_expressions`) removes `parquet`, `liblzma`, `bzip2`, and `flate2`, but no feature flag removes `zstd-sys`. Adopting DataFusion means accepting one C FFI dependency, and the Tech Plan's parenthetical should be corrected rather than left implying otherwise.

**The 481 MiB control-arm peak is a T3 read-path finding, not a DataFusion one.** `EventStore::scan_range` over the whole 26-hour window materializes all 120,000 `ProcessRecord`s; repeated full-range scans retain roughly 481 MiB resident. The DataFusion arm stays at 88.72 MiB over the same eleven queries purely because its provider reads one bucket per partition as that partition executes. **T6 must not read detection windows through a single whole-range `scan_range` call** — bucket-at-a-time streaming is what keeps RSS bounded, and this spike is the evidence.

## What this does not answer

- **Linux and Windows are unmeasured.** The dev host cannot build the Windows target at all ([GOTCHAS.md](../../GOTCHAS.md) §1.1). Binary size and RSS both move across targets.
- **The arm carries no MRC or index pushdown.** T6's real implementation would add both, so these numbers are a conservative floor on performance, not a ceiling.
- **Only one query shape was measured.** Aggregations and window functions are unmeasured.
- **Peak RSS does not generalize past 26 buckets.** The provider plans one partition per surviving bucket with no cap, so partition count tracks bucket count. This fixture holds 26; the event store's default seven-day hourly retention holds up to 168. `target_partitions` caps DataFusion's execution concurrency, not the scan's partition count, so it does not bound this. A T6 query over a full retention window is unmeasured and is the most likely place the 11.28 MiB headroom goes.
- **No bucket in the fixture exceeds one Arrow batch.** At ~4,615 rows per bucket against `batch_size = 8192`, every partition emits exactly one `RecordBatch`. Behavior once a bucket needs several batches per partition — higher event volume, or a coarser granularity — is unmeasured.
- **The control arm's 481 MiB is partly a repeat-count artifact.** It is the high-water mark across eleven identical whole-range scans, not the cost of one. The growth from 66 MiB after one query is consistent with allocator fragmentation across many small per-record allocations, not with a single call's working set. Bucket-at-a-time reads shrink each burst, but whether they bound a long-running agent's steady-state RSS needs a longer run than this spike's eleven calls.
- **Both arms read through the same `EventStore` calls.** A defect inside `scan_range` or `list_buckets` would make both arms agree while both were wrong, and nothing here would catch it. T3's own tests are the guard for that.

## Dependency audit

`cargo deny check --manifest-path spikes/datafusion-gate/Cargo.toml` reports **advisories ok, bans ok, licenses ok, sources ok** across all 349 crates, under the repo's existing `deny.toml` policy. Nothing in DataFusion's tree carries a known advisory or an unapproved license.

Two duplicate-version warnings surface (`winnow`, `toml`), both from `daemoneye-lib`'s own `figment` and `toml` dependencies rather than from DataFusion. The root `deny.toml` sets `multiple_crate_versions = "allow"`, so they are warnings, not failures.

This check does not run in CI for the spike: `cargo-deny` resolves from the root manifest and never reaches an excluded crate. It was run by hand for this decision, and T6 will pick the tree up under the workspace's own gate when DataFusion moves in.

## Downstream ticket impact

Under this GO verdict, **T6, T7, T10, and T12 are unchanged**. T6 builds Phase 2 on DataFusion as ADR-0006 specifies; T5's planner output retargets nothing; T10's ad-hoc query path and T12's integration scope follow T6 as planned.

Three things carry into T6 as inputs rather than changes:

- Read detection windows bucket-at-a-time, never with one whole-range `scan_range`.
- The spike reached the store through `EventStore`'s public API only — `bucket` and `codec` are private modules, `TsSeqKey` is `pub(super)`, and there is no accessor on the `redb::Database`. T6 is inside the workspace and will not have that constraint, but the public path was sufficient and is the lower-risk starting point.
- Pin `default-features = false` with the minimal feature set. The default set pulls `parquet` and three more C FFI compression crates for nothing.

## Reproducing

**The spike crate has been removed.** It did its job and requirement R12 called for its deletion, so `spikes/datafusion-gate/`, its `.gitignore` negation, the root `[workspace] exclude` entry, and its `just` recipes are gone.

The harness is preserved in history at commit `bfa6782` (the squash of PR #260). To re-run these measurements:

```bash
git checkout bfa6782 -- spikes/datafusion-gate
git show bfa6782:justfile | sed -n '/T4 · M3 DataFusion feasibility spike/,$p' >> justfile
# add `exclude = ["spikes"]` under [workspace] in the root Cargo.toml
just spike-datafusion-measure   # fixture, both arms, RSS, latency, binary size
just spike-datafusion-test      # 43 validation tests
```

The fixture is deterministic (pure index arithmetic, no randomness) and refuses to append to a non-empty store, so a rerun on the same hardware reproduces these numbers. No measurement counted unless both arms agreed (R18); that equivalence test is what enforced it.

## Compatibility facts worth keeping

- DataFusion 55.1.0 declares MSRV 1.94.0 and edition 2024 — inside the workspace's `rust-version = "1.95"` and the pinned 1.97.1 toolchain.
- `sqlparser` aligns exactly: the repo pins 0.62.0 and DataFusion requires `^0.62.0`, so no second copy enters the graph. The Phase 1 planner and Phase 2 executor share one parser.
- DataFusion 55.1 requires `arrow ^59.2`, while the current arrow release is 60.0. Take Arrow through `datafusion::arrow`; a direct `arrow` dependency produces two incompatible `arrow_schema::Schema` types.
- `object_store 0.13` is a non-optional DataFusion dependency and ships in the binary even though DaemonEye reads redb.
- DataFusion 55.1's `TableProvider` and `ExecutionPlan` have **no** `as_any` member (`Any` is a supertrait), and `ExecutionPlan` requires `apply_expressions`. The published custom-table-provider guide is ahead of 55.1 here.
- `EventStore` does not implement `Debug`, which both `TableProvider` and `ExecutionPlan` require. A hand-written `Debug` impl is needed.
