# T4 · M3 — DataFusion feasibility spike (gate)

**Milestone:** M3 (precursor) · **Backlog:** T7.6 gate

## Scope

**In:** Prototype one DataFusion `SessionContext` + one redb-backed `TableProvider` (over the T3 event store) + one representative derived-SQL query. Measure binary size, RSS, and per-rule latency.

**Out:** Full planner/executor; production wiring.

## Spec references

- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(Detection — DataFusion gate)
- requirements R3, R20; ADR-0006

## Key touchpoints

- New deps under evaluation: `datafusion` + `arrow` (FFI-free, but large — this spike exists to measure footprint).
- Prototype a single `SessionContext` + one redb-backed `TableProvider` over the T3 `processes.events` layout, running one representative derived-SQL query.
- Measure: release binary size delta (e.g., `cargo bloat`/artifact size), peak RSS, and per-rule latency (criterion + RSS sampling).
- Reference: Tech Plan "Detection — DataFusion gate"; design.md ADR-0006 + §11.5–11.7.

## Testing & quality gates

- Spike may live behind a feature flag / example; must still pass `cargo clippy -- -D warnings` and `cargo fmt --all --check`.
- Record measured numbers in the ticket/PR; produce an explicit written **go/no-go** decision artifact before T6.

## Dependencies

T3 (needs a real event-store table to read).

## Decision

**GO** — recorded 2026-09-19 in [docs/decisions/2026-09-19-t4-datafusion-gate.md](../../../docs/decisions/2026-09-19-t4-datafusion-gate.md). All three measured criteria pass; T6, T7, T10, and T12 are unchanged. Two qualifications are in that artifact: DataFusion/Arrow is not FFI-free (`zstd-sys` via `arrow-ipc`), and whole-range `scan_range` is a T3 read-path memory trap T6 must avoid.

**Ticket closed.** The spike crate was removed per R12 once the decision was recorded; the harness is preserved at commit `bfa6782` and the decision artifact explains how to restore it. The Tech Plan's detection section and T6's touchpoints now carry what the measurements bind.

## Acceptance criteria

- Recorded measurements show `<100MB RSS` and `<100ms/rule` are achievable; **go/no-go decision documented** before T6 proceeds. Binary size has no numeric threshold — record the release-binary delta and let the maintainer judge it explicitly in the decision artifact rather than implying a bar the repo does not define. If no-go, the fallback is the hand-rolled executor (see Tech Plan, no-go contingency).
