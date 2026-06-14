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

## Event store

### Event store

The agent-managed redb database of collected telemetry — `processes.events` and its sibling tables (scans, detection rules, alerts, alert deliveries). The agent is the single writer; the operator CLI and the detection engine read it. Distinct from the procmond-owned audit ledger, which is write-only forensic provenance, not queryable telemetry.

### Time bucket

The partition unit of `processes.events`: one base table (plus its secondary indexes) per time window, hourly by default and daily for low-volume hosts. Retention works at bucket granularity — expiring history is an O(1) drop of a whole bucket, not a row scan.

### MRC (materialized relation cache)

An in-memory cache of the parent relation (`pid → {ppid, parent_name, start_time}`), rebuilt on start from a bounded recent window. It turns the common parent/child lookup into a single map read. Always a cache, never a source of truth — if absent it is rebuilt, never recovered.

### Schema-version rebuild

The recovery path when the event store's `schema_version` tag does not match the running binary. The old partitions are exported as a signed bundle and dropped, the store reinitializes at the new version, and available procmond WAL is replayed — with an explicit gap record for whatever the WAL could not restore. Deliberately not an in-place migration.
