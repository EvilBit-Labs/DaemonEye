---
title: 'IPC backbone actual state: eventbus/interprocess transport duality and stubbed seams'
date: 2026-06-11
category: architecture-patterns
module: daemoneye IPC backbone (eventbus + interprocess)
problem_type: architecture_pattern
component: service_object
severity: medium
applies_when:
  - Planning or implementing the M2 IPC backbone, or any ticket that frames CollectorIpcServer / IpcClientManager as net-new
  - Deciding where agent->procmond task dispatch actually rides (eventbus RPC vs interprocess client)
  - Touching capability negotiation, the codec frame, or the interprocess resilience stack (CircuitBreaker / ConnectionPool / LoadBalancingStrategy)
  - Wiring a CLI->agent IPC path or un-stubbing the agent-side storage reads (get_all_processes / get_stats)
  - Scoping multi-collector work and questioning why the resilience stack has no current production consumer
tags:
  - ipc
  - eventbus
  - interprocess
  - transport-duality
  - capability-negotiation
  - resilience-stack
  - spec-vs-code
  - shadowhunt
---

# IPC backbone actual state: eventbus/interprocess transport duality and stubbed seams

> **Point-in-time snapshot (2026-06-11, during M2/T2 planning).** Several seams named below are stubs scheduled for implementation. Re-verify line numbers and stub status against the working tree before relying on this after that work lands.

## Context

The M2 IPC backbone ticket (`spec/full/tickets/T2_·_M2_—_IPC_backbone...`) and the `.kiro/specs/daemoneye-core-monitoring/tasks.md` entries frame the IPC layer as net-new work to "implement" and "complete":

> **Source of truth:** `.kiro/` and `spec/full/` are both valid, but `spec/full/` (specs + tickets) updates faster and leads. On any conflict, trust `spec/full/` and correct `.kiro/`. The `.kiro/` citations here reflect where the framing/numbering currently lives; verify against `spec/full/` if they diverge.

- "**Implement** `CollectorIpcServer` in `collector-core/src/ipc.rs`"
- "**Complete** `IpcClientManager` in daemoneye-agent — reconnection, capability negotiation, task distribution"

Taken at face value this is misleading. Roughly **80% of the backbone already exists**, and **`IpcClientManager` is a phantom name** — no Rust type by that name exists anywhere in the workspace (it appears only in `BACKLOG.md` and `.kiro` planning prose). The real types are:

- `BrokerManager` (`daemoneye-agent/src/broker_manager/`) — eventbus-backed collector RPC plus lifecycle/health/config providers.
- `ResilientIpcClient` (`daemoneye-lib/src/ipc/client.rs`) — the interprocess transport client (circuit breaker, pool, load balancing).
- `CollectorRegistry` (`daemoneye-agent/src/collector_registry.rs`) — registration bookkeeping.
- `CollectorIpcServer` (`collector-core/src/ipc.rs`) — the interprocess server, spun up in production at `collector-core/src/collector.rs:1614`.

An agent who searches for `IpcClientManager` finds nothing and may conclude the work is unstarted. It is not.

## Guidance

Treat the IPC layer as **two distinct transports**, not one. Conflating them is the single most expensive misread available here.

### Transport A — eventbus (the live process-dispatch path)

This is the path actually exercised in production today.

- `daemoneye-agent/src/main.rs:353` calls `broker_manager.execute_task_rpc("procmond", task)` each scan cycle.
- `execute_task_rpc` (`broker_manager/rpc.rs:190`) serializes the `DetectionTask` to JSON (`RpcPayload::Task`), sends it over `CollectorRpcClient`, and deserializes `RpcPayload::TaskResult` back into a `DetectionResult`.
- `health_check_rpc` (`rpc.rs:170`) and the `HealthProvider` / `ConfigProvider` / `RegistrationProvider` impls on `BrokerManager` are fully implemented.

This is the transport that matters for single-host ShadowHunt operation. It works.

### Transport B — interprocess `ResilientIpcClient` + `CollectorIpcServer`

The interprocess transport carries `DetectionTask`/`DetectionResult` as framed protobuf over Unix sockets / named pipes. Its two halves are in very different states, and that asymmetry is the heart of this doc:

