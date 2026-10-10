# Architecture Decision Records

Decisions recorded in this repository. Each ADR states the context, the decision, the alternatives that were rejected and why, and the consequences.

## Relationship to Confluence

**ADR-0001 through ADR-0007 live in the Confluence ES space, not here.** ADR-0006 (Apache DataFusion over redb `TableProvider`s) and ADR-0007 were authored there, and the repo references them by number — see the Source-of-Truth Map in [AGENTS.md](../../AGENTS.md). Per the open-core hygiene rules, this repo does not carry internal Confluence hyperlinks, so those ADRs are cited by number alone.

This directory continues that sequence rather than restarting it, so an ADR number means one thing regardless of where the document lives. The next ADR recorded here takes the next unused number.

## Index

| ADR                                                                    | Title                                                                | Status   | Date       |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------- | -------- | ---------- |
| 0001–0005                                                              | *(Confluence ES space)*                                              | —        | —          |
| 0006                                                                   | Apache DataFusion over redb `TableProvider`s *(Confluence ES space)* | accepted | —          |
| 0007                                                                   | *(Confluence ES space)*                                              | —        | —          |
| [0008](0008-bucket-at-a-time-detection-reads.md)                       | Read detection windows bucket-at-a-time                              | accepted | 2026-09-20 |
| [0009](0009-refuse-rules-the-planner-cannot-lower.md)                  | Refuse a rule the planner cannot lower                               | accepted | 2026-09-27 |
| [0010](0010-reserved-table-names-bind-to-an-authenticated-identity.md) | Reserved table names bind to an authenticated collector identity     | accepted | 2026-09-27 |
| [0011](0011-the-latency-budget-detects-a-breach-between-batches.md)    | The latency budget detects a breach between batches                  | accepted | 2026-10-04 |
| [0012](0012-a-latency-report-names-the-rule-instance-it-measured.md)   | A latency report names the rule instance it measured                 | accepted | 2026-10-04 |
| [0013](0013-the-provider-reports-inexact-filter-pushdown.md)           | The provider reports Inexact filter pushdown, never Exact            | accepted | 2026-10-10 |
| [0014](0014-a-cycle-evaluates-its-own-window.md)                       | A detection cycle evaluates the rows its own window covers           | accepted | 2026-10-10 |
