---
title: Assert the mechanism, not just the outcome, in resilience integration tests
date: 2026-06-13
category: docs/solutions/best-practices
module: daemoneye-lib
problem_type: best_practice
component: testing_framework
severity: high
applies_when:
  - writing integration tests for resilience, failover, retry, or recovery code paths
  - a test asserts only on final outcome (e.g. send/request success)
  - the system under test has a fallback or fast-path that can satisfy the outcome without exercising the target branch
  - reviewing tests for reconnect, retry-with-backoff, connection-pool, or circuit-breaker behavior
  - test setup mutates shared state (e.g. server shutdown) but cannot guarantee the intended failure condition was reached
tags:
  - integration-testing
  - resilience-testing
  - test-assertions
  - false-positive-tests
  - failover
  - connection-pool
  - ipc
  - mechanism-vs-outcome
---

# Assert the mechanism, not just the outcome, in resilience integration tests

## Context

When you add an integration test for a resilience or recovery feature — reconnect-with-backoff, explicit failover, circuit-breaker tripping, connection-pool stale-fallback — the natural first assertion is on the *outcome*: the request eventually succeeded. That assertion is dangerously weak. A resilience feature is, by definition, a *secondary* path that only runs when the *primary* path fails. If the test setup lets the request reach success via the primary path, the secondary path you are trying to cover never executes, and an outcome-only assertion passes anyway. You get a green test with a confident name (`test_failover_routes_around_downed_endpoint`) that proves nothing about failover.

