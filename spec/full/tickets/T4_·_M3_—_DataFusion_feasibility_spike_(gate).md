# T4 · M3 — DataFusion feasibility spike (gate)

**Milestone:** M3 (precursor) · **Backlog:** T7.6 gate

## Scope

**In:** Prototype one DataFusion `SessionContext` + one redb-backed `TableProvider` (over the T3 event store) + one representative derived-SQL query. Measure binary size, RSS, and per-rule latency.

**Out:** Full planner/executor; production wiring.

## Spec references

- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Detection — DataFusion gate)
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

## Acceptance criteria

- Recorded measurements show binary-size, `<100MB RSS`, and `<100ms/rule` are achievable; **go/no-go decision documented** before T6 proceeds. If no-go, surface alternatives for re-planning.
