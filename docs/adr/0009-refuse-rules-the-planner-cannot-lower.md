# ADR-0009: Refuse a rule the planner cannot lower

**Date**: 2026-09-27 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

The planner lowers each detection rule into a pushed half — a typed predicate conjunction plus a projection, addressed to the owning collector — and a residual half the agent keeps. It reads exactly two fields of the parsed `Select`: `projection` and `selection`.

A valid `SELECT` can carry a great deal more. `HAVING`, `QUALIFY`, `DISTINCT`, `ORDER BY`, `LIMIT`, `FETCH`, `TOP`, `PREWHERE`, `SORT BY`, `CLUSTER BY`, a `WITH` clause, and an aggregate in the projection were all accepted by validation and then dropped on the floor, while `CompiledRule::residual()` still reported a complete split.

Probed against a real catalog, each of these loaded clean and produced a different row set than the operator asked for:

- `SELECT pid FROM processes WHERE pid = 1 LIMIT 1` compiled to every matching process.
- `SELECT count(pid) FROM processes WHERE pid > 1` reduced to `projection: ["pid"]`, returning rows where a count was requested.
- `WITH processes AS (SELECT pid FROM processes WHERE pid > 100) SELECT pid FROM processes WHERE pid = 1` planned against the catalog's real `processes` and discarded the CTE's own filter entirely.
- `regexp(name, '(?=x)')` skipped load-time compilation, because the pattern collector only matched the operator spelling while the allowlist admitted the function spelling — one this repository's own tests use.

This is the opposite direction from the failure the split rule guards. Requirement R14 is careful never to *drop* a matching row; nothing guarded against *admitting* rows the rule excluded. Every design review had gone into one direction, so the other stayed invisible — and invisible is the operative word: no error, no log line, and a green test suite.

## Decision

A rule carrying any construct the planner cannot represent is **refused at load**, with an error naming the construct. This is R17 (`a rule that cannot be lowered into tasks plus a residual is rejected on first load, with no fallback`) applied to the whole statement rather than only to the pushed/residual split.

The gate **destructures `Query` and `Select` naming every field, with no rest pattern**, and refuses everything the planner does not read. A field added by a future `sqlparser` release is therefore a build break rather than a silent addition to the dropped set.

## Alternatives Considered

### Alternative 1: Carry the unrepresentable parts into the residual

- **Pros**: no rule is refused; the residual exists precisely to hold what the collector cannot evaluate, so this reads like the natural home
- **Cons**: the residual is rendered SQL evaluated over the event store, while the pushed half runs against live collector state. `LIMIT`, `DISTINCT` and an aggregate mean different things over those two row sources, and the executor that would settle the semantics does not exist yet (ADR-0006, T6)
- **Why not**: it would ship a semantic guess as behavior, in the one place where being wrong is silent. Refusing is reversible when T6 lands; a wrong lowering that operators have written rules against is not

### Alternative 2: Keep dropping them and document the limitation

- **Pros**: no work, no rules refused
- **Cons**: a detection rule that silently matches a different row set is a false-negative generator in a security product, and the failure is invisible to operator and test alike
- **Why not**: "documented" does not help an operator who reads the rule they wrote and believes it

### Alternative 3: Enumerate the rejected clauses instead of destructuring

- **Pros**: a smaller, more readable diff; rejects exactly what was reported
- **Cons**: probing the AST found nine further spellings beyond the six first reported — `TOP` and `FETCH FIRST` are `LIMIT` under other keywords, `PREWHERE` is a second filter expression
- **Why not**: an enumeration is a list someone must remember to extend, and the first version of it was already incomplete

## Consequences

### Positive

- No accepted rule can match a row set it did not ask for, in either direction.
- A new `sqlparser` field or variant fails the build instead of silently widening what is dropped.
- The compiled rule's claim about its own completeness is now true, which matters because T6 is specified to consume it.

### Negative

- Rules an operator could previously load now fail, including any using `GROUP BY`/`HAVING`, `ORDER BY`, `LIMIT` or `DISTINCT`.
- `avg`, `count`, `max`, `min` and `sum` left the function allowlist, because a projected aggregate lowers to row output and a threshold on an aggregate is a `HAVING`. The allowlist's stated rationale had been exactly those "threshold rules", so the justification was removed with the names.
- A CTE is refused by the planner even though the validation gate still accepts one — deliberate, because three validation tests exist to prove the gate reaches constructs hidden inside a CTE body, and refusing `WITH` there would delete that coverage.

### Risks

- The spec and T6's ticket describe aggregate rules as planned work, delivered through the DataFusion executor with explicit time windows. Refusing them is correct for a planner that cannot lower them, and is not a decision against aggregates. Both documents now say so, and name the allowlist plus this clause gate as the two places the support returns — alongside a planner that can represent a windowed aggregate.
- Operator-facing documentation listed the aggregates as supported with worked examples. That reference was corrected with this change; a future capability change has to move both.