- **Server half — wired AND consumed.** `CollectorIpcServer::new(...)` is constructed and `ipc_server.start(move |task| ...)` invoked with a live handler in production at `collector-core/src/collector.rs:1614`. The server loop (`interprocess_transport.rs:322`) decodes a `DetectionTask` per frame and calls the handler, whose signature is `Fn(DetectionTask) -> Fut` returning `DetectionResult` (`set_handler`, `interprocess_transport.rs:131`).
- **Client half — wired but NOT consumed in production.** `ResilientIpcClient::send_task` (`client.rs:1244`) → `send_task_with_routing` (1049) → `send_task_to_endpoint` (1086) → `send_task_on_stream` (1156) is a functionally complete chain, exercised by integration tests (`daemoneye-lib/tests/resilient_client_integration.rs`, `ipc_capability_negotiation.rs`) and benches. It has **zero production callers** — grep across `procmond/`, `daemoneye-agent/`, `daemoneye-cli/`, `collector-core/` finds none outside tests/benches. Its only non-test caller is `daemoneye-lib/src/detection/sql_to_ipc.rs` (the SQL→IPC detection path, the ticket's out-of-scope T5/T6 consumers), which is itself not wired into any binary.

### Three categories — keep them distinct

Do not collapse these into "TODO" or "dead code":

1. **Complete machinery without a production caller** — the `send_task*` chain, `CircuitBreaker` (`client.rs:247`), `ConnectionPool` (`client.rs:592`), `LoadBalancingStrategy` (`client.rs:716`). These carry `#[allow(dead_code)] // Work in progress` markers but are **not** deletable dead code — they pass integration tests and benches. They lack a production caller, full stop. (Historical note: the reconnect-with-backoff loop was the single genuine *implementation* gap when this doc was written; T2a / PR #196 closed it — the `max_reconnect_*` fields are now consumed by the bounded retry loop in `send_task_to_endpoint`, alongside explicit failover, real circuit-breaker state in `get_stats`, and pool reuse. The chain still has no production caller, so the `#[allow(dead_code)]` markers remain.)
2. **Genuine stubs that return placeholders:**
   - `negotiate_capabilities` (`client.rs:~898`) *looks* real — it opens a socket and sends a `DetectionTask` marked `metadata: "capability_negotiation"` — but on success it discards any peer response and returns a **hardcoded constant** (`CollectionCapabilities { supported_domains: vec![Process], advanced: { kernel_level: false, realtime: true, system_wide: true } }`). It parses nothing from the peer.
   - Storage reads: `get_all_processes` (`storage.rs:243`) returns `Ok(Vec::new())`, `get_stats` (`storage.rs:387`) returns `Ok(DatabaseStats::default())`. Both carry the verbatim comment `// TODO: Implement in Task 8 - redb database integration` (this "Task 8" is the `.kiro/tasks.md` numbering, distinct from the `T`-prefixed ticket schemes — do not relabel and chase the wrong item).
   - `let _db_manager = storage::DatabaseManager::new(...)` at `main.rs:68` is constructed and **immediately dropped** — the agent does not yet read/write its event store; it relies entirely on the eventbus RPC round-trip to procmond for process data.
3. **Defined-but-unhandled protobuf scaffolding** — capability negotiation exists as wire types on *both* transports but is stubbed two different ways:
   - eventbus: `CollectorOperation::GetCapabilities` and `RpcPayload::Capabilities(CapabilitiesData)` are defined (`daemoneye-eventbus/src/rpc/messages.rs`), but procmond routes `GetCapabilities` into the `UnsupportedOperation` catch-all (`procmond/src/rpc_service/mod.rs:182`).
   - interprocess: `CapabilityRequest`/`CapabilityResponse` are defined in `proto/ipc.proto:78-97` and re-exported in `proto.rs`, but **never constructed or consumed** in Rust. Negotiation piggybacks on the `DetectionTask` metadata marker (the stub above).

### One codec detail that constrains real negotiation

The frame format (`codec.rs` `write_message` / `read_message`) is `length(u32 LE) + crc32(u32 LE) + protobuf bytes`. There is **no message-type tag / discriminator** in the frame. The codec is generic over `T: Message` and the receiver must already know which type to decode — which is why the interprocess server hardcodes `DetectionTask` at `interprocess_transport.rs:324`. Adding a second request type on this transport (e.g. a real `CapabilityRequest`) requires either a frame type tag or continued reuse of the `DetectionTask` envelope. This is a real design constraint for anyone implementing true capability negotiation.

## Why This Matters

A first inventory pass *in this very planning session* wrongly labeled the `ResilientIpcClient` resilience stack as "dead code" based purely on the `#[allow(dead_code)]` attributes. That was wrong: the chain is wired and exercised by tests/benches — it simply has no production caller yet. The truth required reading the call chain and grepping for callers, not trusting the attribute. A plan built on the wrong inventory would have "added" a circuit breaker that already exists and skipped the one thing actually missing (the backoff loop and the tests).

Future agents on IPC work will burn discovery cost or build the wrong thing if they:

- trust the "implement/complete" framing and rebuild `CollectorIpcServer` from scratch (it already runs at `collector.rs:1614`);
- search for `IpcClientManager` and conclude the client is unstarted (the real type is `ResilientIpcClient`);
- assume the resilience stack is either dead (it's not — tests depend on it) or production-active (it's not — no caller until the SQL→IPC path is wired);
- assume capability negotiation works because the protobuf types and `negotiate_capabilities` exist (the handlers are stubs / unhandled, and `negotiate_capabilities` returns a hardcoded constant);
- assume the agent reads/writes its event store (it constructs `DatabaseManager` and drops it; storage reads are stubbed pending `.kiro` Task 8).

The deeper lesson: **on a partially-built codebase, "what exists / is wired / is dead code" must be code-verified, not taken from a ticket, a spec, or a single inventory pass.** Adversarial, code-verified review is what separates a real inventory from a plausible-sounding one.

## When to Apply

Apply this inventory when planning or implementing anything touching:

- the IPC layer (either transport);
- capability negotiation / `CollectionCapabilities` / `CapabilityRequest`/`Response`;
- agent ↔ collector, agent ↔ procmond, or agent ↔ CLI communication;
- the redb event-store read path on the agent side (currently stubbed).

## Examples

**Before/after framing — server:**

- Ticket: "Implement `CollectorIpcServer` in `collector-core/src/ipc.rs`."
- Reality: `CollectorIpcServer` is already constructed and started with a live handler at `collector-core/src/collector.rs:1614`. The remaining work is the reconnection/backoff loop and tests — not a from-scratch implementation.

**Before/after framing — client:**

- Ticket: "Complete `IpcClientManager` in daemoneye-agent."
- Reality: there is no `IpcClientManager` type. The closest real type, `ResilientIpcClient`, has a complete `send_task` chain (`client.rs:1244→1049→1086→1156`) covered by tests/benches but with no production caller. The work is to *wire it in* and replace the hardcoded `negotiate_capabilities` constant — not to write the client.

**The trap that looks real:**

- `negotiate_capabilities` opens a socket, builds a `DetectionTask` tagged `metadata: "capability_negotiation"`, and sends it — looking like a genuine round-trip. But on success it discards the peer response and returns a hardcoded `CollectionCapabilities`. Extending domain support means replacing this constant *and* adding a real `GetCapabilities` handler in procmond (currently `UnsupportedOperation`).

**Stub vs. complete-machinery distinction in one line:**

- `storage.rs:243 get_all_processes → Ok(Vec::new())` is a **stub** (placeholder, `.kiro` Task 8).
- `client.rs:592 ConnectionPool` is **complete machinery without a production caller** (full impl, tested, `#[allow(dead_code)]`) — different category, different remediation.

## Related

- `docs/solutions/best-practices/rust-security-batch-cleanup-patterns-2026-04-04.md` — eventbus broker internals (atomic sequence counter, `Arc<BusEvent>` fan-out, socket `0o600` perms, 107-byte socket-path limit) that form the "what's built" substrate of Transport A. **Possible refresh candidate:** it documents a crossbeam spin-wait fix, but crossbeam was removed in PR #190 (commit `3fa2341`) — that section may now describe code that no longer exists.
- `docs/solutions/best-practices/rust-async-arc-rwlock-await-holding-lock-pattern-2026-04-18.md` — `procmond/src/event_bus_connector` + `EventBusClient` wiring; the consumer side of the eventbus transport.
- Tracking issues for the unconsumed/scaffolded work: **#116** (Task Distribution and Capability-Based Routing via EventBus), **#114** (Epic: Multi-Process Collector Coordination), **#117** (Result Aggregation / Load Balancing), **#118** (multi-collector E2E + failover). The closed origin issues are **#87** (custom daemoneye-eventbus, remove legacy IPC) and **#115** (message broker).
- Brainstorm/plan that surfaced this inventory: `docs/brainstorms/2026-06-11-ipc-backbone-collector-agent-requirements.md`, `docs/plans/2026-06-11-001-feat-ipc-backbone-plan.md`.
