# T2 · M2 — IPC backbone (CollectorIpcServer + IpcClientManager)

**Milestone:** M2 · **Backlog:** T15.1 (#78), T15.2 (#77)

## Scope

**In:**

- Implement `CollectorIpcServer` in file:collector-core/src/ipc.rs: capability negotiation, task routing, connection management.
- Complete `IpcClientManager` in daemoneye-agent: reconnection, capability negotiation, task distribution/result collection; compatible with procmond `ProcessMessageHandler`.
- Integration tests for both directions.

**Out:** SQL→task generation (T5); DataFusion execution (T6).

## Spec references

- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Component Architecture: eventbus/IPC interfaces)
- requirements R3.1–3.2, R11.1–11.2, R12.1–12.2

## Key touchpoints

- file:collector-core/src/ipc.rs — implement `CollectorIpcServer` (capability negotiation, task routing, connection mgmt); see file:collector-core/src/capability_router.rs, file:collector-core/src/rpc_services.rs.
- file:daemoneye-agent/src/ipc_server.rs, file:daemoneye-agent/src/broker_manager.rs, file:daemoneye-agent/src/collector_registry.rs — complete `IpcClientManager` (reconnection w/ backoff, capability negotiation, task distribution/result collection).
- file:daemoneye-lib/src/ipc/ — `codec.rs`, `client.rs` (`ResilientIpcClient`), `interprocess_transport.rs` (protobuf + CRC32 framing; honor 107-byte Unix socket path limit).
- file:daemoneye-lib/proto/ipc.proto — `DetectionTask`/`DetectionResult`, capability messages.
- procmond compatibility: file:procmond/src/rpc_service.rs (`ProcessMessageHandler` path).
- Tests: file:collector-core/tests/ipc_integration.rs, file:daemoneye-lib/tests/ipc_integration.rs.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; `await_holding_lock` respected (clone owned `Arc` before await).
- Integration tests cover capability negotiation, task/result round-trip, and reconnection; `NO_COLOR=1 TERM=dumb` for any snapshot output.
- No regression to procmond actor/standalone modes (`DAEMONEYE_BROKER_SOCKET` present/absent).

## Dependencies

T1 (single event-routing layer settled).

## Acceptance criteria

- Agent ↔ collector capability negotiation and task/result round-trips pass integration tests over protobuf+CRC32 framing.
- Reconnection with backoff verified; no regression to existing procmond actor/standalone paths.
