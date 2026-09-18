# T1 · M1 — Finish in-flight foundation (integrity hashing + crossbeam removal)

**Milestone:** M1 · **Backlog:** T3 (remaining), T2.7

**Status (2026-09-17):** Substantially shipped in PR #190 (`3fa2341`) and `d5a94ee`. Both workstreams landed. See **Remaining work** below for what is still open.

## Scope

**In:**

- Complete executable integrity hashing: criterion baselines for hashing impact on enumeration; verify missing/inaccessible executables are handled without failing enumeration; ensure hashing runs async outside the R1 enumeration deadline and reuses cached hashes (existing `MultiAlgorithmHasher` in file:daemoneye-lib/src/integrity/mod.rs).
- Add ssdeep fuzzy hash (`fuzzyhash`) alongside SHA-256 identity hash, plus on-disk-vs-running mismatch metadata (R2 AC6/AC7).
- Remove crossbeam `HighPerformanceEventBus` (file:collector-core/src/high_performance_event_bus.rs + dep) once the eventbus in-process path meets the R14 AC4 no-regression budget; in-process needs use plain tokio channels.

**Out:** New detection/storage logic; Prometheus metrics (M10).

## Spec references

- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md (Data Model: `ProcessRecord` additions)
- file:.kiro/specs/daemoneye-core-monitoring/requirements.md R2, R14

## Key touchpoints

