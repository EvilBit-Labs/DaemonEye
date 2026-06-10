# T1 · M1 — Finish in-flight foundation (integrity hashing + crossbeam removal)

**Milestone:** M1 · **Backlog:** T3 (remaining), T2.7

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
- file:daemoneye-lib/src/models/process.rs — extend `ProcessRecord` with `ssdeep_hash: Option<String>` + on-disk-vs-running mismatch marker; keep `executable_hash` as SHA-256 identity.
- file:daemoneye-lib/proto/common.proto — add optional fields to carry the ssdeep hash and the on-disk-vs-running mismatch marker across the procmond→agent IPC boundary (the contract currently exposes only a single `executable_hash`). Assign fresh field numbers and preserve existing `executable_hash`/`hash_algorithm` semantics, or ssdeep never crosses the process boundary.
- file:procmond/src/hash_pass.rs, file:procmond/src/process_collector.rs, file:procmond/src/main.rs (shared `Arc<MultiAlgorithmHasher>` composition root) — wire ssdeep + async-out-of-deadline hashing.
- file:collector-core/src/high_performance_event_bus.rs — remove **once the R14 AC4 gate is met** (see Scope and Acceptance criteria); replace in-process needs with plain `tokio` channels; drop the `crossbeam` dep from file:Cargo.toml. Note: this bus is currently export-only (re-exported from `collector-core/src/lib.rs`, no runtime consumers).
- Benches: file:daemoneye-lib/benches/integrity_operations.rs (hashing). For the crossbeam gate, note `file:procmond/benches/eventbus_benchmarks.rs` measures the `EventBusConnector` (WAL/buffering), **not** the broker path — the R14 AC4 no-regression check needs a broker end-to-end benchmark, so add or identify one rather than reusing the WAL bench.
- New dep: `fuzzyhash` (pure-Rust ssdeep/CTPH, FFI-free, `unsafe_code="forbid"`-compatible), `default-features = false`.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings` and `cargo fmt --all --check` clean; no new `unwrap`/`panic`/`todo` in production paths; `unsafe_code = "forbid"` preserved.
- Criterion baselines recorded for hashing impact and for the crossbeam-removal no-regression check (R14 AC4) — the latter measured against the daemoneye-eventbus broker end-to-end path vs the recorded pre-migration baseline, not the `EventBusConnector` WAL bench.
- New deps pass `cargo deny`/`cargo audit`; pinned per AGENTS.md dependency policy.

## Dependencies

None (entry point).

## Acceptance criteria

- ssdeep + SHA-256 recorded; binary-change observation emitted when fuzzy-similarity to the previously recorded value falls below a configurable threshold. The threshold has a named default constant (e.g., `DEFAULT_SSDEEP_SIMILARITY_THRESHOLD`) and a `validate()` bound rejecting out-of-range values (mirroring `HasherConfig::validate()` in file:daemoneye-lib/src/integrity/mod.rs), so a misconfigured or zero threshold cannot silently disable the observation.
- Enumeration never fails on inaccessible executables; sustained CPU stays within R1 budget.
- Crossbeam dual-bus removal gated on R14 AC4: **if** the daemoneye-eventbus in-process broker path meets the R14 AC4 budget (alert latency < 100ms per rule, sustained CPU < 5%, no regression vs the recorded pre-migration baseline) **then** `HighPerformanceEventBus` is removed, in-process needs move to plain `tokio` channels, and the `crossbeam` dep is dropped; **else** the benchmark result is recorded and the removal is deferred to a follow-on ticket (the milestone item does not block on it). Because the bus is currently export-only (no runtime consumers), the gate is verified against the broker end-to-end path, not a measurement of the bus itself. Workspace builds with `-D warnings`.

## Deferred / Open Questions

### From 2026-06-10 review

- **Split bundled ticket into independent hashing / crossbeam-removal units** — Scope / Dependencies (P2, scope-guardian, confidence 100)

  The integrity-hashing workstream and the crossbeam-removal workstream share zero files, no data-model changes, and no call-site dependency on each other. Bundling them under one acceptance-criteria block means either half blocking (e.g., the eventbus in-process path failing its R14 AC4 gate) holds the completed hashing work hostage in the same ticket. Consider splitting into T1a (hashing: ssdeep, async, cache, R2 AC2–AC7) and T1b (crossbeam removal, carrying the conditional R14 AC4 gate as an explicit dependency), or keep bundled if the milestone framing is deliberate.

- **On-disk-vs-running mismatch probe mechanism undefined** — Scope / Key touchpoints (P2, security-lens, confidence 75)

  R2 AC6 defines the concept (the hash attests on-disk state, not the executing image; mismatch arises from a deleted or replaced executable) but the ticket adds a "marker" field without specifying what comparison sets it — e.g., a `/proc/<pid>/exe` trailing "(deleted)", an inode divergence between the hash-time fd and the running image, or a symlink retarget. Without a defined probe and set-conditions, an implementer may emit false negatives (deleted-then-replaced executables) or false positives (files in transient states such as package upgrades), producing unreliable forensic data. The mechanism and threat model should land in the spec/ticket before implementation.

- **"Dependencies: None" understates the internal R14 AC4 gate** — Dependencies (P2, scope-guardian, feasibility, confidence 75)

  The crossbeam removal within this ticket's own scope is conditioned on the daemoneye-eventbus in-process transport meeting R14 AC4 — a benchmark that must be verified before the removal can proceed, and which depends on a broker in-process transport + comparative benchmark that may not yet exist. Declaring "Dependencies: None (entry point)" hides this internal gate from any scheduler or progress tracker reading only the header. State the gate explicitly (or split the ticket so T1b carries the dependency).
