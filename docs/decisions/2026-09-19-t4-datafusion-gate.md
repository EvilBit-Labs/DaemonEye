# T4 · M3 — DataFusion feasibility gate: **GO**

**Date:** 2026-09-19 · **Ticket:** [T4 · M3](<../../spec/full/tickets/T4_%C2%B7_M3_%E2%80%94_DataFusion_feasibility_spike_(gate).md>) · **Governs:** ADR-0006, T6

DataFusion clears all three measured criteria. T6 proceeds on DataFusion; the hand-rolled executor contingency is not invoked. Two findings below qualify that verdict and one of them changes a stated constraint.

## Verdict against the gate

| Criterion                             | Threshold           | Measured                          | Result                       |
| ------------------------------------- | ------------------- | --------------------------------- | ---------------------------- |
| Absolute peak RSS                     | `< 100 MiB`         | **88.67 MiB**                     | pass, 11.33 MiB headroom     |
| Marginal RSS over the control ceiling | `<= 40 MiB`         | **−392.33 MiB**                   | pass                         |
| Per-rule latency                      | `< 100 ms`          | **9.79 ms** p50, **10.57 ms** p95 | pass, ~10× headroom          |
| Cross-arm equivalence (R18)           | both arms identical | **500 = 500**                     | pass                         |
| Release binary size delta             | no threshold        | **+62.68 MiB** stripped (55.0×)   | maintainer's judgment, below |

Every number was measured on **macos/aarch64**, release profile with `overflow-checks = true`, `lto = "thin"`, `codegen-units = 1` — the same profile DaemonEye ships. Linux and Windows are unmeasured.

## What was measured

A disposable crate at `spikes/datafusion-gate/`, excluded from the workspace with its own lockfile. It generates a real T3 event store — **120,000 process events across 26 hourly `processes.events@<id>` partitions**, deterministic from a fixed seed — and runs one representative detection query two ways:

- **DataFusion arm** — `SessionContext` (`target_partitions = 4`, `batch_size = 8192`) over a redb-backed `TableProvider` that maps one bucket to one partition and prunes on a `ts_ms` range predicate. It answers the join with DataFusion's own hash join and does not consult the MRC parent map.
- **Control arm** — the same partitions decoded into memory through `EventStore::scan_range` with no engine on top. It is the materialization ceiling, the latency floor, the reference answer, and the binary-size baseline.

The query is a ShadowHunt parent/child lineage self-join: a shell process whose parent is a long-running service, bounded by a time predicate.

### Full measurements

|                                             | control arm      | DataFusion arm  | delta                |
| ------------------------------------------- | ---------------- | --------------- | -------------------- |
| Baseline RSS (open/registered, nothing run) | 6.52 MiB         | 17.31 MiB       | **+10.79 MiB**       |
| RSS after one query                         | 66.28 MiB        | 79.73 MiB       | **+13.45 MiB**       |
| Peak RSS over 11 queries                    | 481.00 MiB       | 88.67 MiB       | **−392.33 MiB**      |
| Latency p50 / p95                           | 24.48 / 24.89 ms | 9.79 / 10.57 ms | **2.5× faster**      |
| Decode-only pass                            | 23.48 ms         | —               | —                    |
| Release binary, stripped                    | 1.16 MiB         | 63.84 MiB       | **+62.68 MiB**       |
| Release binary, unstripped                  | 1.40 MiB         | 80.16 MiB       | +78.75 MiB           |
| Crates in lockfile                          | —                | 349             | workspace today: 352 |

**DataFusion's own resident footprint is ~10.8 MiB** — the baseline difference, before either arm touches data. That is the cleanest answer to "what does the engine cost in memory," and it is a quarter of the 40 MiB carve-out.

## Binary size — the maintainer's call

`+62.68 MiB` stripped, a 55× increase over a binary that links `daemoneye-lib` and the fixture code but not DataFusion. Unstripped it is `+78.75 MiB`. The dependency graph roughly doubles: 349 crates for the spike against 352 for the entire existing six-crate workspace.

R8 sets no threshold deliberately, so this is not a pass or a fail. It is the price of not writing a join and cost engine, and it is the number to weigh against that.

## Two findings that qualify the verdict

**DataFusion and Arrow are not FFI-free.** The Tech Plan's cross-cutting constraints say new deps must be "FFI-free where feasible (DataFusion/Arrow, `caps`, `windows-service`, `fuzzyhash`)". They are not. `zstd-sys` — which compiles C through a build script — reaches the normal build graph via `arrow-ipc` → `arrow` → `datafusion`, unconditionally. Building with `default-features = false` and a minimal feature set (`sql`, `recursive_protection`, `datetime_expressions`, `string_expressions`) removes `parquet`, `liblzma`, `bzip2`, and `flate2`, but no feature flag removes `zstd-sys`. Adopting DataFusion means accepting one C FFI dependency, and the Tech Plan's parenthetical should be corrected rather than left implying otherwise.

**The 481 MiB control-arm peak is a T3 read-path finding, not a DataFusion one.** `EventStore::scan_range` over the whole 26-hour window materializes all 120,000 `ProcessRecord`s; repeated full-range scans retain roughly 481 MiB resident. The DataFusion arm stays at 88.67 MiB over the same eleven queries purely because its provider reads one bucket per partition as that partition executes. **T6 must not read detection windows through a single whole-range `scan_range` call** — bucket-at-a-time streaming is what keeps RSS bounded, and this spike is the evidence.

## What this does not answer

- **Linux and Windows are unmeasured.** The dev host cannot build the Windows target at all ([GOTCHAS.md](../../GOTCHAS.md) §1.1). Binary size and RSS both move across targets.
- **The arm carries no MRC or index pushdown.** T6's real implementation would add both, so these numbers are a conservative floor on performance, not a ceiling.
- **Only one query shape was measured.** Aggregations and window functions are unmeasured.

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

```bash
just spike-datafusion-measure   # fixture, both arms, all numbers
just spike-datafusion-test      # 24 validation tests
just spike-datafusion-lint      # fmt + clippy -D warnings
```

The fixture is deterministic from a fixed seed and refuses to append to a non-empty store, so a rerun reproduces these numbers. No measurement counts unless the arms agree (R18); the equivalence test is what enforces that.

## Compatibility facts worth keeping

- DataFusion 55.1.0 declares MSRV 1.94.0 and edition 2024 — inside the workspace's `rust-version = "1.95"` and the pinned 1.97.1 toolchain.
- `sqlparser` aligns exactly: the repo pins 0.62.0 and DataFusion requires `^0.62.0`, so no second copy enters the graph. The Phase 1 planner and Phase 2 executor share one parser.
- DataFusion 55.1 requires `arrow ^59.2`, while the current arrow release is 60.0. Take Arrow through `datafusion::arrow`; a direct `arrow` dependency produces two incompatible `arrow_schema::Schema` types.
- `object_store 0.13` is a non-optional DataFusion dependency and ships in the binary even though DaemonEye reads redb.
- DataFusion 55.1's `TableProvider` and `ExecutionPlan` have **no** `as_any` member (`Any` is a supertrait), and `ExecutionPlan` requires `apply_expressions`. The published custom-table-provider guide is ahead of 55.1 here.
- `EventStore` does not implement `Debug`, which both `TableProvider` and `ExecutionPlan` require. A hand-written `Debug` impl is needed.
