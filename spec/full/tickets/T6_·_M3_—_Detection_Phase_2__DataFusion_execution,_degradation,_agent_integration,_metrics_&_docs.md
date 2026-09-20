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

- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(Execution/Storage, Degradation, CompletenessTracker)
- design.md §11.5–11.7; requirements R3, R10, R17, R20, R21

## Key touchpoints

- `datafusion`/`arrow` (**adopted — T4 gate passed GO, 2026-09-19**, see [decision](../../../docs/decisions/2026-09-19-t4-datafusion-gate.md)): per-collector redb `TableProvider`s with filter/projection pushdown into base scans + multimap secondary indexes; `SessionContext` restricted to the approved function allowlist, bounded memory pool, cardinality caps; aggregations require explicit time windows; derived-SQL only.
- **Carried from the T4 measurements** (each is a measured result, not a preference):
  - Read detection windows **bucket-at-a-time** ([ADR-0008](../../../docs/adr/0008-bucket-at-a-time-detection-reads.md)). A single whole-range `EventStore::scan_range` retained ~481 MiB over 120,000 records; the partitioned provider stayed under 89 MiB on the same workload. This is the largest single memory lever T6 has.
  - Pin `datafusion` with `default-features = false`. The default set pulls `parquet` plus `liblzma`/`bzip2`/`flate2`. `zstd-sys` arrives unconditionally via `arrow-ipc` regardless, so the dependency is not FFI-free.
  - Take Arrow through `datafusion::arrow`; a direct `arrow` dependency resolves to a different major and yields two incompatible `arrow_schema::Schema` types.
  - `sqlparser` needs no second copy: DataFusion 55.1 requires `^0.62.0` and the workspace already pins 0.62.0.
  - Partition count tracks bucket count with no cap in the spike's provider. At seven-day hourly retention that is up to 168 partitions against the 26 measured, and `target_partitions` does not bound it — cap it or measure it before trusting the RSS headroom.
  - Run `cargo deny` against the new tree once it enters the workspace. The spike's 349-crate graph passed advisories, bans, licenses, and sources, but it was checked by hand outside CI.
- file:daemoneye-lib/src/detection/mod.rs — extract a `DetectionEngine` trait; remove the substring/category placeholder `execute_rule`.
- file:daemoneye-agent/src/main.rs (detection loop), file:daemoneye-agent/src/broker_manager/ — wire `ResilientIpcClient` (file:daemoneye-lib/src/ipc/client.rs) + capability negotiation; load persisted rules from storage (T3).
- file:daemoneye-lib/src/models/alert.rs — add persisted `Completeness { status, reasons[] }`; `CompletenessTracker` threads executor/collector signals.
- Metrics via `tracing` (no Prometheus crate here); on-demand `EXPLAIN` plan output. Docs: dialect/extension policy + CLI rule-testing guidance.
- **T3 storage handoff:** T3 ships the event store with index-scan primitives shaped for caching and an in-memory MRC parent map, but defers the §11.7.4 posting-list page LRU — build it as the first storage-side task here, then layer pushdown intersection on top. See the T3 brainstorm: [storage-engine event store](../../../docs/brainstorms/2026-06-13-storage-engine-event-store-requirements.md).

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
