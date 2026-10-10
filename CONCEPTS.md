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

### Audit ledger

The write-once, hash-chained record of what a privileged collector observed and did. procmond is its only writer; the agent and the operator CLI read it. That asymmetry is the point — a component that cannot write the chain cannot forge history in it, so the ledger stays trustworthy even if everything above it is compromised. Distinct from the Event store, which holds collected telemetry and which the agent does write.

### Time bucket

The partition unit of `processes.events`: one base table (plus its secondary indexes) per time window, hourly by default and daily for low-volume hosts. Retention works at bucket granularity — expiring history is an O(1) drop of a whole bucket, not a row scan.

### MRC (materialized relation cache)

An in-memory cache of the parent relation (`pid → {ppid, parent_name, start_time}`), rebuilt on start from a bounded recent window. It turns the common parent/child lookup into a single map read. Always a cache, never a source of truth — if absent it is rebuilt, never recovered.

### Schema-version rebuild

The recovery path when the event store's `schema_version` tag does not match the running binary. The old partitions are exported as a signed bundle and dropped, the store reinitializes at the new version, and available procmond WAL is replayed — with an explicit gap record for whatever the WAL could not restore. Deliberately not an in-place migration.

### Drop gate

The checks a schema-version rebuild must pass before it destroys the live store. Three distinct properties, each rejecting on its own: the archive bundle is authentic (signature), it holds every partition its manifest claims (completeness), and it is the archive this run just wrote rather than an older one (this-run identity). Any failure aborts before the drop, leaving the old store intact.

## Detection rules

### Schema catalog

The agent's registry of what collectors can actually serve: per collector, the tables it contributes to the shared namespace, each table's columns and types, and the pushdown operations it claims per column. Populated by authenticated registration at collector startup and static thereafter — a collector changes its descriptor by re-registering, not by pushing an update.

### Pushdown plan

The two halves a detection rule lowers into at load time. The pushed half is the typed predicate conjunction and projection sent to the owning collector as a Detection task; the residual half is everything the collector was not verified to handle, which the agent evaluates itself. Both halves are recorded, so "not pushed" is always distinguishable from "nothing to push."

### Conformance vector

The per-operation check that a collector's evaluation of a pushdown operation matches the agent's own. An operation the collector advertises but has no passing vector for is treated as unadvertised, and its predicates stay in the residual. The gate is a capability boolean, never a cost estimate.

### Unhealthy rule

A rule the agent has stopped trusting to run correctly. The usual cause is a collector re-registering with a descriptor that dropped a table or column the rule references, but a pushdown task expiring without renewal and a regex pattern exceeding its latency threshold both mark a rule unhealthy too. The rule is surfaced to the operator through daemoneye-cli rather than being silently dropped or left to match nothing.

The cause decides how the rule recovers. A dropped reference or an expired task is answered by the catalog, so a later registration that actually revalidates the rule — a first registration, or one whose change touches the rule's tables — re-validates and re-plans it on its own; an identical re-registration produces no change and re-heals nothing. A latency breach is not answered by the catalog at all: nothing about the schema speaks to how long a pattern takes to run, so that verdict survives re-validation, re-planning, and re-enabling, and only reloading the rule clears it. A latency-breached rule is also disabled, which the other two causes leave untouched.

### Completeness marker

A value carried by every evaluation and every alert saying whether the rule saw everything it was meant to: `Complete`, or `Degraded` with at least one concrete reason (a collector failure or missed heartbeat, ingest shedding, a sequence gap, a resource limit, the match cap, an execution error). The two cannot be mixed: a degraded marker with no reason, or a complete one with reasons, cannot be built or deserialized. Zero matches under `Degraded` means "could not fully evaluate", not "no match". There is deliberately no default, because a defaulted marker would claim `Complete` for a run that examined nothing.

### Evaluation window

The half-open interval of `collection_time`, `(after_ms, through_ms]`, that one detection cycle evaluates each rule against: the previous cycle's high-water mark, exclusive, to this cycle's, inclusive. Consecutive windows tile the timeline, so a row is evaluated by exactly one cycle, and the last completed cycle's mark is persisted so a restart resumes from it. A window normally sits inside one time bucket; a window spanning many buckets (a wide ad-hoc query or catch-up after an outage) is the full-retention shape, which costs far more memory and time.

### Rule generation

An engine-unique number issued each time a rule is loaded, naming that load of the rule. A rule id survives a reload, so a latency report carries the generation of the instance it measured and the engine discards it if the rule has since been reloaded. Only the engine can issue one; a caller cannot construct it.
