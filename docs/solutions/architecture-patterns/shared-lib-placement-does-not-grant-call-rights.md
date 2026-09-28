---
title: A primitive living in the shared library does not mean your component may call it
date: 2026-09-21
category: docs/solutions/architecture-patterns
module: daemoneye-lib (shared primitives)
problem_type: architecture_pattern
component: service_object
severity: high
applies_when:
  - adding the first production caller of a `daemoneye-lib` type that currently has none
  - implementing a requirement that says an event must be audited, recorded, or persisted
  - a shared type exposes a mutating method with no component-ownership check in its signature
  - planning work that spans procmond, daemoneye-agent, and daemoneye-cli
tags:
  - privilege-separation
  - audit-ledger
  - component-boundaries
  - shared-library
---

# A primitive living in the shared library does not mean your component may call it

## Context

Planning T5 produced two requirements saying rule-load and registration rejections must be audited. Both events happen in `daemoneye-agent`. The obvious implementation was to call `AuditLedger` (`daemoneye-lib/src/crypto.rs:172`): it is public, it exposes `add_entry` (`:187`), it hash-chains entries exactly the way an audit trail should, and it sits in the shared library both components already depend on.

It also has zero production callers. A `grep` for `AuditLedger` across the workspace returns its own definition, its own unit tests, and prose in specs — nothing else. The plan initially made the agent its first caller, and a scope review caught that this inverts the privilege boundary the whole product rests on.

## Guidance

**Check the ownership table before adding the first caller of a shared primitive.** `daemoneye-lib` is a dependency of every component, so placement there says nothing about who may invoke what. Ownership is declared separately:

- `AGENTS.md:407` — `audit_ledger | W (procmond), R (others) | procmond`
- `AGENTS.md:119-121` — procmond is `Write-only (audit)`; the agent is `Read/write (events)`; the CLI is `Read-only`
- `spec/full/specs/Tech_Plan_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md:77` — "The ledger file remains procmond-owned for writes; the agent opens it read-only and serves CLI audit requests over IPC."

Three documents agree, and none of that constraint is expressed in the type. `AuditLedger::add_entry` takes `&mut self` and an actor string; nothing in its signature, its module, or its doc comment stops the agent from calling it.

**Zero callers is a signal to slow down, not a blank slate.** A shared type with no production caller is ambiguous: it may be genuinely unused, or it may be reserved for a component that has not implemented its half yet. `AuditLedger` is the second kind — T8 owns persisting it, from procmond. Treating "nothing calls this" as "nothing constrains this" is what turned a correct-looking implementation into a privilege inversion.

**When the audit-shaped requirement belongs to the wrong component, record where you are allowed to write.** The agent keeps its own hash-chained record of its own rejections, and stays a reader of `audit_ledger`. Routing agent-side rejections through procmond just to reach the ledger would invert the boundary in the other direction for no forensic gain — procmond would be attesting to events it never observed.

## Why This Matters

Privilege separation is the property DaemonEye sells. procmond is the only component that runs elevated and the only writer to the forensic chain, which is what makes the chain worth trusting: a compromised agent cannot forge history it has no write path to. An agent that writes to `audit_ledger` silently removes that guarantee while every test still passes, because nothing in the code encodes the rule the architecture depends on.

The failure is quiet in review, too. "Rejections are written to the audit trail" reads as obviously correct, and the implementation that satisfies it reads as obviously correct. Only the component boundary makes it wrong, and the boundary lives in a table three documents away from the code.

## When to Apply

Apply before the first production call of any `daemoneye-lib` type that currently has none, and whenever a requirement says an event must be audited, recorded, or persisted without naming which component does the writing. Apply when a plan or ticket assigns storage work to a component and the table in `AGENTS.md` assigns that table to a different one.

Do not apply to shared primitives with no ownership dimension — hashing, parsing, serialization, and validation helpers are called from anywhere by design.

## Examples

The implementation that looks right and inverts the boundary:

> **Approach:** Introduce the first production caller of `AuditLedger` and route rule-load rejections and registration rejections to it.

The correction, which keeps the same durable-record property without crossing the boundary:

> **Approach:** Record rejections in the agent's own store, hash-chained the way `AuditLedger` chains entries. The `audit_ledger` table is procmond's to write and everyone else's to read, so the agent recording its own rejections there would break privilege separation.

A cheap guard when a component genuinely must be prevented from reaching a primitive: give the type a constructor only the owning component can satisfy, the same newtype-with-private-constructor shape that closed a prior authorization defect in this repo ([binary hashing authorization](../security-issues/binary-hashing-authorization-and-toctou-fixes.md)). The compiler then carries the rule the table states.

## Related

- [Binary Hashing P1 Blockers](../security-issues/binary-hashing-authorization-and-toctou-fixes.md) — the same class of defect from the other direction: an authorization gate that existed but was never wired at the composition root, plus the newtype-receipt pattern that fixed it.
- [IPC backbone actual state](ipc-backbone-actual-state-transport-duality.md) — another case where the shipped code and the documented architecture describe different things, and the code is the one that surprises you.
