---
title: 'Rust Async: Wrap RwLock-held resources in Arc to satisfy await_holding_lock'
date: 2026-04-18
category: best-practices
module: procmond/event_bus_connector
problem_type: best_practice
component: tooling
severity: medium
applies_when:
  - A struct holds an `Option<T>` or `T` behind `Arc<tokio::sync::Mutex<...>>` or `Arc<tokio::sync::RwLock<...>>` and a caller needs to `await` on that inner value
  - Clippy denies `await_holding_lock` and a lock guard is live across an `.await` point
  - The inner resource's async methods take `&self` and the caller can work through a cloned `Arc` (no need for `&mut self`)
  - Shutdown needs exclusive ownership of a resource that is normally shared via `Arc`
  - A per-iteration timeout must not reset on each loop iteration — use a single absolute `timeout_at` deadline instead
tags:
  - rust
  - async
  - tokio
  - concurrency
  - rwlock
  - arc
  - clippy
  - await-holding-lock
  - shutdown
---

# Rust Async: Wrap RwLock-held resources in Arc to satisfy `await_holding_lock`

## Context

During the END-297 PR #178 review, `procmond`'s startup code introduced a shared `EventBusConnector` behind `Arc<tokio::sync::RwLock<EventBusConnector>>` and added an opt-in control-message subscription call path. The initial implementation acquired the read guard and then awaited on `bus_guard.subscribe_with_control(...)` — the guard was live across the `.await` point, which the workspace rule `clippy::await_holding_lock = "deny"` correctly flagged.

The pattern below is the canonical project-wide fix for this class of problem. It applies anywhere a resource lives inside a `tokio::sync::Mutex` or `RwLock` and callers need to issue async calls on it.

The `clippy::await_holding_lock = "deny"` rule was originally not inherited on the `collector-core` and `daemoneye-eventbus` crates (session history); once the workspace-lints inheritance landed, any async call made while holding a lock guard started failing to compile. This pattern is the canonical response — expect it to be needed again whenever an `Arc<RwLock<Container>>` field is introduced (session history).

## Guidance

### 1. The anti-pattern: lock guard held across `.await`

```rust
// WRONG — read guard is live when .await executes
let subscribe_result = {
    let bus_guard = event_bus.read().await;          // guard acquired
    bus_guard
        .subscribe_with_control(subscriber_id, topics)
        .await                                        // clippy::await_holding_lock fires here
};
```

Other tasks needing the lock are blocked for the full duration of the async call, which can be unbounded. The lint denies this shape workspace-wide.

### 2. The fix: clone an `Arc` inside a short guard, drop it, then await

Wrap the inner resource in `Arc<ResourceType>` inside the lockable container. Add a cheap accessor that clones the `Arc`. Callers acquire the guard only long enough to take the clone, drop the guard, then `.await` on the owned `Arc`.

In `procmond/src/event_bus_connector.rs`:

```rust
pub struct EventBusConnector {
    // store the client behind Arc so callers can clone it out of the lock
    client: Option<Arc<EventBusClient>>,
    /* ... */
}

/// Cheap accessor — just clones the Arc pointer. No async, no I/O.
#[must_use]
pub fn client_arc(&self) -> Option<Arc<EventBusClient>> {
    self.client.clone()
}
```

At the call site in `procmond/src/main.rs`:

```rust
// CORRECT — guard scope ends before .await
let client_arc = {
    let bus_guard = event_bus.read().await;
    bus_guard.client_arc()                // clone; guard drops at block end
};
let subscribe_result = match client_arc {
    Some(client) => client.subscribe_with_control(subscription).await,
    None => return Err(anyhow::anyhow!("not connected to broker")),
};
```

The `{ let bus_guard = ...; bus_guard.client_arc() }` block shape is what makes the lint pass — the guard drops at the end of the block, before the expression value feeds into the subsequent `.await`.

### 3. `Arc::into_inner` for shutdown when the inner type consumes `self`

Some inner types have `async fn shutdown(self) -> Result<(), E>` that consumes `self`. When the normal value is shared via `Arc`, use `Arc::into_inner` to reclaim ownership only when the strong count is 1 — if any clones are in flight, log and drop so background tasks exit naturally.

From `procmond/src/event_bus_connector.rs`:

```rust
if let Some(client_arc) = self.client.take() {
    match Arc::into_inner(client_arc) {
        Some(client) => {
            if let Err(e) = client.shutdown().await {
                error!(error = %e, "Error during client shutdown");
            }
        }
        None => {
            warn!(
                "EventBusClient has outstanding Arc references; skipping \
                 explicit shutdown - background tasks will exit when the \
                 Arc drops"
            );
        }
    }
}
```

`self.client.take()` removes the connector's own reference. If `Arc::into_inner` returns `None`, outstanding callers still hold the client alive; they will drop it when they go out of scope and the client's `Drop` / background-task shutdown paths take over.

### 4. Single absolute deadline with `timeout_at` for wait loops

When waiting for a signal across multiple loop iterations, compute the deadline once before the loop. `tokio::time::timeout(dur, fut)` applied per iteration resets the window on every received message, which lets a chatty or adversarial stream push the deadline out indefinitely (session history: this was a CodeRabbit Major finding on the first implementation).

