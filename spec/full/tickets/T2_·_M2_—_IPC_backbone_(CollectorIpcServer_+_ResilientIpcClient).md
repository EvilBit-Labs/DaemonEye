# T2 · M2 — IPC backbone (CollectorIpcServer + ResilientIpcClient)

**Milestone:** M2 · **Backlog:** T15.1 (#78), T15.2 (#77)

## Scope

**In:**

- Wire the existing `CollectorIpcServer` (already constructed and started with a live handler in file:collector-core/src/collector.rs) into the production agent→collector path: capability negotiation, task routing, connection management. This is not net-new code — see file:docs/solutions/architecture-patterns/ipc-backbone-actual-state-transport-duality.md before starting.
- Finish capability negotiation on the existing `ResilientIpcClient` (file:daemoneye-lib/src/ipc/client.rs). There is no type named `IpcClientManager` in the workspace; reconnection/backoff/failover already ship. The real gap: `negotiate_capabilities` sends a task, discards the peer's response, and returns a hardcoded `CollectionCapabilities` constant — replace it with a round-trip reflecting the peer's actual reply. Stays compatible with procmond `ProcessMessageHandler`.
- Integration tests for both directions.

**Out:** SQL→task generation (T5); DataFusion execution (T6).

## Spec references

- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(Component Architecture: eventbus/IPC interfaces)
- requirements R3.1–3.2, R11.1–11.2, R12.1–12.2

## Key touchpoints

- file:collector-core/src/ipc.rs — `CollectorIpcServer` (capability negotiation, task routing, connection mgmt), already live at file:collector-core/src/collector.rs; see file:collector-core/src/capability_router.rs, file:collector-core/src/rpc_services/.
- file:daemoneye-agent/src/ipc_server.rs, file:daemoneye-agent/src/broker_manager/, file:daemoneye-agent/src/collector_registry.rs — wire the agent side to `ResilientIpcClient` (reconnection w/ backoff already implemented; capability negotiation is the outstanding piece; task distribution/result collection).
- file:daemoneye-lib/src/ipc/ — `codec.rs`, `client.rs` (`ResilientIpcClient`), `interprocess_transport.rs` (protobuf + CRC32 framing; honor 107-byte Unix socket path limit).
- file:daemoneye-lib/proto/ipc.proto — `DetectionTask`/`DetectionResult`, capability messages.
- **Frame format decision:** the codec frame (file:daemoneye-lib/src/ipc/codec.rs) is `length(u32 LE) + crc32(u32 LE) + protobuf bytes` with no message-type tag, and the interprocess server hardcodes `DetectionTask` as the only decodable type. T2 adds a **message-type discriminator** to the frame so capability and task messages are distinct types rather than conventions inside one envelope. Because this is a wire-format change, T2 states and tests the compatibility plan for existing peers.
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
