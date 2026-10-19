---
title: Transactional outbox
date: 2026-10-12
input_shape: concept
subject: alert delivery (alert_deliveries table, SinkWorker)
---

# Transactional outbox

Instead of sending an alert to a receiver and then recording that it was sent, the agent records the intent to send in the same database transaction that stores the alert, and a separate worker reads those rows and performs the sends. The row, not the send, is the source of truth.

## Why here

Publishing straight from the detection cycle was the obvious alternative. The embedded broker's queue is in memory, rate-limited and silently drops with no subscriber, so it cannot carry durability, and a send inside the cycle would couple the detection cursor to a receiver's uptime. With the outbox the cycle only pays for its own write, and an outage is a backlog of rows rather than lost alerts (ADR-0015).

## In this repository

- `EventStore::enqueue_alert` writes the alert and one `alert_deliveries` row per sink in one redb transaction, as an insert that never overwrites an existing row.
- `SinkWorker` claims a due row (recording the attempt before the send), sends, then writes the outcome back with compare-and-set, so a redrive or an eviction that touched the row first wins over a stale worker.
- A crash between the send and the write-back is handled by delivering at least once, which is why every receiver gets an idempotency key (the alert id) and a second key for re-evaluated windows (the deduplication key).

## When not to use it

When a lost notification is acceptable (metrics, best-effort telemetry), or when the receiver cannot deduplicate, because an outbox turns "maybe never" into "maybe twice".
