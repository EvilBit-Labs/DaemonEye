---
mode: 'agent'
model: GPT-5 (copilot)
tools: ['githubRepo', 'edit', 'search', 'new', 'runCommands', 'runTasks', 'usages', 'vscodeAPI', 'think', 'problems', 'changes', 'testFailure', 'openSimpleBrowser', 'fetch', 'extensions', 'todos', 'memory']
description: 'Analyze diff for performance, apply safe micro-optimizations, produce report'
---

Analyze ONLY changed files (diff scope) for runtime performance characteristics while preserving correctness, public APIs, and security constraints. Apply only clearly safe micro-optimizations.

**FOCUS CATEGORIES**
1. Algorithmic Complexity (unnecessary O(n^2), repeated scans, avoidable clones)
2. Allocation Behavior (temporary allocations, Vec growth patterns, reserve vs push, string churn)
3. Async & Concurrency (blocking calls in async, unnecessary awaits, join patterns, parallelism opportunities)
4. I/O Efficiency (sync I/O in async context, redundant reads/writes, batching opportunities)
5. Data Structures (better fit: map vs vec scan, small vec, newtype for clarity/perf)
6. Caching & Reuse (recomputing constants, repeated serialization, repeated formatting)
7. Hot Path Error Handling (avoidable string formatting, cheap early exits)
8. Logging Cost (expensive formatting on hot path without level guards)
9. Memory Footprint (unbounded growth, retain vs shrink_to_fit decisions, large temporary clones)
10. Instrumentation (where metrics/tracing would help future perf investigations)

**ACTION WORKFLOW (MANDATORY)**
1 Diff list → 2 Perf analysis per category → 3 Classify (`safe-edit` / `deferred` / `requires-approval`) → 4 Apply only mechanical, behavior-preserving micro-optimizations (e.g., remove redundant clone, pre-allocate capacity, replace blocking fs with async) → 5 Run `just lint` & `just test` → 6 Revert failing hunk if gates fail → 7 Report (summary, applied, deferred, approval-needed, perf notes, next steps) → 8 Output unified diff (no commit).

If zero safe edits: state "No safe performance edits applied" and still produce full report.

**SAFE PERFORMANCE EDIT EXAMPLES**
- Replace `clone()` with reference when ownership not required
- Preallocate Vec with `with_capacity` when length is known
- Convert repeated `format!` in loop to pre-built prefix + push_str
- Hoist constant regex / hashers / serializers
- Short-circuit early on empty input slices
- Use iterators instead of temporary Vec collects where semantic match
- Replace `join_all` with `FuturesUnordered` if streaming beneficial (internal only)
- Avoid converting to String just to log when `Display` exists

**AUTO-EDIT CONSTRAINTS (STRICT)**
Scope: diff-only | Gates: `just lint` + tests must pass | No commits | No public signature/visibility changes | Validate after edits | No semantic changes

**CRITICAL REQUIREMENTS**
- Do not trade readability or security for micro perf
- Never introduce unsafe
- Provide benchmarks only as recommendations (do not add heavy harness automatically)
- Defer structural refactors (module splits) unless trivial & internal
- Avoid premature caching introducing invalidation complexity

**REPO RULES (REINFORCED)**
Zero warnings | No unsafe | Precise typing | Async I/O only | Trait boundaries | thiserror+anyhow | SQL AST validation | CLI-first | Memory efficiency | redb-only | Path canonicalization | No binary blobs | rustdoc for public APIs

**EXECUTION CHECKLIST**
1 Diff scan 2 Analyze perf 3 Classify 4 Apply safe micro-optimizations 5 Gates pass 6 Report 7 Output diff | On blocker: report & remediate guidance.

**QUICK PERFORMANCE MATRIX**
Category → Sample Safe Edit:
- Complexity → Replace nested loop with `HashSet` membership check
- Allocation → Pre-size Vec for known iteration length
- Async → Replace blocking fs call with `tokio::fs` equivalent
- I/O → Batch multiple writes into single buffer append
- Data Structure → Use `SmallVec` for typical <=8 elements (internal)
- Caching → Hoist constant serialization of static JSON template
- Logging → Wrap expensive computation in level guard `if tracing::enabled!(...)` (internal)
- Memory Footprint → Replace accumulating Vec with sliding window bound
- Instrumentation → Add `tracing::instrument` to hot path for future profiling

Ambiguous? Defer and document.
