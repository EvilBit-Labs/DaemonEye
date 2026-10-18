# ADR-0015: Delivery state is independent of the detection cursor

**Date**: 2026-10-12 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

A detection cycle stores its alerts and advances the evaluated-through mark once every alert of the cycle is stored (ADR-0014). Before this decision the agent also delivered each alert inline, in the cycle loop, with no retry and no record of the outcome. An outage of every sink lost the external notification while the store said the cycle had completed, and a backoff of up to 60 s and an attempt timeout of up to 120 s could not live in the cycle without stalling detection.

Requirement 5 asks for network sinks, a circuit breaker, bounded retries, a dead-letter queue and publication on the embedded broker. Something has to hold delivery state across attempts, restarts and outages, and something has to decide whether the detection cursor waits for it.

## Decision

Delivery state lives in an agent-owned `alert_deliveries` table, one row per alert and sink, written in the same transaction as the alert. Per-sink workers deliver from those rows with a circuit breaker and a retry budget, and the detection cursor never depends on delivery: it advances when the cycle's alerts are stored, exactly as before.

The stored row is the source of truth. The eventbus message published on `events.alert.new` is fan-out and a wake signal for the workers; a periodic tick claims any row the bus did not announce.

## Alternatives Considered

### Alternative 1: Couple the cursor to delivery

- **Pros**: a cycle is "done" only when its alerts reached a sink, so nothing is ever recorded as evaluated without being delivered
- **Cons**: a dead sink stalls evaluation of every later window, or the same rows are re-evaluated and re-alerted each cycle until the sink recovers
- **Why not**: it turns a delivery outage into a detection outage; the mark exists to make evaluation progress, not to attest delivery

### Alternative 2: Use the broker as the delivery transport

- **Pros**: no new table; sinks are plain subscribers, which is how Requirement 5 reads at first
- **Cons**: the broker's queue is in memory, drops a message that has no subscriber, and rate-limits publishes; a crash between store and publish, or a slow subscriber, loses the notification with no record
- **Why not**: it cannot satisfy the dead-letter requirement, and nothing durable would show what was never delivered

## Consequences

### Positive

- An alert is stored once and delivered at least once per sink; an outage of every sink parks rows that are retried and dead-lettered with a reason, and the dead letters are redriven in bounded batches when the sink recovers.
- Delivery attempts are auditable per sink and alert without touching procmond's audit ledger.
- A failing or slow sink costs the cycle nothing beyond writing its delivery row.

### Negative

- Delivery is at least once. A crash between a sink accepting and the row being written resends the alert; a re-evaluated window stores the same logical alert under a new id. Every network sink therefore carries two keys: the alert id for retries and the deduplication key for re-evaluation.
- `lettre` keeps the SMTP credentials as plain strings for the process lifetime; the resolver's zeroising wrapper protects them only until the transport is built.
- Credentials and the CA file are read once at sink build, so rotating either needs a restart.
- Delivery rows are not tamper-evident; they are the agent's record, not the ledger's.
- During a sustained store outage the re-evaluated window delivers the same alert to local sinks every cycle until the store recovers; this is visible on the console and preferred to suppressing alerts that were never durably stored.

### Risks

- The row cap per sink is severity-aware so a flood of low-severity alerts cannot keep a critical one off the wire; the eviction counter in health shows when the cap bit.
- Automatic redrive arms only after a sink's breaker has opened and then stayed closed for a full cool-down, so a sink that fails below the breaker threshold keeps its dead letters until the agent restarts or an operator redrives it. Health shows the dead-letter depth.