- file:daemoneye-lib/src/integrity/mod.rs — `MultiAlgorithmHasher`, `HashAlgorithm`, `HasherConfig`, `HashResult` (already emits SHA-256 + BLAKE3). Do **not** add ssdeep as a `HashAlgorithm` variant: `HashResult.hashes` holds only cryptographically secure hashes (runtime `debug_assert!(is_cryptographically_secure)`, `hex_len() == 64`). Fuzzy binary-change verification is not a cryptographic-identity process, so compute the ssdeep/CTPH hash on a separate, explicitly non-cryptographic path (its own module/location) and carry it as a dedicated field; leave the `HashAlgorithm` enum and the `HashResult.hashes` invariant untouched.
- file:daemoneye-lib/src/models/process.rs — **superseded by the shipped design.** The original plan was to extend the native `ProcessRecord` with `ssdeep_hash: Option<String>` + a mismatch marker. As built, the integrity signals (`ssdeep_hash`, `on_disk_mismatch`, `ssdeep_degraded`) live **only on the protobuf record**, because they originate on the procmond `ProcessEvent` → proto path and the native model has nothing to lift them from. Both conversion directions in file:daemoneye-lib/src/proto.rs therefore drop them by design, and a tripwire test (`native_proto_conversion_drops_integrity_signals_by_design`) locks that asymmetry so a future edit cannot silently change the lossy boundary. Consumers that need the signals — e.g. the agent integrity-alert bridge — MUST read them off the proto record _before_ conversion. `executable_hash` remains the SHA-256 identity hash.
- file:daemoneye-lib/proto/common.proto — carries `ssdeep_hash = 15`, `on_disk_state = 16` (the `OnDiskState` enum), and `ssdeep_degraded = 17`; `executable_hash`/`hash_algorithm` semantics are unchanged. Field 16 was a `bool on_disk_mismatch` until 2026-09-18; see **Remaining work** item 1 for why it became three-state. `collector-core` keeps its own `OnDiskState` so the SDK event model stays independent of the wire contract, and `map_on_disk_state` in file:procmond/src/lib.rs is the single boundary where the two meet.
- file:procmond/src/hash_pass.rs, file:procmond/src/process_collector.rs, file:procmond/src/main.rs (shared `Arc<MultiAlgorithmHasher>` composition root) — wire ssdeep + async-out-of-deadline hashing.
- ~~file:collector-core/src/high_performance_event_bus.rs~~ — **done (`d5a94ee`).** The file, its `collector-core/src/lib.rs` re-exports, and the `crossbeam` dependency in both manifests are gone; in-process delivery runs on the daemoneye-eventbus broker. No dual-bus end state remains.
- Benches: file:daemoneye-lib/benches/integrity_operations.rs carries `bench_ssdeep_only` and the combined SHA-256+ssdeep benches at 1 KiB / 256 KiB / 4 MiB, both feature-gated on `fuzzy-hashes`. The broker end-to-end bench the R14 AC4 check needed was added as file:daemoneye-eventbus/benches/broker_inprocess_latency.rs — distinct from `file:procmond/benches/eventbus_benchmarks.rs` (`EventBusConnector` WAL/buffering), `daemoneye-eventbus/benches/throughput.rs` (publish-only), and `daemoneye-eventbus/benches/ipc_performance.rs` (socket). Criterion baselines are recorded through the CLI (`cargo bench --baseline previous`, state under `target/criterion/`), not stored in-source.
- New dep: `fuzzyhash` (pure-Rust ssdeep/CTPH, FFI-free, `unsafe_code="forbid"`-compatible), `default-features = false`.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings` and `cargo fmt --all --check` clean; no new `unwrap`/`panic`/`todo` in production paths; `unsafe_code = "forbid"` preserved.
- Criterion baselines recorded for hashing impact and for the crossbeam-removal no-regression check (R14 AC4) — the latter measured against the daemoneye-eventbus broker end-to-end path vs the recorded pre-migration baseline, not the `EventBusConnector` WAL bench. _Status: the hashing baselines and the broker benchmark exist; the pre-migration comparison number was never captured — see **Remaining work** item 3._
- New deps pass `cargo deny`/`cargo audit`; pinned per AGENTS.md dependency policy.

## Dependencies

No external ticket dependency (entry point). **One internal gate:** the crossbeam removal in this ticket's own scope was conditioned on the daemoneye-eventbus in-process broker path meeting R14 AC4 — a measurement that did not exist when the ticket was written.

_Resolved 2026-06-10._ The gate was discharged in `d5a94ee` on the grounds that `HighPerformanceEventBus` was export-only dead code with zero runtime consumers, so its deletion could not regress any live path. The broker end-to-end benchmark the gate called for was added at file:daemoneye-eventbus/benches/broker_inprocess_latency.rs (plan unit U10), distinct from the publish-only, socket, and WAL benches. A pre-migration comparison number was never captured — see **Remaining work** below, item 3.

## Acceptance criteria

- ssdeep + SHA-256 recorded; binary-change observation emitted when fuzzy-similarity to the previously recorded value falls below a configurable threshold. The threshold has a named default constant (e.g., `DEFAULT_SSDEEP_SIMILARITY_THRESHOLD`) and a `validate()` bound rejecting out-of-range values (mirroring `HasherConfig::validate()` in file:daemoneye-lib/src/integrity/mod.rs), so a misconfigured or zero threshold cannot silently disable the observation.
- Enumeration never fails on inaccessible executables; sustained CPU stays within R1 budget.
- Crossbeam dual-bus removal gated on R14 AC4: **if** the daemoneye-eventbus in-process broker path meets the R14 AC4 budget (alert latency < 100ms per rule, sustained CPU < 5%, no regression vs the recorded pre-migration baseline) **then** `HighPerformanceEventBus` is removed, in-process needs move to plain `tokio` channels, and the `crossbeam` dep is dropped; **else** the benchmark result is recorded and the removal is deferred to a follow-on ticket (the milestone item does not block on it). Because the bus is currently export-only (no runtime consumers), the gate is verified against the broker end-to-end path, not a measurement of the bus itself. Workspace builds with `-D warnings`.

## Remaining work

Recorded 2026-09-17 from a completeness pass over the shipped code.

1. **Three-state wire representation — done; macOS and Windows probes still open.** _Resolved 2026-09-18:_ `bool on_disk_mismatch = 16` became `OnDiskState on_disk_state = 16` (`UNKNOWN` / `MATCH` / `MISMATCH`, with `UNKNOWN = 0`), a deliberate wire break taken before v1.0 while no deployed consumer speaks the field. Three false negatives are closed: macOS and Windows now report `UNKNOWN` instead of a fabricated clean result, and **Linux itself was affected** — a failed `/proc/<pid>/exe` read (kernel thread, permission denied) previously reported "checked, clean" and now reports `UNKNOWN`. A successfully read link with no `(deleted)` suffix is now a positive `MATCH` finding, recorded explicitly, because silence is reserved for "not probed". Alerting fires on `MISMATCH` only, so unprobed platforms stay silent rather than flooding.

   _Still open:_ neither macOS nor Windows has an actual probe. They are honest (`UNKNOWN`) rather than wrong, but they detect nothing. Both need platform-specific mechanisms — likely `proc_pidpath` plus an inode comparison against the hashed fd on macOS, and an image-path/handle comparison on Windows. That is real design work, not a port of the Linux probe, and belongs in its own ticket.

2. **ssdeep is not covered by the hash cache.** `MultiAlgorithmHasher` caches SHA-256 results keyed on `(path, mtime, size, identity)`, but `compute_ssdeep_best_effort` in file:procmond/src/hash_pass.rs streams the whole file on every pass regardless of a cache hit. Unchanged binaries pay full fuzzy-hash cost each scan. Whether this matters against the R1/R2 AC4 CPU budget is a question for the U9 bench numbers, not a settled defect.

3. **U10 pre-migration baseline number was never captured.** Plan unit U10 asked for the broker end-to-end number to be recorded so the U11 gate had something concrete to compare against. `d5a94ee` substituted a dead-code argument instead. The benchmark exists and can be run; the historical pre-migration figure is gone. Note that hashing baselines (U9) are _deliberately_ not stored in-source — criterion records them under `target/criterion/` via `cargo bench --baseline previous` — so their absence from the repo is by design, not a gap.

4. **BACKLOG checkboxes are stale.** file:BACKLOG.md line 52 (`T2.7`, crossbeam removal) and the `T3 (remaining)` entry above it are both still unchecked despite the work having shipped.

**Shipped beyond the acceptance criteria:** `ssdeep_degraded` and the degraded-coverage alert landed in PR #190 but appear in none of this ticket's ACs. They distinguish "fuzzy hash attempted and failed while SHA-256 succeeded" (a real coverage degradation worth surfacing) from "`fuzzy-hashes` feature compiled out" (not a degradation), so a build without ssdeep does not flood operators with signals.

## Deferred / Open Questions

### From 2026-06-10 review

- **Split bundled ticket into independent hashing / crossbeam-removal units** — Scope / Dependencies (P2, scope-guardian, confidence 100)

  The integrity-hashing workstream and the crossbeam-removal workstream share zero files, no data-model changes, and no call-site dependency on each other. Bundling them under one acceptance-criteria block means either half blocking (e.g., the eventbus in-process path failing its R14 AC4 gate) holds the completed hashing work hostage in the same ticket. Consider splitting into T1a (hashing: ssdeep, async, cache, R2 AC2–AC7) and T1b (crossbeam removal, carrying the conditional R14 AC4 gate as an explicit dependency), or keep bundled if the milestone framing is deliberate.

  **Moot (2026-09-17).** Both halves landed, so the hostage risk never materialised. The plan sequenced them as Phase A / Phase B precisely so either could land or defer independently, which addressed the concern without a ticket split.

- **On-disk-vs-running mismatch probe mechanism undefined** — Scope / Key touchpoints (P2, security-lens, confidence 75)

  R2 AC6 defines the concept (the hash attests on-disk state, not the executing image; mismatch arises from a deleted or replaced executable) but the ticket adds a "marker" field without specifying what comparison sets it — e.g., a `/proc/<pid>/exe` trailing "(deleted)", an inode divergence between the hash-time fd and the running image, or a symlink retarget. Without a defined probe and set-conditions, an implementer may emit false negatives (deleted-then-replaced executables) or false positives (files in transient states such as package upgrades), producing unreliable forensic data. The mechanism and threat model should land in the spec/ticket before implementation.

  **Partially resolved (2026-09-17) — still open for macOS and Windows.** Linux has a concrete probe in file:procmond/src/linux_collector.rs. Neither other Primary-tier platform sets the flag at all, and the bare-`bool` wire representation makes that silence indistinguishable from a genuine "no mismatch" — which is the exact false-negative class this finding predicted, now confirmed rather than hypothetical. Carried forward as **Remaining work** item 1.

- **"Dependencies: None" understates the internal R14 AC4 gate** — Dependencies (P2, scope-guardian, feasibility, confidence 75)

  The crossbeam removal within this ticket's own scope is conditioned on the daemoneye-eventbus in-process transport meeting R14 AC4 — a benchmark that must be verified before the removal can proceed, and which depends on a broker in-process transport + comparative benchmark that may not yet exist. Declaring "Dependencies: None (entry point)" hides this internal gate from any scheduler or progress tracker reading only the header. State the gate explicitly (or split the ticket so T1b carries the dependency).

  **Resolved (2026-09-17).** The **Dependencies** section now states the gate and how it was discharged. The finding was correct on both counts: the comparative benchmark did not exist when the ticket was written, and it had to be built (plan unit U10) before the removal could be justified.
