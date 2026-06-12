# Development Gotchas & Pitfalls

This document tracks non-obvious behaviors, common pitfalls, and hard-won lessons in the DaemonEye codebase to assist future maintainers and contributors. It accretes over time — add an entry whenever a surprise costs more than a few minutes.

## 1. Testing, Linting & CI

### 1.1 Local `cargo test` Does Not Match CI

CI runs `cargo nextest run --profile ci --all-features` on a Linux / macOS / Windows matrix. A local `cargo test --workspace` (one OS, default features) is **not** equivalent and routinely passes while CI fails.

- **Misses feature-gated code:** anything behind a non-default feature (e.g. `fuzzy-hashes`) only compiles/runs under `--all-features`.
- **Misses platform `cfg` code:** `#[cfg(windows)]` / `#[cfg(unix)]`-only paths only compile on that platform. The most common bite is a `use` that is referenced *only* inside a `#[cfg(unix)]` block — fine on Linux/macOS, but on Windows it's an unused import, and the workspace `warnings = "deny"` turns that into a build failure.
- **You can't cross-compile to Windows locally:** the macOS dev host can't build the `x86_64-pc-windows-msvc` target — `blake3`'s build script needs the MSVC assembler (`ml64.exe`). So Windows-only `cfg` issues validate **only** in CI.
- **Mitigation:** gate `cfg`-specific imports with the *same* `cfg` as their use (`#[cfg(unix)] use ...;`). Before declaring a feature-gated change done, run `cargo clippy --workspace --all-targets --all-features -- -D warnings` locally; accept that Windows-`cfg` correctness ultimately rides on CI.

### 1.2 `clippy::expect_used` Is Denied in Production Code

The workspace denies `clippy::expect_used`, `unwrap_used`, and `panic` outside tests. `.expect("...")` will fail `clippy -- -D warnings` in any non-test path.

- **Use instead:** `?`, `unwrap_or_default()`, `unwrap_or_else(...)`, or explicit error handling.
- **Tests are exempt:** every `#[cfg(test)]` module carries a `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` header by convention. Keep it on new test modules — don't remove it, and don't "fix" it. A reviewer suggestion to strip the module-wide allow is declining the project's own test convention.

### 1.3 Pre-commit Hooks Rewrite Files and Abort the Commit

`pre-commit` runs `cargo fmt` and `mdformat` (markdown), both of which **modify files in place**. When they change a staged file, the commit aborts:

- `cargo fmt` → "Stashed changes conflicted" if unstaged changes coexist.
- `mdformat` → "files were modified by this hook".
- **Fix:** let the hook reformat, then `git add` the reformatted files and commit again. Run `cargo fmt --all` and reset unrelated unstaged files before staging to avoid the stash conflict.

### 1.4 Cross-Crate Trait Methods Need the Trait in Scope

To call a trait method on a type from another crate, the trait must be imported even if you never name it directly — e.g. `use daemoneye_eventbus::rpc::RegistrationProvider;`. Without it the method "doesn't exist" with a confusing error. Common in tests exercising cross-crate behavior.

## 2. Toolchain (mise)

### 2.1 mise pipx Tools Need `uv` Installed

`mise.toml` sets `[settings.pipx] uvx = true`, but if the `uv` tool itself is missing from `[tools]`, mise's pipx flow falls back to plain `pip` for some steps and passes `uv`-only flags (e.g. `--uploaded-prior-to=<lockfile-timestamp>`) that `pip` rejects (`no such option: --uploaded-prior-to`). Every CI job then dies at the "Set up mise toolchain" step.

- **Fix:** keep `uv = "latest"` in `[tools]`.
- **This is NOT a Python-version problem** (3.14 works locally). Full write-up + the misdiagnosis trail: [`docs/solutions/build-errors/mise-pipx-uploaded-prior-to-pip-uvx-fix.md`](docs/solutions/build-errors/mise-pipx-uploaded-prior-to-pip-uvx-fix.md).

## 3. CI Infrastructure & Bots

### 3.1 Benchmarks Don't Run on PRs

`.github/workflows/benchmarks.yml` is `workflow_dispatch`-only — shared GitHub runners are too inconsistent for latency/throughput benchmarking to gate on. Don't expect benchmark results in PR CI; run them locally or via manual dispatch.

### 3.2 Dosubot Pushes to Open PRs

Dosubot auto-pushes a documentation commit when a PR opens, which cancels and restarts any in-progress CI run. This is normal — the cancelled run isn't a real failure, the re-triggered run is what counts. If your local branch diverges, `git pull --rebase` over the Dosu commit before pushing.

### 3.3 Merges Go Through Mergify, By a Human

Per AGENTS.md rule 01, merges are performed by a human maintainer via Mergify (the merge queue / `/queue` command), never by an AI assistant. `.mergify.yml` uses the current schema (`queue_rules` + `merge_protections` + `merge_protections_settings`); bot PRs auto-merge via `auto_merge_conditions`, maintainer PRs are enqueued manually. Allowed merge methods are **squash** and **rebase** (no merge commits).

### 3.4 Security Scanners Flag Non-Obvious Things

- **zizmor** (GitHub Actions): every workflow needs an explicit top-level `permissions:` block, or it flags the implicit broad token. Add `permissions: { contents: read }` (or narrower) to each workflow.
- **CodeQL `rust/cleartext-logging`**: don't interpolate sensitive values into `assert!`/`assert_eq!` messages — CodeQL treats assert-message interpolation as logging and flags it as cleartext logging of sensitive data.

## 4. Code & Domain

### 4.1 Unix Socket Path Limit Is 107 Bytes

`sockaddr_un.sun_path` is 108 bytes including the trailing NUL — the usable limit is **107**, not 255. Validate socket paths against 107.

### 4.2 ssdeep Stays Off the Cryptographic Hasher

ssdeep / CTPH is a non-cryptographic binary-change signal — **never** add it as a `HashAlgorithm` variant in `MultiAlgorithmHasher`. `HashResult.hashes` enforces a cryptographic-only invariant (`is_cryptographically_secure`, 64-hex-char length). ssdeep lives on its own path (`daemoneye-lib/src/integrity/fuzzy.rs`) and is carried as a dedicated field, not as an identity hash.

## 5. Clippy & Rustdoc Lint Traps

`cargo clippy -- -D warnings` runs with pedantic/nursery/restriction lints enabled; these specific traps fail the build in non-obvious ways (never silence with `#[allow]` unless justified — `08. Never remove clippy restrictions`):

- **`uninlined_format_args`**: use inlined `{variable}` in `format!`/`anyhow!`, not positional `{}` + trailing args.
- **`unnecessary_negation`**: put `==` checks first in if/else, not `!=` then `else`.
- **`map_err_ignore`**: name ignored closure vars (`|_elapsed|`, not `|_|`).
- **`as_conversions`**: intentional casts need `#[allow(clippy::as_conversions)]` with a `// SAFETY:` comment.
- **`future_not_send`**: never `.await` inside `info!`/`debug!`/`warn!`/`error!` — extract the value into a binding first, then log it.
- **`string_slice`**: denied — use `split_once('=')` / `char_indices()`, never `&s[..pos]`, on untrusted input.
- **`items_after_statements`**: all `const` declarations go at the top of the function, not after a `return`.
- **`redundant_pub_crate`** (nursery): for sibling-only visibility across submodules, use `pub(super)`, not `pub(crate)`.
- **Rustdoc broken links**: escape brackets in paths (`/proc/\[pid\]/stat`) to avoid broken-link warnings.