This surfaced concretely while shipping the M2/T2a IPC interprocess resilience slice (PR #196, merged to `main` as `d3fec67`). Two integration tests in `daemoneye-lib/tests/resilient_client_integration.rs` were hollow — both passed on outcome and never ran the named code path. Both were exposed only when assertions were strengthened from "did it succeed?" to "did the mechanism run?", at which point the strengthened assertions failed.

## Guidance

For any resilience/recovery test, assert the **mechanism**, not just the **outcome**:

1. **Identify the secondary path** the test is named for (failover branch, reconnect loop, stale-connection fallback, breaker-open rejection).
2. **Make the setup force the primary path to genuinely fail** — in a way that is not short-circuited before the secondary path is reached.
3. **Assert an observable signal that the secondary path ran** — counters and state transitions, not the final result:
   - `connection_attempts_total >= 2` (the primary was actually dialed before failover)
   - `connections_established_total == before + 1` (a *fresh* connection was made, proving the pooled one was discarded)
   - endpoint `is_healthy == false` / breaker state transitioned to `Open` after the failure
4. **Apply the falsifiability test:** ask *"can this test reach the passing outcome via the wrong path?"* If yes, the assertion is too weak — tighten it until the wrong path produces a failing assertion.
5. **If the mechanism cannot be triggered deterministically with the available API, delete the test** rather than keep a green-but-hollow one. Validate the production code by review instead, and say so. A hollow test is worse than no test: it actively asserts false confidence.

Two DaemonEye-specific traps make hollow resilience tests easy to write — watch for analogues in any system:

- **Pre-culling of "down-from-the-start" endpoints.** `send_task_with_routing` runs `refresh_capabilities()` *before* `select_endpoint_for_task`. An endpoint with no live server fails capability negotiation, is marked unhealthy, and is culled before selection — so routing picks the healthy alternate *directly* and the explicit-failover branch never runs. To test failover you must make the primary **selectable** (healthy, task-compatible — pre-set capabilities via `CollectorEndpoint::update_capabilities` with no live server) yet **failing on send**.
- **`graceful_shutdown()` does not kill accepted connections.** `InterprocessServer::graceful_shutdown()` only signals the accept loop to stop and runs `force_cleanup` (removes the Unix socket file but does not close already-open fds). An already-accepted connection handler keeps running, so on Unix a pooled connection stays alive after "shutdown" and serves the next request — the stale-connection fallback never fires. (Windows named pipes behaved differently, which is why this only failed in CI under a `#[cfg(unix)]` gate that had been papering over it.)

## Why This Matters

A test that passes via the wrong path is strictly worse than no test. No test is an honest gap; a hollow green test is a false claim of coverage that survives refactors, gives reviewers false confidence, and lets the actual resilience path rot undetected. Resilience code is exactly the code you cannot afford to have silently break — it only matters during failure, and by the time it is exercised in production you have lost the chance to catch the bug. The whole value of the test is to exercise the rare failure path in a controlled setting; if the setup routes around that path, the test inverts its own purpose.

The strengthened assertion is also a *detector*, not just stronger coverage. In PR #196, `connection_attempts_total >= 2` is what *revealed* that the "down" endpoint was never dialed, and `connections_established_total == before + 1` is what revealed the pooled connection stayed alive. The mechanism assertion did double duty — it both hardened the good test and exposed the hollow one.

## When to Apply

Apply this whenever you write or review a test whose name or intent references a fault-tolerance behavior:

- Retry / reconnect-with-backoff loops
- Failover / routing-around-a-bad-endpoint
- Circuit breaker open/half-open/closed transitions
- Connection-pool reuse with stale-connection fallback
- Fallback-to-default / degraded-mode paths
- Timeout-then-recover, cancellation-then-cleanup

Trigger signs you are at risk: the only assertions are `result.success` / `result.is_ok()` / a returned value; the test name promises a failure path but the body never inspects counters, health, or breaker state; or the test relies on a "server is down" / "connection is dead" precondition you have not *verified* actually holds at the moment of the send.

Do **not** force it: if the failure precondition cannot be created deterministically with the current API (e.g. no cross-platform way to force-kill an accepted connection), the correct move is to delete the test and validate the production code by review — not to ship a test that passes for the wrong reason.

## Examples

**Hollow failover test — outcome only (before):**

```rust
// "down" has NO server and was never negotiated, so refresh_capabilities()
// culls it before selection. Routing picks "up" DIRECTLY; the failover
// branch never runs. This still passes — it proves nothing about failover.
let result = client.send_task(task).await.expect("should succeed");
assert!(result.success);
assert_eq!(result.task_id, "failover-test");
```

**Genuine failover test — mechanism asserted (after):**

```rust
// Pre-set capabilities on "down" so it is SELECTABLE as a healthy,
// task-compatible primary (no live server needed). It then fails on the
// actual send, forcing a genuine failover to the live "up" endpoint.
let mut down = CollectorEndpoint::new("down".to_string(), down_config.endpoint_path.clone(), 1);
down.update_capabilities(process_capabilities());
let mut up = CollectorEndpoint::new("up".to_string(), up_config.endpoint_path.clone(), 2);
up.update_capabilities(process_capabilities());

let result = client.send_task(task).await.expect("failover should succeed");
assert!(result.success);

// Prove the failover machinery actually ran (not "up" picked directly):
// "down" was dialed before the successful "up" dial...
assert!(
    client.metrics().connection_attempts_total.load(Ordering::Relaxed) >= 2,
    "failover should have retried the down endpoint before succeeding on up"
);
// ...and "down" is now marked unhealthy by the failure.
let stats = client.get_stats().await;
let down_healthy = stats.endpoint_stats.iter()
    .find(|e| e.endpoint_id == "down").expect("down present").is_healthy;
assert!(!down_healthy, "down endpoint should be unhealthy after the failed primary attempt");
```

**Hollow stale-connection test — outcome only (before):**

```rust
// Pool a connection, "shut down" the server, restart on the same path,
// then send again and expect the stale-connection fallback to re-dial.
server.graceful_shutdown().await;          // does NOT close the accepted fd
let mut new_server = echo_server(&config); // restart on same socket
new_server.start().await.unwrap();
let second = client.send_task(task2).await.expect("should recover");
assert!(second.success);                   // passes — but on Unix the OLD
                                           // pooled connection served it;
                                           // the fallback never ran.
```

**Mechanism assertion that exposed it (and the resolution):**

```rust
// The intended proof: a FRESH connection was established (the pooled one
// was discarded). This FAILED — left: 1, right: 2 — revealing no new
// connection was made, because graceful_shutdown() left the accepted
// connection alive and it served the second request.
assert_eq!(
    client.metrics().connections_established_total.load(Ordering::Relaxed),
    established_before + 1,
    "a fresh connection should be established after the pooled one goes stale",
);
```

Resolution for the stale-connection case: there is no cross-platform server API to force-kill an *already-accepted* connection, so a deterministic stale-connection integration test is not feasible with the current API. The hollow test was **removed**; the production stale-connection fallback in `send_task_to_endpoint` (`client.rs`) stays as correct defensive code, validated by review rather than by a false-confidence test.

The unifying rule across both: **if a test can reach the right outcome via the wrong path, the assertion is too weak.** Assert attempt counts, fresh-connection counts, and health/breaker-state transitions — the fingerprints of the mechanism — so the test fails loudly when the mechanism does not run.

## Related

- [ipc-backbone-actual-state-transport-duality.md](../architecture-patterns/ipc-backbone-actual-state-transport-duality.md) — the IPC backbone inventory that explains *why* these resilience mechanisms behave as they do (the `ResilientIpcClient` chain is complete but has no v1.0 production caller; capability negotiation is a stub). Read it to understand the `refresh_capabilities` pre-cull and the server transport behavior referenced above.
- [tee-masks-exit-code-false-green-2026-06-09.md](../workflow-issues/tee-masks-exit-code-false-green-2026-06-09.md) — a sibling "false green" lesson: a shell pipeline masking a real exit code. Same theme from a different angle — a check that reports success without actually verifying what it claims.
