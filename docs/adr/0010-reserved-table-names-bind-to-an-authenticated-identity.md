# ADR-0010: Reserved table names bind to an authenticated collector identity

**Date**: 2026-09-27 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

A collector authenticates to the agent with a per-spawn token, then registers a `SchemaDescriptor` declaring the tables it serves. The catalog records one owner per table name, and the planner addresses each rule's pushed half to `owner_of(table)`.

Registration originally inserted that mapping unconditionally. Any authenticated collector could therefore declare a table another collector already owned and become its owner, which re-pointed every pushed plan for that table. `collector-core` is the published SDK for third-party collectors, so the adversary here is not hypothetical: a lower-trust collector holding a valid token for its *own* identity could take over `processes` and become the source of the detection results the privileged process collector was supposed to answer. A test asserted the takeover as expected behavior, so the suite endorsed it.

Refusing a takeover closed that, and inverted the same problem. Ownership became first-claim, which means a third-party collector that registers *before* `procmond` locks it out of `processes` permanently — a race deciding a privilege boundary instead of an authorization rule deciding it.

The underlying gap in both shapes is the same: the spawn token proves *which* collector is speaking. It says nothing about *which* tables that collector is entitled to serve.

## Decision

Reserved table names may be owned **only** by the collector identity they are reserved for, regardless of registration order, and the reservation is keyed on the token-authenticated `collector_id`. A non-designated claimant is refused even when the reserved owner has never registered; the designated owner can always claim its name.

Unreserved names keep the first-claim rule with takeover refused.

## Alternatives Considered

### Alternative 1: Key the reservation on `collector_type`

- **Pros**: reads as the natural grouping — the reservation is conceptually about what kind of collector this is, and `RegistrationRequest` already carries the field
- **Cons**: `collector_type` is a free string on the wire that nothing verifies, and `RegistrationRequest` even defaults it to the collector id
- **Why not**: any third-party collector could send `collector_type: "procmond"` and claim the reserved table. It would have produced a reservation that read correctly and enforced nothing — worse than no reservation, because it would look handled. The spawn token authenticates the id, and `design.md` reserves identities, so the id is both the secure key and the one the spec already asserts

### Alternative 2: Leave ownership first-claim

- **Pros**: no policy to maintain, no static map to grow
- **Cons**: registration order decides who serves the privileged table; a third-party collector that starts first displaces `procmond` with no recourse short of operator intervention
- **Why not**: startup ordering is not an authorization mechanism

### Alternative 3: Reject a reserved identity at token issuance instead

- **Pros**: stops a reserved identity earlier, at the point the agent mints credentials
- **Cons**: the agent only mints tokens for processes it spawns from its own configured binaries, so this addresses operator misconfiguration rather than a hostile collector, which is a different threat
- **Why not**: complementary rather than substitutable. `SpawnTokenStore::issue` accepts any id string today with no reserved-identity check, despite `design.md` stating that dynamic registration of a reserved identity is rejected — recorded here as remaining work rather than as the fix for this problem

## Consequences

### Positive

- `procmond` cannot be locked out of `processes` by registration order, and cannot have it taken away mid-flight.
- The reservation rests on the only identity the spawn token actually proves, so it cannot be spoofed by a claim in the registration payload.
- The two refusals are distinguishable to an operator: *owned* means the name was taken first and the rightful owner may simply not have registered yet; *reserved* means this claimant was never entitled to it.
- `VerifiedRegistration` keeps its single meaning — an authenticated identity with a private constructor — because keying on the id required no new field to reach the check.

### Negative

- A static reserved map has to grow as first-party collectors appear. It holds one entry today, because `procmond` is the only collector that declares a table; `netmond`, `fsmond` and `perfmond` are named as reserved identities but declare nothing yet, so no table names were invented for them.
- The rejection record's gate still collapses every descriptor refusal to one value, so the distinction above is visible in the error message but not in the recorded gate.

### Risks

- The reserved id and table name are needed by both `daemoneye-lib` (which enforces the reservation) and `procmond` (which declares the table), and duplicating either literal would mean a future change silently refusing `procmond` its own table. Closed by making `daemoneye-lib` the single authority for both constants, with `procmond` deriving its `DEFAULT_COLLECTOR_ID` and `PROCESS_TABLE` from them — drift is now a compile error rather than a runtime refusal.
- The reservation assumes the agent never spawns a non-first-party binary under a reserved id. That is an operator-configuration property, not an enforced one, until the token-issuance check in Alternative 3 exists.
