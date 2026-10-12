# ADR-0013: The provider reports Inexact filter pushdown, never Exact

**Date**: 2026-10-10 **Status**: accepted **Deciders**: UncleSp1d3r

## Context

The redb-backed `TableProvider` (ADR-0006) answers `supports_filters_pushdown` for each predicate DataFusion offers. It uses the predicate to prune buckets and to pick an index, which is most of the cost saving of a scan.

None of those lookups is row-exact. The name index keys a lowercase 128-bit hash, so `name = 'Bash'` admits a stored `bash`, and a hash collision admits a different name entirely. The time window is clamped at bucket and key granularity, not per row. A scan that applied such a filter has returned a superset of the matching rows.

DataFusion's contract for the answer is binary. `Exact` means the provider applied the filter fully, so the planner drops its own `FilterExec`. `Inexact` means the provider may have returned extra rows, so the planner keeps `FilterExec` above the scan and re-checks every row.

## Decision

The provider reports `TableProviderFilterPushDown::Inexact` for every predicate it consumes and `Unsupported` for the rest. It never reports `Exact`, and the provider's doc comment states this as an invariant.

## Alternatives Considered

### Alternative 1: Report `Exact` for predicates the index serves

- **Pros**: no `FilterExec` over those predicates, so one fewer pass over the batch; the plan looks tighter
- **Cons**: the saving is paid for with correctness. A filter the provider applied only approximately is no longer re-checked, so a row with a case-differing name or a colliding hash reaches the rule as a match
- **Why not**: a false match in a detection engine is an alert on something that did not happen, raised with no trace of why. The re-check is cheap against a decode that has already happened, and it is the only judge of a row

### Alternative 2: Make the index lookups exact

- **Pros**: `Exact` would then be true
- **Cons**: it needs the index to store the full value rather than a hash, or a verification read per hit, which is the re-check moved into the provider and made harder to see. The time window would need per-row filtering inside the scan
- **Why not**: it rebuilds what `FilterExec` already does, and it ties correctness to the provider's code instead of the planner's contract

## Consequences

### Positive

- A row is admitted only if DataFusion's own filter says so. The provider can be coarse without being wrong.
- Index and bucket pruning stay free to change granularity. A future change to the hash width or bucket size cannot silently turn into a false match.

### Negative

- Every pushed predicate is evaluated twice, once as a coarse prune and once as the filter. The second pass runs over rows the scan has already decoded.

### Risks

- Someone sees `Inexact` as a missed optimisation and promotes a predicate to `Exact`. The doc comment names the invariant, and a test in the provider's execution suite pins the `Inexact` consequence.
