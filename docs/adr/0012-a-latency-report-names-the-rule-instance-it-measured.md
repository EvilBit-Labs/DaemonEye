# ADR-0012: A latency report names the rule instance it measured

**Date**: 2026-10-04 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

`DetectionEngine::observe_pattern_latency(rule_id, observed)` identifies its subject by rule id alone. A rule id is stable across reloads: `load_rule` replaces a rule's SQL, re-plans it, and forgets its health, which is the one operation that clears a latency verdict (ADR-0011's sibling consequence, R8).

Those two facts admit a stale report. The agent holds the engine behind a lock it deliberately releases around execution — the call site says so — so a measurement is taken while the lock is down and reported after re-acquiring it. An operator who fixes a slow pattern and reloads the rule in that window gets the fresh instance disabled for the old instance's cost, with a reason string describing a pattern the rule no longer has.

The operator's only recovery from a latency verdict is to reload the rule. This defeats that recovery, and it does so in exactly the window where the operator is using it.

## Decision

A latency report must identify the **rule instance** it measured, not just the rule. The engine issues a generation with the compiled plan it hands the executor; the executor returns that generation with the measurement; a report whose generation does not match the rule's current generation is discarded and logged.

`load_rule` bumps the generation, so a reload invalidates every measurement already in flight against the instance it replaced.

## Alternatives Considered

### Alternative 1: Leave the signature alone and accept the race

- **Pros**: no plumbing; the window is small and needs an operator acting inside it
- **Cons**: the window is not rare in the case that matters. The operator is *reloading a rule the guard disabled*, which is precisely when a measurement of the old pattern is in flight. The failure also looks like the engine refusing to accept a fix, which is the worst possible reading for an operator under pressure
- **Why not**: a recovery path that fails when exercised is not a recovery path. Rarity does not help when the trigger is the recovery itself

### Alternative 2: Have the engine timestamp the plan and compare times

- **Pros**: no new identifier; `SystemTime` is already threaded through this subsystem
- **Cons**: correctness would rest on clock behaviour, and this repository already carries a learning about a real clock drifting. Two reloads inside one clock tick are indistinguishable
- **Why not**: a monotonic counter answers the only question being asked — "is this the instance I handed out?" — without inheriting a clock's problems

### Alternative 3: Refuse to clear a verdict while any measurement is outstanding

- **Pros**: no generation; the engine simply will not reload mid-flight
- **Cons**: it inverts the fix. The operator is now blocked by in-flight work they cannot see, and the engine must track outstanding measurements to know when it is safe
- **Why not**: it protects the verdict at the operator's expense, when the verdict is the thing that should yield to a deliberate reload

## Consequences

### Positive

- A stale report becomes unattributable rather than merely unlikely. The engine cannot apply a measurement to an instance it was not taken against, which is the "make the wrong thing unrepresentable" shape this subsystem's type review argued for where it is cheap.
- The generation is opaque to the executor and obtained from the engine, so a caller cannot fabricate one that would latch an arbitrary instance.
- Reload keeps its meaning as a fresh judgment, which is what makes it the single clearing point.

### Negative

- `observe_pattern_latency`'s signature gains a parameter. It has no production callers today, so the cost is paid entirely by T6, which is also the party that benefits.
- The engine must carry a per-rule counter and hand it out with the plan — a small amount of state whose only purpose is to be compared.

### Risks

- The generation is only as good as its issuance point. If T6 obtains a plan once and reports many measurements against it across reloads, the guard holds; if it caches a generation and reuses it after re-fetching a plan, it does not. T6 should take the generation from the same call that yields the plan, never store it separately.
- A discarded stale report means a genuine breach can be missed when it arrives just after a reload. That is the correct trade — the new instance has not been measured, so it has not earned a verdict — but it means a slow pattern surviving a reload is detected on the next cycle rather than immediately.
