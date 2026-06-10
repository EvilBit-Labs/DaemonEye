---
title: Piping just/cargo through tee masks the real exit code (false-green CI)
date: 2026-06-09
category: docs/solutions/workflow-issues
module: ci-verification
problem_type: workflow_issue
component: development_workflow
severity: high
applies_when:
  - Piping a build/test/lint command through tee, grep, head, or any pipeline stage
  - Reporting CI or build status from a command's exit code
  - Verifying a green codebase after a dependency bump or breaking upstream change
  - Interpreting a background-task or harness completion exit code
symptoms:
  - just ci-check reported exit 0 / green while the build was actually broken
  - A lighter unpiped recipe (just check) surfaced a real compile error the piped run hid
  - error[E0004] non-exhaustive patterns present in the log of a supposedly passing run
tags:
  - ci-verification
  - exit-code
  - shell-pipeline
  - pipefail
  - false-green
  - just
  - cargo
related_components:
  - testing_framework
  - tooling
---

# Piping just/cargo through tee masks the real exit code (false-green CI)

## Context

The friction was a **false green**: a verification command reported success while the build was actually broken.

The project's full CI parity check was run as a piped command:

```bash
just ci-check 2>&1 | tee /tmp/ci-check.log
```

It reported "green, exit 0" — twice. After a `sqlparser` dependency bump silently broke the build, the **second run still reported exit 0**, even though compilation was failing hard.

Root cause: **a shell pipeline returns the exit status of the LAST command in the pipe, not the first.** Here the last command is `tee`, which essentially always succeeds (exit 0). So `tee`'s success masked `just`'s real non-zero exit. The pipeline's exit code had nothing to do with whether `just ci-check` passed.

The broken state was only caught by luck: a separate `just check` run (no pipe) surfaced a hard `error[E0004]` compile error. Re-inspecting the *earlier* `| tee` log confirmed the same compile error had been present during the supposedly-passing run all along. The "green" report was never true.

## Guidance

**Never trust the exit code of a verification command that has been piped through anything** (`tee`, `grep`, `head`, `awk`, `less`, ...). The pipeline's status reflects the last stage, not your build/test/lint tool.

Use one of these trustworthy patterns instead.

**Option 1 — `set -o pipefail` (pipeline returns the first non-zero exit):**

```bash
set -o pipefail; just ci-check > /tmp/ci-check.log 2>&1; echo "EXIT=$?"
# then inspect:
grep -E "^EXIT=" /tmp/ci-check.log    # confirm the real status marker
grep -iE "tests run:|FAIL|error\[|could not compile" /tmp/ci-check.log
```

**Option 2 — check `${PIPESTATUS[0]}` explicitly (the first command's status):**

```bash
just ci-check 2>&1 | tee /tmp/ci-check.log
echo "JUST_EXIT=${PIPESTATUS[0]}"   # NOT $? — that is tee's status
```

**Option 3 — do not pipe at all; redirect, then grep the file:**

```bash
just ci-check > /tmp/ci-check.log 2>&1; echo "EXIT=$?"
grep -iE "error\[|could not compile|FAIL" /tmp/ci-check.log
```

Additional rules:

- **Always write an explicit `EXIT=$?` marker** immediately after the tool runs, and verify that marker — do not eyeball log tails and infer success.
- **Do not trust a harness / background-task "exit 0" either.** That status reflects the *last* command in a compound command. A trailing `echo` (or any always-succeeding final command) reports 0 regardless of whether the real tool failed. Capture the real tool's exit into an `EXIT=$?` marker and check *that*, not the harness-reported status.
- Capture `$?` into the marker *on the very next line* — any intervening command overwrites `$?`.

## Why This Matters

False greens erase the trust that makes CI signals worth having. A passing report you cannot trust is worse than no report: it actively hides breakage and sends downstream work forward on a broken foundation.

In a **security-focused, sole-maintainer** project that explicitly values honest velocity and honest CI reporting, declaring a broken build "green" is a real failure mode, not a cosmetic one. It compounds: time is wasted building on a broken base, the eventual discovery is by luck rather than process, and every future "green" becomes suspect. Credibility, once a green is shown to be hollow, is expensive to rebuild.

## When to Apply

- Any time you pipe a build/test/lint command through `tee`, `grep`, `head`, `less`, `awk`, or any other stage.
- Any time you report pass/fail based on a command's exit code.
- Any time you review or report a background-task / harness completion status (the reported code may be from a trailing command, not the tool).
- Whenever a verification "passes" right after a dependency bump, refactor, or generated-code change — exactly when a stale or masked result is most likely and most dangerous.

## Examples

**False green (the trap):**

```bash
just ci-check 2>&1 | tee /tmp/ci-check.log
# reports exit 0 — but that is tee's exit, not just's.
# just ci-check actually failed with error[E0004]; the log proves it,
# yet the reported status said "green."
```

**Trustworthy re-run (the fix):**

```bash
set -o pipefail; just ci-check > /tmp/ci-check.log 2>&1; echo "EXIT=$?"
grep -E "^EXIT=" /tmp/ci-check.log
grep -iE "tests run:|FAIL|error\[|could not compile" /tmp/ci-check.log
# EXIT now reflects just's real status; the grep surfaces the actual failure.
```

**The concrete failure that was being masked.** `sqlparser` 0.61 → 0.62 added a new `SelectItem` variant `ExprWithAliases` (Spark SQL `expr AS (alias1, alias2, ...)`). The exhaustive match in `daemoneye-lib/src/models/rule.rs` (`validate_select_basic`) did not cover it → `error[E0004]: non-exhaustive patterns`:

Before:

```rust
sqlparser::ast::SelectItem::UnnamedExpr(expr)
| sqlparser::ast::SelectItem::ExprWithAlias { expr, .. } => {
    Self::validate_expr_basic(expr)?;
}
```

After:

```rust
sqlparser::ast::SelectItem::UnnamedExpr(expr)
| sqlparser::ast::SelectItem::ExprWithAlias { expr, .. }
| sqlparser::ast::SelectItem::ExprWithAliases { expr, .. } => {
    Self::validate_expr_basic(expr)?;
}
```

Two properties worth keeping:

- **Route the new variant through `validate_expr_basic`** (not into the wildcard/ignore arm). Every projection expression stays validated — the safe choice for a security-sensitive SQL validator.
- **Keep the match exhaustive (no wildcard `_`).** This is the fail-loud counterpart to the verification lesson: the next `sqlparser` bump that adds a variant will again fail at *compile time*, forcing a deliberate decision instead of silently skipping validation. Fail-loud-at-compile-time pairs naturally with trust-the-real-exit-code — both refuse to let a problem hide behind a passing-looking signal.

## Related

- Sibling workflow learning (structure only, unrelated topic): [open-core-hygiene-confluence-migration-2026-04-18.md](./open-core-hygiene-confluence-migration-2026-04-18.md)
- The `sqlparser` exhaustive-match prevention rule above complements SQL-validation work in `daemoneye-lib/src/models/rule.rs` (closed issues #7, #47, #48). No existing doc covers shell pipelines or exit-code masking — this is a net-new learning.
- Session history (session history): a prior same-branch session reproduced this exact trap and arrived at the same `set -o pipefail` correction, confirming `pipefail` as the durable takeaway rather than a one-off.
