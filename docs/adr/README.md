# Architecture Decision Records

Decisions recorded in this repository. Each ADR states the context, the decision, the alternatives that were rejected and why, and the consequences.

## Relationship to Confluence

**ADR-0001 through ADR-0007 live in the Confluence ES space, not here.** ADR-0006 (Apache DataFusion over redb `TableProvider`s) and ADR-0007 were authored there, and the repo references them by number — see the Source-of-Truth Map in [AGENTS.md](../../AGENTS.md). Per the open-core hygiene rules, this repo does not carry internal Confluence hyperlinks, so those ADRs are cited by number alone.

This directory continues that sequence rather than restarting it, so an ADR number means one thing regardless of where the document lives. The next ADR recorded here takes the next unused number.

## Index

| ADR                                              | Title                                                                | Status   | Date       |
| ------------------------------------------------ | -------------------------------------------------------------------- | -------- | ---------- |
| 0001–0005                                        | *(Confluence ES space)*                                              | —        | —          |
| 0006                                             | Apache DataFusion over redb `TableProvider`s *(Confluence ES space)* | accepted | —          |
| 0007                                             | *(Confluence ES space)*                                              | —        | —          |
| [0008](0008-bucket-at-a-time-detection-reads.md) | Read detection windows bucket-at-a-time                              | accepted | 2026-09-20 |
