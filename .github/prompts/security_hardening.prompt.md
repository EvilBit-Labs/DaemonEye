---
mode: 'agent'
model: GPT-5 (copilot)
tools: ['githubRepo', 'edit', 'search', 'new', 'runCommands', 'runTasks', 'usages', 'vscodeAPI', 'think', 'problems', 'changes', 'testFailure', 'openSimpleBrowser', 'fetch', 'extensions', 'todos', 'memory']
description: 'Analyze diff for security posture, apply safe internal hardening edits, produce report'
---

Analyze ONLY changed files (diff scope) for security posture and apply clearly safe hardening improvements while preserving all public APIs.

**FOCUS CATEGORIES**
1. Privilege Separation Integrity (no privilege creep, no added unsafe, boundary adherence)
2. Input Validation & Parsing (config, CLI, SQL, paths) – reject invalid early, no silent defaults
3. Data Handling & Storage (no secrets logged, path canonicalization, redb usage only, no binary blobs)
4. Cryptography & Integrity (correct hashing usage, future-proof abstractions, no insecure algorithms)
5. IPC & Concurrency Safety (ordering, timeouts, backpressure semantics placeholders respected)
6. Error Handling & Logging Hygiene (no sensitive leakage, structured context, no println! for operational info)
7. Dependency & Surface Minimization (avoid unnecessary crates/features, dead code removal)
8. Defense-in-Depth Opportunities (rate limiting, dedup windows, bounds, resource ceilings)
9. Security Regression Risks (stubs flagged, TODOs categorized, unimplemented sections clearly documented)
10. Supply Chain & Build Hygiene (forbid unsafe, clippy -D warnings, deny unknown features)

**ACTION WORKFLOW (MANDATORY)**
1 Diff list → 2 Security analysis per category → 3 Classify findings (`safe-edit` / `deferred` / `requires-approval`) → 4 Apply only mechanical non-breaking hardening edits (logging normalization, path canonicalize + bound checks, converting println!/eprintln! to tracing, adding `#[deny(unsafe_code)]` locally if missing, adding missing error context) → 5 Run `just lint` & `just test` → 6 Revert any failing hunk → 7 Report (summary, applied, deferred, approval-needed, risk notes, roadmap) → 8 Output unified diff (no commit).

If zero safe edits: state "No safe security edits applied" and still emit full report.

**SAFE HARDENING EDIT EXAMPLES**
- Replace `println!/eprintln!` with `tracing::{info,warn,error}!`
- Add `tracing::instrument` to sensitive boundaries (detection exec, IPC, storage access) without changing signatures
- Inline guard clauses for obvious panics or unchecked unwraps (if internal)
- Canonicalize paths then verify prefix containment
- Remove dead code exposing potential attack surface
- Strengthen error messages (no raw system paths if sensitive)
- Add length / size / iteration bounds for unbounded growth structures
- Replace stringly-typed mode flags with private enums
- Ensure all public API doc comments mention security considerations where relevant

**AUTO-EDIT CONSTRAINTS (STRICT)**
Scope: diff-only | Gates: `just lint` + tests must pass | No commits | No public signature/visibility changes | Validate after edits

**CRITICAL REQUIREMENTS**
- Preserve functional behavior while reducing risk
- No new dependencies unless strictly necessary for safety
- Avoid speculative rewrites—minimal surface change
- Avoid perf regressions; if added checks are non-trivial mark as deferred
- Do not mask existing errors—surface with context instead

**REPO RULES (REINFORCED)**
Zero warnings | No unsafe | Precise typing | Async I/O | Trait-based boundaries | thiserror+anyhow | SQL AST validation | CLI-first | Memory efficiency | redb-only | Path canonicalization | No binary blobs | rustdoc for public APIs

**EXECUTION CHECKLIST**
1 Diff scan 2 Analyze security 3 Classify 4 Apply safe hardening edits 5 Gates pass 6 Report 7 Output diff | On blocker: report with remediation.

**QUICK SECURITY MATRIX**
Category → Sample Safe Edit:
- Privilege Separation → Remove obsolete privileged call stub
- Input Validation → Add numeric range check before use
- Data Handling → Canonicalize + ensure path within data root
- Cryptography → Note TODO to migrate to BLAKE3 (if placeholder)
- IPC/Concurrency → Add timeout constant reference docs
- Logging → Replace raw error chain with redacted display
- Resource Bounds → Add comment + bound to vector growth pattern
- Stub Sections → Mark with `SECURITY_TODO:` prefix for tracking

Ambiguous? Defer and document.
