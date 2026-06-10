# T12 · M9 — End-to-end integration

**Milestone:** M9 · **Backlog:** T25.1–T25.4 (#63)

## Scope

**In:**

- Wire IPC across components via collector-core (task distribution/result collection through runtime, preserve protobuf+CRC32).
- Connect SQL→task planning + collector event sources → detection → alert generation → multi-channel delivery, end to end.
- Unified configuration and service management across all three components.
- Full-pipeline e2e tests (collect → persist → detect → alert → deliver → query → audit).

**Out:** New feature logic (consumes T2–T11).

## Spec references

- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (End-to-end data flow)
- requirements (all-integration)

## Key touchpoints

- Wire IPC across components via the collector-core runtime (file:collector-core/src/collector.rs, file:collector-core/src/ipc.rs), preserving protobuf + CRC32 framing; agent embeds the eventbus broker.
- Connect SQL→task planning (T5) + collector event sources → detection (T6) → alert generation → multi-channel delivery (T7); persisted alerts/deliveries via T3.
- Unified configuration + service management across procmond / daemoneye-agent / daemoneye-cli (T9, T10).
- E2E suites under each crate's `tests/` plus workspace-level scenarios.
- This ticket integrates existing pieces; avoid introducing new feature logic.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check`, `cargo test --workspace` green.
- E2E test runs the full v1.0 success path: collect → persist → detect → alert → deliver → query → audit, across Linux/macOS/Windows in CI; completeness markers propagate end-to-end.

## Dependencies

T7, T9, T10, T11.

## Acceptance criteria

- A clean host runs the full v1.0 success-condition path end to end with passing e2e tests across Linux/macOS/Windows.
