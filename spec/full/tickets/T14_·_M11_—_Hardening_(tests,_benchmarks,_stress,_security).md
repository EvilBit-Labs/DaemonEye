# T14 · M11 — Hardening (tests, benchmarks, stress, security)

**Milestone:** M11 · **Backlog:** T22.1–22.4, T20.1–20.8, T23.1–23.6, T24 (#61, #62)

## Scope

**In:**

- Unit coverage >85% (llvm-cov); integration + CLI snapshot tests; property tests (proptest) on critical paths.
- CI matrix & quality gates (OS/arch/Rust versions, fmt/clippy/audit/deny, SLSA provenance, signed release pipeline).
- Criterion benchmark suite (process enum, hashing, IPC, DB, alerting, SQL/detection, collector-core) + regression detection.
- Stress/load tests (collector-core, enumeration, DB, alert delivery, IPC, system-wide).
- Advanced security testing: SQL-injection vectors, privilege-boundary verification, cargo-fuzz, Miri/ASan, IPC pen-testing, audit-chain tampering detection.

**Out:** Feature work (this is verification/hardening).

## Spec references

- requirements (all-verification); performance budgets in Tech Plan cross-cutting constraints

## Key touchpoints

- Coverage via `cargo llvm-cov` (>85%); runner `cargo-nextest`; property tests with `proptest`; snapshots with `insta`.
- Criterion benches across crates: file:daemoneye-lib/benches/, file:procmond/benches/, file:collector-core/benches/, file:daemoneye-eventbus/benches/ (process enum, hashing, IPC, DB, alerting, SQL/detection, collector-core) + regression detection.
- Stress/load tests (collector-core, enumeration, DB, alert delivery, IPC, system-wide); fuzz via `cargo-fuzz`; memory safety via Miri/ASan for any boundary code.
- CI matrix & gates: file:.github/workflows/ci.yml (Linux/macOS/Windows × x86_64/ARM64 × stable/beta/MSRV 1.95), fmt/clippy/`cargo audit`/`cargo deny`, SLSA provenance, signed release pipeline.
- Security suites: SQL-injection vectors, privilege-boundary verification, IPC pen-testing, audit-chain tampering detection.

## Testing & quality gates

- All gates green in CI across the support matrix; criterion baselines stored for regression comparison.
- Performance budgets verified: `<5% CPU`, `<100MB RSS`, `<100ms/rule`, `>1,000` DB writes/sec, `<5s` enumeration for 10k+ processes.

## Dependencies

T12 (benchmarks/stress target integrated code; T20.1/T20.3 may start earlier against existing code).

## Acceptance criteria

- Coverage, benchmark-regression, and security gates pass in CI across the support matrix; performance budgets (\<5% CPU, \<100MB RSS, \<100ms/rule, >1k writes/sec) verified.