From `procmond/src/main.rs` (`lifecycle_wait_task`):

```rust
// CORRECT — deadline is fixed once; cannot be reset by arriving messages
let now = tokio::time::Instant::now();
let deadline = now
    .checked_add(BEGIN_MONITORING_WAIT_TIMEOUT)
    .unwrap_or(now);
loop {
    match tokio::time::timeout_at(deadline, control_rx.recv()).await {
        Ok(Some(msg)) if msg.topic == lifecycle_topic => {
            /* transition to running */
            return;
        }
        Ok(Some(_))   => { /* non-lifecycle — keep waiting */ }
        Ok(None)      => { /* channel closed — fallback */ return; }
        Err(_elapsed) => { /* timed out — fallback */ return; }
    }
}
```

`checked_add` + `unwrap_or(now)` is required because the workspace enables `clippy::arithmetic_side_effects = "deny"`; the overflow case is treated as an immediate deadline (fails fast) rather than silently panicking.

## Why This Matters

- **Lock contention under load.** An async call holding a lock can take tens to hundreds of milliseconds. Every other task needing the lock — including read-only observers — blocks for that duration. Under high throughput the effect compounds.
- **Workspace clippy enforcement is hard.** `await_holding_lock = "deny"` is workspace-wide, and the `--all-targets` pre-commit hook treats any offending shape as a build failure, not a warning. The `Arc`-clone pattern is the canonical fix the lint expects.
- **Shutdown-race survivability.** `Arc::into_inner` is the safe way to call a consuming `shutdown(self)` on a shared value. The fallback (log + drop) ensures shutdown never hangs on in-flight subscribers.
- **Deadline absoluteness matters.** `timeout_at` makes the wait window honest about the operator's intent. Per-iteration `timeout` is the most common accidental way to turn a hard SLO into an infinite wait.

## When to Apply

Apply this pattern when **all** of the following hold:

- A shared resource is behind `Arc<tokio::sync::Mutex<T>>` or `Arc<tokio::sync::RwLock<T>>`.
- Callers need to invoke `async fn (&self, ...)` methods on the resource.
- The inner resource is either cheap to wrap in `Arc` or already uses interior mutability internally.

**Do not apply when:**

- The method requires `&mut self`. `Arc<T>` only hands out `&T`; mutating operations must still happen inside the lock. Restructure the caller so the mutation is a short non-awaiting block, or move to a dedicated owner-task/actor pattern.
- The resource has exclusive single-owner semantics and is never shared.
- `shutdown` is a sync `&mut self` method — plain `Option::take()` + normal shutdown is simpler.

## Examples

Concrete references in this repo:

- `procmond/src/event_bus_connector.rs` — the `client: Option<Arc<EventBusClient>>` field and `client_arc()` accessor; `Arc::into_inner` in the shutdown path.
- `procmond/src/main.rs` — the short-guard clone shape at the `subscribe_with_control` call site; `timeout_at` + absolute deadline in `lifecycle_wait_task`.
- Enforcement: `cargo clippy --workspace --all-targets -- -D warnings`.

### Before — lock guard held across `.await`

```rust
let bus_guard = event_bus.read().await;
let result = bus_guard
    .subscribe_with_control(subscription)
    .await?;                                 // guard alive — clippy denies
```

### After — clone `Arc`, drop guard, then await

```rust
let client_arc = {
    let bus_guard = event_bus.read().await;
    bus_guard.client_arc()                    // Option<Arc<EventBusClient>>
};                                            // guard dropped here
let result = match client_arc {
    Some(client) => client.subscribe_with_control(subscription).await?,
    None => return Err(anyhow::anyhow!("not connected")),
};
```

### Before — per-iteration timeout (window resets on every message)

```rust
loop {
    match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await { /* ... */ }
}
```

### After — single absolute deadline

```rust
let deadline = tokio::time::Instant::now()
    .checked_add(Duration::from_secs(60))
    .unwrap_or_else(tokio::time::Instant::now);
loop {
    match tokio::time::timeout_at(deadline, rx.recv()).await { /* ... */ }
}
```

### Shutdown — consuming the client when the refcount allows

```rust
if let Some(client_arc) = self.client.take() {
    match Arc::into_inner(client_arc) {
        Some(client) => { let _ = client.shutdown().await; }
        None         => warn!("outstanding Arc clones; background tasks will self-exit"),
    }
}
```

## Related

- [rust-security-batch-cleanup-patterns-2026-04-04.md](rust-security-batch-cleanup-patterns-2026-04-04.md) — companion best-practices doc covering `Arc<AtomicU64>` (over `Mutex<u64>` for counters), `Arc<BusEvent>` zero-copy fan-out, crossbeam idle-loop blocking, Unix socket permissions, and UTF-8-safe truncation in the same eventbus/IPC domain. This doc is orthogonal — that one is about replacing locks with cheaper primitives; this one is about living well when a lock is genuinely needed.
- PR #178 on `EvilBit-Labs/DaemonEye`, commit `2eb4ba4` — the concrete instance that motivated this doc (END-297 closure pass).
