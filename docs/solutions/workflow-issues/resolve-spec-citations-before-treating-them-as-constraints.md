---
title: Resolve a spec citation before you treat it as a constraint
date: 2026-09-21
category: docs/solutions/workflow-issues
module: repository-hygiene
problem_type: workflow_issue
component: documentation
severity: high
applies_when:
  - planning or implementing from a ticket in spec/full/tickets/
  - a requirement defers a value, bound, or rule to another section by number
  - a ticket names a source file as the place to hook new work
  - a doc states a dependency version you are about to rely on
  - a citation resolves to nothing and you are deciding whether that is a gap or an external reference
tags:
  - spec-drift
  - planning
  - citations
  - tickets
---

# Resolve a spec citation before you treat it as a constraint

## Context

Planning ticket T5 meant reading its requirements, its Tech Plan references, and its enumerated touchpoints, then building a plan on them. Four of those pointers did not resolve against the tree, and each one was found only because something forced the check — a reviewer doing arithmetic, a researcher opening the file, a grep that came back empty.

None of them looked wrong on the page. Every one reads as a precise, confident reference.

## Guidance

**Resolve every citation you intend to build on, before it becomes a constraint in your plan.** Four shapes showed up in one ticket:

| Citation                             | Where                                                       | What it actually is                                                                 |
| ------------------------------------ | ----------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| "bounds per spec §4.2/§4.5"          | `.kiro/specs/daemoneye-core-monitoring/requirements.md:237` | No such section exists in the repo. A requirement deferring its numbers to nothing. |
| `collector-core/src/rpc_services.rs` | T5 ticket, and `T2` ticket line 22                          | A directory since June, not a file. Two tickets carry the stale shape.              |
| `ResilientIpcClient` as the IPC hook | T5 ticket                                                   | Real code with zero production callers; the live path is elsewhere.                 |
| redb `3.0+`                          | `AGENTS.md:174`                                             | `Cargo.toml:117` pins `4.2.0`. The canonical tech table is a major version behind.  |

The failure modes differ and so do the responses. A missing section means the requirement has no stated bound and you must supply one and say that you did. A stale path is cosmetic and costs a minute. A pointer at uncalled code is the dangerous one, because the code compiles, reads correctly, and is simply not what runs — planning against it produces work that integrates with nothing.

**Check the Source-of-Truth Map before calling a citation broken.** `ADR-0006` is cited in `AGENTS.md`, `BACKLOG.md`, the SQL-to-IPC spec, and the T4 ticket, and no such file exists under `docs/adr/`. That is not drift: `AGENTS.md:591` states that ADR-0001 through ADR-0007 live in the Confluence ES space and are cited by number only. An unresolvable citation is a question, not a verdict, and this repo keeps the answer in one place.

**Say so in the plan when a citation did not resolve.** A requirement whose bound is missing should not silently acquire one. Record that the section is absent, that the values are this project's own, and what they are — otherwise the next reader inherits invented numbers wearing the authority of a spec reference.

## Why This Matters

The cost is asymmetric. Verifying a citation is a grep; acting on a wrong one propagates. The `ResilientIpcClient` pointer nearly routed a whole ticket's protobuf and registration work onto a dormant transport — recoverable during planning, expensive after implementation. The missing `§4.2` nearly shipped a memory ceiling that nothing could enforce ([unmeasurable ceilings](../best-practices/unmeasurable-memory-ceilings-are-not-bounds.md)).

There is also a compounding effect: a stale pointer copied between tickets gains credibility from repetition. `rpc_services.rs` appears as a file in two tickets, which makes it look verified rather than duplicated.

## When to Apply

Apply when a ticket names the files you will modify, when a requirement defers a value to another section, and when a doc states a version you intend to rely on. Prefer resolving in bulk at the start of planning — a handful of `ls` and `grep` calls against the ticket's enumerated touchpoints — rather than discovering each one when it blocks you.

Do not apply to prose references that shape understanding but that nothing will be built on. The trigger is that a citation is about to become a constraint.

## Examples

Cheap, and it catches all four shapes:

```bash
# paths the ticket names
ls -d collector-core/src/rpc_services* daemoneye-lib/src/detection/sql_to_ipc.rs

# a section a requirement defers to
grep -rn '§4\.2' --include=*.md .

# is the code the ticket points at actually called?
grep -rn 'ResilientIpcClient' --include=*.rs --exclude-dir=target \
  daemoneye-agent/src procmond/src

# does a stated version match the pin?
grep -n '^redb' Cargo.toml
```

The third one is the one worth running even when nothing seems wrong. An empty result there means the ticket is pointing at code that does not execute, which no amount of reading the code itself will reveal.

## Related

- [A memory ceiling you cannot measure is not a bound](../best-practices/unmeasurable-memory-ceilings-are-not-bounds.md) — what the missing `§4.2` reference nearly produced downstream.
- [IPC backbone actual state](../architecture-patterns/ipc-backbone-actual-state-transport-duality.md) — the standing record of which transport is live, and the doc to read before trusting any ticket's IPC pointer.
