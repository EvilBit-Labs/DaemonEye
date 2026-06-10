# T5 · M3 — Detection Phase 1: SQL planner, regex, schema catalog, pushdown

**Milestone:** M3 · **Backlog:** T7.1, T7.2, T7.3, T7.4, T12.4

## Scope

**In:**

- Grow file:daemoneye-lib/src/detection/sql_to_ipc.rs into a validating planner: sqlparser AST validation (SELECT-only, function allowlist, subquery depth ≤ 3), typed `CollectionRequirements`.
- Regex compile/cache: linear-time `regex` with size limits, AST-before-compile, full-string-keyed LRU, latency flag/disable.
- Static schema catalog (R19): authenticated startup registration (peer creds/spawn token), table-reference validation, re-validate/re-plan on descriptor change with unhealthy-rule surfacing.
- Pushdown planner (R18): capability-based protobuf `DetectionTask`s, conformance vectors (unverified ops run agent-side), TTL renewal + re-issue on collector re-registration.
- Collector-side counterpart (T12.4): `EventSource` accepts `DetectionTask`, capability/schema advertisement, task validation.

**Out:** DataFusion execution (T6); load-time-only here, no derived-SQL run.

## Spec references

- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(Detection planner, Schema Catalog, Pushdown, Regex)
- requirements R3, R17, R18, R19

## Key touchpoints

- file:daemoneye-lib/src/detection/sql_to_ipc.rs — grow `SqlToIpcTranslator` into the validating planner; migrate `CollectionRequirements` to typed form; enforce SELECT-only + function allowlist + subquery depth (default 3).
- file:daemoneye-lib/src/detection/mod.rs — `DetectionEngine` load path / `load_rule` validation hook.
- Regex cache: `regex` crate (linear-time) with `RegexBuilder::size_limit`/`dfa_size_limit`; full-pattern-string-keyed LRU (`quick_cache` already present, or `lru`); AST-validate before compile.
- Schema catalog (R19): authenticated startup registration via file:collector-core/src/capability_router.rs / file:collector-core/src/rpc_services.rs + peer-cred/spawn-token check; table-ref validation; re-plan + unhealthy-rule surfacing.
- Pushdown: emit protobuf `DetectionTask`s (file:daemoneye-lib/proto/ipc.proto) with conformance gating + TTL renewal.
- Collector-side (T12.4): file:collector-core/src/source.rs (`EventSource` accepts `DetectionTask`), procmond file:procmond/src/event_source.rs.
- Deps present: `sqlparser`, `regex`.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean.
- Unit + property tests (`proptest`) for SQL validation/rejection, depth/regex bounds, pushdown lowering, and conformance fallback; SQL-injection vectors rejected and audited.
- This ticket is load-time only — no derived-SQL execution here.

## Dependencies

T2, T3.

## Acceptance criteria

- Invalid/forbidden SQL rejected at load with actionable, audited errors; depth/regex bounds enforced.
- Eligible predicates/projections lowered to `DetectionTask`s; unverified ops fall back agent-side; rules referencing absent tables marked unhealthy.
