# T6 · M3 — Detection Phase 2: DataFusion execution, degradation, agent integration, metrics & docs

**Milestone:** M3 · **Backlog:** T7.5, T7.6, T7.7, T7.8, T7.9, T7.10

## Scope

**In:**

- Per-collector redb `TableProvider`s with filter/projection pushdown into scans/indexes; locked-down `SessionContext` (function allowlist, memory pool, cardinality caps; aggregations require time windows); derived-SQL only.
- Detection storage wiring (T7.5) on the T3 layout.
- Degradation/completeness (R20/R21): `Completeness { status, reasons[] }`, collector-disconnect handling, `(task_id, seq_no)` replay dedup + grace period, shed counting, seq-no recovery.
- Agent integration: extract `DetectionEngine` trait, replace placeholder in file:daemoneye-lib/src/detection/mod.rs, wire `ResilientIpcClient` + `BrokerManager` capability negotiation into the agent loop.
- Detection metrics via tracing + on-demand execution plans (T7.9); operator rule-authoring docs incl. dialect/extension policy and CLI rule testing (T7.10).

**Out:** Prometheus exporter (M10); AUTO JOIN/specialty/reactive (post-v1.0).

## Spec references

- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Execution/Storage, Degradation, CompletenessTracker)
- design.md §11.5–11.7; requirements R3, R10, R17, R20, R21

## Key touchpoints

- `datafusion`/`arrow` (adopted only if T4 gate passes): per-collector redb `TableProvider`s with filter/projection pushdown into base scans + multimap secondary indexes; `SessionContext` restricted to the approved function allowlist, bounded memory pool, cardinality caps; aggregations require explicit time windows; derived-SQL only.
- file:daemoneye-lib/src/detection/mod.rs — extract a `DetectionEngine` trait; remove the substring/category placeholder `execute_rule`.
- file:daemoneye-agent/src/main.rs (detection loop), file:daemoneye-agent/src/broker_manager.rs — wire `ResilientIpcClient` (file:daemoneye-lib/src/ipc/client.rs) + capability negotiation; load persisted rules from storage (T3).
- file:daemoneye-lib/src/models/alert.rs — add persisted `Completeness { status, reasons[] }`; `CompletenessTracker` threads executor/collector signals.
- Metrics via `tracing` (no Prometheus crate here); on-demand `EXPLAIN` plan output. Docs: dialect/extension policy + CLI rule-testing guidance.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean.
- Integration tests: real SQL detections over event store; event-to-alert latency ≤ one scan interval + 100ms/rule; degraded-vs-no-match completeness assertions; `(task_id, seq_no)` dedup + late-event grace; collector-disconnect degradation.
- Confirm `<100MB RSS` budget holds with DataFusion enabled.

## Dependencies

T4 (gate passed), T5.

## Acceptance criteria

- Real SQL detections execute per cycle over the event store; event-to-alert latency ≤ one scan interval + 100ms/rule.
- Alerts and evaluations carry completeness markers distinguishing "no match" from "could not fully evaluate"; degraded conditions surfaced, not silent.
- Placeholder substring matcher fully removed; agent loads and runs persisted rules.
