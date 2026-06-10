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

- file:daemoneye-lib/src/integrity/mod.rs — `MultiAlgorithmHasher`, `HashAlgorithm`, `HasherConfig`, `HashResult` (already emits SHA-256 + BLAKE3; add ssdeep).
- file:daemoneye-lib/src/models/process.rs — extend `ProcessRecord` with `ssdeep_hash: Option<String>` + on-disk-vs-running mismatch marker; keep `executable_hash` as SHA-256 identity.
- file:procmond/src/hash_pass.rs, file:procmond/src/process_collector.rs, file:procmond/src/main.rs (shared `Arc<MultiAlgorithmHasher>` composition root) — wire ssdeep + async-out-of-deadline hashing.
- file:collector-core/src/high_performance_event_bus.rs — remove; replace in-process needs with plain `tokio` channels; drop the `crossbeam` dep from file:Cargo.toml.
- Benches: file:daemoneye-lib/benches/integrity_operations.rs, file:procmond/benches/eventbus_benchmarks.rs (baselines).
- New dep: `fuzzyhash` (pure-Rust ssdeep/CTPH, FFI-free, `unsafe_code="forbid"`-compatible), `default-features = false`.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings` and `cargo fmt --all --check` clean; no new `unwrap`/`panic`/`todo` in production paths; `unsafe_code = "forbid"` preserved.
- Criterion baselines recorded for hashing impact and for the crossbeam-removal no-regression check (R14 AC4) vs the pre-removal baseline.
- New deps pass `cargo deny`/`cargo audit`; pinned per AGENTS.md dependency policy.

## Dependencies

None (entry point).

## Acceptance criteria

- ssdeep + SHA-256 recorded; binary-change observation emitted below configurable similarity threshold.
- Enumeration never fails on inaccessible executables; sustained CPU stays within R1 budget.
- Crossbeam dual-bus removed with a recorded no-regression benchmark vs the pre-removal baseline; workspace builds with `-D warnings`.
