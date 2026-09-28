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
  - a citation resolves to nothing and you are deciding whether that is a gap, an external reference, or a bad search pattern
tags:
  - spec-drift
  - planning
  - citations
  - tickets
---

# Resolve a spec citation before you treat it as a constraint

## Context

Planning ticket T5 meant reading its requirements, its Tech Plan references, and its enumerated touchpoints, then building a plan on them. Three of those pointers did not resolve against the tree. A fourth appeared not to, and that one was the expensive mistake: the citation was fine and the search for it was wrong, which was caught only after the conclusion had been written into a requirement.

None of them looked wrong on the page. Every one reads as a precise, confident reference — including the one that was.

## Guidance

**Resolve every citation you intend to build on, before it becomes a constraint in your plan.** Three real drifts showed up in one ticket:

| Citation                             | Where                              | What it actually is                                                                |
| ------------------------------------ | ---------------------------------- | ---------------------------------------------------------------------------------- |
| `collector-core/src/rpc_services.rs` | T5 ticket, and `T2` ticket line 22 | A directory since June, not a file. Two tickets carry the stale shape.             |
| `ResilientIpcClient` as the IPC hook | T5 ticket                          | Real code with zero production callers; the live path is elsewhere.                |
| redb `3.0+`                          | `AGENTS.md:174`                    | `Cargo.toml:117` pins `4.2.0`. The canonical tech table is a major version behind. |

The failure modes differ and so do the responses. A stale path is cosmetic and costs a minute. A pointer at uncalled code is the dangerous one, because the code compiles, reads correctly, and is simply not what runs — planning against it produces work that integrates with nothing.

**A citation that does not resolve is a question, not a verdict — and your own search is a suspect.** Two cases in this ticket looked like drift and were not.

`ADR-0006` is cited in `AGENTS.md`, `BACKLOG.md`, the SQL-to-IPC spec, and the T4 ticket, and no such file exists under `docs/adr/`. `AGENTS.md:591` explains it: ADR-0001 through ADR-0007 live in the Confluence ES space and are cited by number only. Check the Source-of-Truth Map before concluding anything.

R17 AC6's "bounds per spec §4.2/§4.5" was worse, because the failure was in the checking. A grep for the literal `§4.2` returns nothing, and that emptiness became a written conclusion that the section did not exist. It does: `spec/daemon_eye_spec_sql_to_ipc_detection_architecture.md:172` is `## 4.2) Minimum Contracts by Collector Type`, whose "Regex Implementation Requirements" subsection carries the exact per-pattern memory limit, latency bound, and LRU cache size the requirement defers to. The headings use `4.2)`; the citation uses `§4.2`. **Match the section number, not the glyph the citing document happens to use** — and when a search comes back empty on something that ought to exist, suspect the pattern before the tree.

Getting that backwards is expensive, because "it isn't there" licenses you to invent a replacement. Here it produced a requirement edit that displaced the project's own stated bound with a number from nowhere.

**Say so in the plan when a citation did not resolve.** A requirement whose bound is missing should not silently acquire one. Record that the section is absent, that the values are this project's own, and what they are — otherwise the next reader inherits invented numbers wearing the authority of a spec reference.

## Why This Matters

The cost is asymmetric in both directions. Verifying a citation is a grep; acting on a wrong one propagates. The `ResilientIpcClient` pointer nearly routed a whole ticket's protobuf and registration work onto a dormant transport — recoverable during planning, expensive after implementation.

Concluding drift that is not there costs more, because it converts straight into invented authority. Worth noting that §4.2's real bounds are themselves unenforceable against the resident budget ([unmeasurable ceilings](../best-practices/unmeasurable-memory-ceilings-are-not-bounds.md)) — so the section needed challenging on its merits, which is a more useful conversation than treating it as absent.

There is also a compounding effect: a stale pointer copied between tickets gains credibility from repetition. `rpc_services.rs` appears as a file in two tickets, which makes it look verified rather than duplicated.

## When to Apply

Apply when a ticket names the files you will modify, when a requirement defers a value to another section, and when a doc states a version you intend to rely on. Prefer resolving in bulk at the start of planning — a handful of `ls` and `grep` calls against the ticket's enumerated touchpoints — rather than discovering each one when it blocks you.

Do not apply to prose references that shape understanding but that nothing will be built on. The trigger is that a citation is about to become a constraint.

## Examples

Cheap, and it catches all four shapes:

```bash
# paths the ticket names
ls -d collector-core/src/rpc_services* daemoneye-lib/src/detection/sql_to_ipc.rs

# a section a requirement defers to — match the number, not the § glyph,
# and search headings across every spec file, not just the citing one
grep -rnE '^#+ *4\.2[).]' --include=*.md .

# is the code the ticket points at actually called?
grep -rn 'ResilientIpcClient' --include=*.rs --exclude-dir=target \
  daemoneye-agent/src procmond/src

# does a stated version match the pin?
grep -n '^redb' Cargo.toml
```

The third is worth running even when nothing seems wrong: an empty result means the ticket points at code that does not execute, which no amount of reading that code will reveal. The second is worth running twice, with different patterns, before you believe a negative.

## Related

- [A memory ceiling you cannot measure is not a bound](../best-practices/unmeasurable-memory-ceilings-are-not-bounds.md) — what the missing `§4.2` reference nearly produced downstream.
- [IPC backbone actual state](../architecture-patterns/ipc-backbone-actual-state-transport-duality.md) — the standing record of which transport is live, and the doc to read before trusting any ticket's IPC pointer.
