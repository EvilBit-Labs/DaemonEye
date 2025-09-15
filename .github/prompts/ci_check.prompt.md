---
mode: agent
model: Auto (copilot)
tools: [githubRepo, edit, search, new, runCommands, runTasks, usages, vscodeAPI, think, problems, changes, testFailure, openSimpleBrowser, fetch, extensions, todos, memory]
description: Ensure code changes pass all CI checks before merging.
---

1. First, run `just ci-check` to identify any failures
2. Analyze the output to understand what specific checks are failing. If everything passes, you’re done.
3. Make minimal, targeted fixes to address ONLY the failing checks:
   - For formatting issues: run `just format`
   - For linting issues (clippy): fix the specific violations reported (rerun with `just lint-rust` / `just lint-rust-min`)
   - For compilation/type errors: fix the underlying Rust code until `just check` (or `cargo check`) succeeds
   - For test failures: fix the failing tests or underlying code (verify with `just test` or `just test-ci`)
   - For dependency security/advisory issues: run `just audit` (cargo-audit) and/or update `Cargo.toml` then `cargo update`
   - For license/compliance issues: run `just deny` and address cargo-deny findings
4. After making fixes, run `just ci-check` again to verify all checks pass
5. If any checks still fail, repeat steps 2-4 until all checks pass
6. Provide a summary of what was fixed and confirm that `just ci-check` now passes completely

Keep changes minimal and focused - only fix what's actually causing the CI failures. Do not make unnecessary refactoring or style changes beyond what's required to pass the checks.
