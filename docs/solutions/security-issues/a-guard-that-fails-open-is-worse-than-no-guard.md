---
title: A Tamper Check Whose Failure Path Disables the Control It Guards (T5 spawn-token admission)
category: security-issues
date: 2026-09-22
tags:
  - rust
  - fail-open
  - authentication
  - composition-root
  - option-default
  - spawn-token
  - collector-registration
  - review-findings
module: daemoneye-agent (broker_manager, collector_registry) / daemoneye-eventbus (process_manager::spawn_token)
symptom: |
  Collector registration accepted every request with no spawn-token verification whenever the
  spawn-token store failed to open. Two startup assertions existed to catch mis-wiring and neither
  fired, because both lived inside the branch where the store had opened successfully. No test,
  no clippy lint, and no `just ci-check` run detected it; the suite was fully green.
root_cause: |-
  The admission gate was modelled as `Option<Arc<CollectorAdmission>>` with a `Default` impl
  producing `None`, and the call site was `if let Some(ref admission) = self.admission { ... }`
  with no `else`. Absent gate therefore meant "skip the check" rather than "refuse". The store's
  own tamper detection (refusing to open a group- or world-reachable token directory) became the
  trigger that produced the absent gate, so loosening permissions on the credential directory
  disabled authentication instead of stopping the agent.
---

# A Tamper Check Whose Failure Path Disables the Control It Guards

## Problem

T5 added spawn-token authentication for collector registration: the agent mints a 32-byte token per spawn into an owner-only file, the collector presents the value, and the agent verifies it in constant time before any descriptor enters the schema catalog. The verification itself was sound — constant-time comparison, an unforgeable `VerifiedRegistration` newtype with a private constructor, atomic owner-only file creation with no chmod window.

The hole was in what happened when the store could not be opened at all:

```rust
// broker_manager/mod.rs
match SpawnTokenStore::new(directory) {
    Ok(store) => Some(Arc::new(store)),
    Err(error) => { tracing::error!(...); None }   // -> collector_admission = None
}

// collector_registry.rs
if let Some(ref admission) = self.admission {
    admission.admit(&request).await?;
}            // <- no else: absent gate means every registration is accepted
```

Every `SpawnTokenStore::new` failure — `InsecureDirectory`, ENOSPC, EMFILE, a read-only filesystem, `NoPrivateRoot` off Unix — produced a running agent that accepted **any** local process registering as `procmond`, pushing a `SchemaDescriptor`, taking ownership of the `processes` table, and becoming the sink for every pushed detection plan.

The worst property is which error triggers it. `SpawnTokenStore::new` returns `InsecureDirectory` exactly when the token directory is group- or world-reachable — the store's own tamper detection. So the response to *evidence of tampering* was to switch authentication off.

## Root Cause

Two independently reasonable decisions combined into a failure:

1. **`Option<Gate>` with a `Default` producing `None`.** In Rust this is idiomatic and reads as safe. For a security control it encodes "absent means skip," which is the wrong default for anything whose job is to refuse.
2. **Assertions placed inside the success branch.** The composition root carried two `anyhow::ensure!` checks — that the gate verifies against the same store the process manager mints into, and that it feeds the same detection engine the planner reads. Both are good checks. Both were inside `Some(ref admission) => { ... }`, so the path with no gate at all had no assertion on it.

The code comment described the behaviour accurately and still read as reassuring:

> A failure here is logged and yields `None`: the agent still starts, but with **no** collector authenticated rather than with collectors authenticated against a store only one half of the system can see.

That sentence states the control is off. It was read during implementation and during review as a safety note, because it is phrased as a trade-off against a worse alternative.

## Fix

Make the unauthenticated state impossible to construct by accident, and make absent-gate mean *refuse*:

```rust
enum AdmissionPolicy {
    Gated(Arc<CollectorAdmission>),
    Closed,            // refuse and record every registration
    Unauthenticated,   // test-only, named, never produced by production code
}
```

- The `Default` impl is **deleted**. There is no longer a construction path that does not name which policy it meant.
- The dispatch `match` has **no wildcard arm**, so a fourth state is a compile error rather than a silent "do not authenticate."
- Every store-open failure now builds `CollectorRegistry::closed()`, which refuses with `RegistrationGate::NoTokenPresented` and writes a rejection record.
- The previously unguarded branch gained the assertion it was missing.

## Why this generalises

Three properties made this invisible to every automated gate:

- **The failing path is the error path.** Tests exercise the success path; the suite was green at 1927 passing tests with the hole present.
- **The control's absence is not a type error.** `Option<Gate>` compiles fine with no `else`.
- **The trigger is another security check.** Hardening detection and the fail-open path shared a code path, so the system's response to tampering evidence was to reduce its own defences.

Checks worth applying to any guard:

1. **Ask what happens when the guard cannot be constructed.** Not what happens when it rejects — what happens when it is absent. If the answer is "the operation proceeds," it is fail-open.
2. **Put the invariant assertion on the branch that lacks the guard**, not only on the branch that has it. An assertion inside `Some(..)` never fires on `None`.
3. **Prefer a named state over an `Option`** for a security control, and delete the `Default`. The compiler then requires every construction site to state its policy.
4. **Read the failure-mode comment as a specification.** "The agent still starts, but with no collector authenticated" is not a mitigation; it is the vulnerability, written down.

## Related

- [`binary-hashing-authorization-and-toctou-fixes.md`](binary-hashing-authorization-and-toctou-fixes.md) — the same class, one layer down: `--compute-hashes` was a silent no-op because no composition site injected the hasher. T5's startup assertions were written *because* of that defect, and they still missed this one by being placed on the wrong branch.
- [`shared-lib-placement-does-not-grant-call-rights.md`](../architecture-patterns/shared-lib-placement-does-not-grant-call-rights.md) — the privilege boundary this registration path sits on.
