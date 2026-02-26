# Code Review

## Description

Analyze diff for code quality issues and apply safe improvements while preserving public APIs.

## Focus Categories

Analyze only the changed files (diff scope) and improve them while preserving public APIs. Focus categories: (1) Code Smells (large/duplicate/complex) (2) Design Patterns (traits, builder, newtype, factory) (3) Best Practices (Rust 2024, project conventions) (4) Readability (naming, structure, cohesion) (5) Maintainability (modularization, clarity) (6) Performance (async, redb I/O, allocation, blocking) (7) Type Safety (strong types, avoid needless Option/Result layering) (8) Error Handling (thiserror + anyhow context, no silent failures). Context: DaemonEye = security-first, airgapped, zero-warnings, privilege separation, CLI-first, memory conscious. Prefer clear + secure over clever.

## Steps

1. Collect diff file list. 2. Analyze per focus category. 3. Classify each finding: `safe-edit` (apply now), `deferred`, `requires-approval`. 4. Auto-apply only `safe-edit` (mechanical, internal, non-breaking, warning removal, correctness, logging consistency, blocking I/O → async). 5. Run `just lint` then `just test`. On failure: isolate failing hunk, revert it, re-run, document skip. 6. Generate report (summary table, applied edits + rationale, deferred backlog, approval-needed with risks, next-step roadmap). 7. Output unified diff (never commit). If zero safe edits: state "No safe automatic edits applied" and still output full report.

## Auto-Edit Constraints (Strict)

- Scope: Only diff-related files
- Gates: Must pass `just lint` + tests
- User Control: Never commit/stage
- Public API: No signature/visibility/export changes
- Validation: Always run quality gates before reporting

## Critical Requirements

- Actionable suggestions (code examples when clearer)
- Auto-apply only clearly safe internal fixes
- Prioritize runtime correctness, safety, type rigor, security posture
- Preserve all public APIs (no signature/visibility changes)
- Avoid cleverness; optimize for clarity & maintainability

## Repo Rules (Reinforced)

Zero warnings (clippy -D warnings) | No unsafe | Precise typing | Async I/O only | Trait-based services | `thiserror` + `anyhow` | SQL AST validation | CLI-first (`daemoneye-cli`) | Memory efficient | redb-only storage abstraction | Path canonicalization + root safety | No binary blobs in DB | rustdoc for all public APIs

---

## Execution Checklist

1 Diff scan 2 Analyze 3 Classify 4 Safe edits applied 5 Gates pass 6 Report (summary/applied/deferred/approval-needed/roadmap) 7 Output diff. On blocker: report + remediation guidance.

## Quick Reference Matrix

Category -> Examples of Safe Edits:

- Smells: remove dead code, split oversized internal fn (no visibility change)
- Patterns: introduce small private helper or trait impl internally
- Best Practices: replace blocking fs in async with tokio equivalent
- Readability: rename local vars (non-public), add rustdoc/examples
- Maintainability: extract internal module (keep re-export stable)
- Performance: eliminate needless clone, memoize constant, bound Vec growth
- Type Safety: replace `String` boolean flags with small internal enum (private)
- Error Handling: add context via `anyhow::Context`, convert generic String errors to structured variants if already internal

If ambiguity arises, default to: classify (deferred) instead of applying.

## Completion Checklist

- [ ] Code conforms to DaemonEye project rules and security standards (AGENTS.md)
- [ ] Tests pass (`just test`)
- [ ] Linting is clean (`just lint`)
- [ ] Full CI validation passes (`just ci-check`)
- [ ] A short summary of what was done is reported
