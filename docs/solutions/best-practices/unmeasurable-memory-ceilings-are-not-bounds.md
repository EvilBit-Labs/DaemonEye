---
title: A memory ceiling you cannot measure is not a bound
date: 2026-09-21
category: docs/solutions/best-practices
module: daemoneye-lib
problem_type: best_practice
component: tooling
severity: high
applies_when:
  - writing a requirement or spec that caps a cache by total bytes rather than by entry count
  - bounding memory for a type whose heap footprint the owning crate does not expose
  - reviewing a requirement that cites a per-item limit and an item count as if their presence implied an aggregate ceiling
  - choosing between `quick_cache` and `lru` for a cache that must evict in a stated order
  - reading a `size_limit`/`dfa_size_limit` pair and assuming both reject oversized input
tags:
  - regex
  - memory-budget
  - requirements
  - caching
  - detection-engine
---

# A memory ceiling you cannot measure is not a bound

## Context

Requirement R17 AC6 for the detection engine defers regex-cache bounds to a spec section (`§4.2`/`§4.5`) that does not exist anywhere in this repo. Planning T5 had to supply concrete numbers, and the first attempt wrote them as a per-pattern compile limit plus a cache entry count plus a 16 MiB aggregate ceiling, with prose claiming the numbers were "sized against the 100 MB resident budget."

Two independent reviewers did the arithmetic: 256 entries against a 1 MiB per-pattern limit permits roughly 320 MiB, more than triple the budget the requirement invoked. Fixing the arithmetic surfaced the deeper problem — the aggregate ceiling could not have been enforced at any numbers, because nothing can measure what it was counting.

## Guidance

**Bound a cache by a quantity the process can actually compute.** Before writing an aggregate byte ceiling into a requirement, establish that the runtime can read the size of the thing being cached. When it cannot, the only provable bound is a hard per-item limit multiplied by a fixed item count, and the requirement should say so in those terms rather than naming a total it has no way to check.

For the `regex` crate at the pinned 1.13.1 (`Cargo.toml:118`), three facts decide the shape:

- `RegexBuilder::size_limit` is a hard compile-time cap. Exceeding it fails the build with `Error::CompiledTooBig`, which is catchable and is a legitimate rejection path.
- `RegexBuilder::dfa_size_limit` bounds the lazy DFA's transition cache. It never errors. When the cache fills it silently resets and the engine may fall back to a slower backend. It is a performance hint, not a rejection condition, and treating it as one writes a branch that can never be taken.
- There is no API to read a compiled `Regex`'s heap footprint. `size_of_val` sees the outer struct, not the NFA or DFA behind it. Summing real per-entry bytes at runtime is not available at any effort.

So the enforceable statement is *"at most N entries, each compiled under a hard limit of M"*, whose product is the worst case. The one this project took is 64 entries at 256 KiB, a 16 MiB worst case, stated as a product rather than as a measured total.

**Match the cache crate to the eviction order the requirement names.** `quick_cache` (pinned `=0.6.23`, used in `daemoneye-lib/src/integrity/mod.rs:794`) implements a scan-resistant S3-FIFO-derived policy with no option to make it strictly LRU. A requirement that says "least-recently-used" is not satisfied by it. Its `Weighter` API does offer weight-based capacity, which looks like the answer to an aggregate ceiling right up until you need a weight to put in — and the weight is the number that cannot be measured. The `lru` crate evicts in exact LRU order and bounds by count only, which is all a provable ceiling needs.

## Why This Matters

An unenforceable bound reads as a guarantee and reviews as one. The original wording named a real budget, cited real crate APIs, and would have passed a reader who did not multiply. It survived the first drafting pass and was caught only because two reviewers independently checked the arithmetic — and even then, correcting the numbers alone would have left a ceiling no implementation could honor. The failure mode is not an out-of-memory incident during testing; it is a requirement that stays green because nothing ever evaluates it.

The same shape recurs whenever a spec caps something by bytes: compiled programs, parsed ASTs, deserialized payloads held in a cache. The question to ask is always "what call returns this number at runtime," and a satisfying answer is often absent.

## When to Apply

Apply when a requirement, ticket, or design doc states a memory ceiling in bytes for a collection of objects the process holds. Apply when picking a cache implementation against a requirement that specifies eviction order. Apply when a crate exposes two similarly named limits and only one of them fails loudly.

Do not apply to bounds over things the process does measure — byte buffers, string lengths, file sizes, row counts — where an aggregate total is both meaningful and checkable.

## Examples

Unenforceable, as originally written:

> Compiled patterns are cached keyed by the full pattern string. The cache is bounded by total compiled size with a 16 MiB ceiling, and by a 256-pattern count, whichever binds first.

Nothing can evaluate "total compiled size," and the two stated numbers permit 320 MiB between them.

Enforceable, as it now reads:

> `REGEXP` patterns compile at rule load under a 256 KiB `size_limit`. A pattern exceeding it fails to compile and is rejected. A `dfa_size_limit` is also set, but it bounds a runtime cache that resets rather than failing, so it is never a rejection condition.
>
> The cache holds at most 64 entries, which against that per-pattern ceiling bounds it at 16 MiB without measuring anything — a compiled pattern's real footprint cannot be read back at runtime, so a count against a hard per-pattern limit is the only provable bound.

The second version states the same 16 MiB, but as a product of two numbers the code enforces rather than as a total it would have to observe.

## Related

- [Piping just/cargo through tee masks the real exit code](../workflow-issues/tee-masks-exit-code-false-green-2026-06-09.md) — the same class of defect in verification: a check that reads as passing because nothing evaluates what it claims to.
- [A measurement harness can pass its own correctness gate while measuring the wrong thing](measurement-fixtures-must-exercise-correctness-where-they-measure-cost.md) — measurement that does not cover what it reports on.
