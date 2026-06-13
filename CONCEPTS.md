# Concepts

Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Glossary only, not a spec or catch-all.

## Agent–collector IPC

### Relationships

The agent reaches a collector over the **Eventbus** (the live single-host path, used by procmond) or, for SDK collectors, over the **Interprocess transport**. The operator CLI also uses the Interprocess transport to talk to the agent — it is a client of the agent, not a collector endpoint. Before sending **Detection tasks** to a collector, the agent performs **Capability negotiation** to learn which monitoring domains that collector supports.

### Collector

A process that gathers host telemetry (process, network, filesystem, performance) and answers the agent's tasks. The privileged built-in collector is procmond; the SDK lets others be built. A collector advertises its supported monitoring domains through Capability negotiation, and the agent routes Detection tasks only to collectors that support them.

### Capability negotiation

The exchange in which the agent learns a collector's supported monitoring domains before dispatching work. The agent caches the negotiated view and refreshes it on reconnection or health change; a shortfall between a task's required domain and the collector's advertised domains is a degraded-coverage condition, surfaced rather than silently dropped.

### Eventbus

The broker-based message transport that carries agent↔collector RPC (task dispatch, health, config, registration) on a single host. It is the live path for the built-in collector. Distinct from the Interprocess transport, which serves SDK collectors and the CLI.

### Interprocess transport

The framed-protobuf-over-socket transport (Unix socket or named pipe) used for SDK collectors and operator-CLI communication, as opposed to the Eventbus. Its client carries resilience behavior (reconnection, circuit breaking, connection pooling, endpoint routing) intended for the multi-collector future.

### Detection task

A unit of collection work the agent sends to a collector, naming a monitoring domain and filters; the collector answers with a Detection result (the collected records or an error). A task targets a single monitoring domain and is rejected by a collector that lacks the corresponding capability.
