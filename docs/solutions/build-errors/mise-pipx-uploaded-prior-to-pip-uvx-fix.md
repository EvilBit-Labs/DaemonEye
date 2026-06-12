---
title: 'mise pipx tool install fails in CI with `no such option: --uploaded-prior-to` (fix: add uv)'
date: 2026-06-11
category: build-errors
module: ci/mise-toolchain
problem_type: build_error
component: tooling
symptoms:
  - Every CI job (build, quality, test, test-cross-platform) fails at the 'Set up mise toolchain' step
  - 'mise ERROR Failed to install pipx:pre-commit@latest: pipx exited with non-zero status: exit code 1'
  - 'pip prints its usage text and: no such option: --uploaded-prior-to'
  - A sibling pipx tool (mdformat, which uses uvx_args) installs fine in the same step
  - Reproduces only in CI; dev machines install the same tools without error
root_cause: config_error
resolution_type: tooling_addition
severity: high
tags: [mise, pipx, uvx, uv, pre-commit, ci, toolchain]
---

# mise pipx tool install fails in CI with `no such option: --uploaded-prior-to` (fix: add uv)

## Problem

The `mise` toolchain bootstrap failed to install the `pre-commit` pipx tool, so every CI job died at the "Set up mise toolchain" step and turned all PRs red. The failure was environmental (CI only) and unrelated to any code change.

## Symptoms

- All CI jobs fail at the mise setup step, before any cargo/build work runs.
- `mise ERROR Failed to install pipx:pre-commit@latest: pipx exited with non-zero status: exit code 1`
- The underlying error is from pip: `no such option: --uploaded-prior-to`, followed by pip's full `Usage:` dump.
- The failing command (truncated in logs) is roughly: `.../pipx-pre-commit/4.6.0/shared/bin/python -m pip ... install --upgrade --uploaded-prior-to=2026-06-10T22:16:02Z --force-reinstall -q 'pip >= 23.1'`
- `mdformat` (also a `pipx:` tool, but configured with `uvx_args`) installs cleanly in the same step.
- Works on dev machines — including ones running Python 3.14.

## What Didn't Work

- **Pinning Python to 3.13** (theory: Python 3.14, released the prior day, lacked wheels for pre-commit's deps). This was a **misdiagnosis** and did not fix CI. Tells that should have ruled it out immediately:

  - The error was pip's **usage dump** (an *invalid argument*), not `No matching distribution found` (the wheel-missing signature).
  - The maintainer ran Python 3.14 locally with zero pipx issues.
  - `mdformat` installed fine in the same job regardless of Python version.

  The Python version was a red herring; the pin was reverted.

## Solution

Add `uv` as a tool in `mise.toml` so the pipx backend always routes through `uvx` instead of falling back to plain `pip`:

```toml
[tools]
# ...
uv = "latest"                # ensures pipx tools install via uvx, not pip
"pipx:pre-commit" = "latest"

[settings.pipx]
uvx = true
```

No Python pin and no mise-version pin are needed — the lockfile already pins versions; the missing piece was `uv` itself.

## Why This Works

mise emits a reproducibility flag, `--uploaded-prior-to=<lockfile-timestamp>` (a time-based "don't use packages uploaded after this instant" pin), when installing locked pipx tools. **That flag is a `uv` option; plain `pip` doesn't have it.**

With `[settings.pipx] uvx = true` but **no `uv` tool installed**, mise's "upgrade shared libraries" phase for the pipx venv fell back to `python -m pip install --upgrade --uploaded-prior-to=... pip`. pip rejected the unknown option (`no such option: --uploaded-prior-to`), dumped usage, and exited 1 — failing the whole pipx install and the mise setup step.

`mdformat` survived because its `uvx_args` forced the uv path, which understands the flag. Installing `uv` makes the entire pipx flow use uvx, so the time-pin flag is always handled.

## Prevention

- When using mise's `pipx`/`uvx` backend together with lockfile or time-based pinning, **install `uv` explicitly** so the toolchain never falls back to `pip` for flags only `uv` understands.
- **Read the actual error before theorizing.** A pip **usage/`Usage:` dump** means *invalid argument*, not a version/wheel problem. `No matching distribution found` is the wheel-missing signature — this was not that. Misreading it cost a wrong Python-pin detour.
- A pipx tool that fails **while a sibling with `uvx_args` succeeds** is a strong signal the break is in the uv-vs-pip code path, not the package, the Python version, or the network.

## Related Issues

- PR #195 — landed the `uv` fix in `mise.toml`.
- PR #192 — earlier CI cleanup (removed a flaky benchmark gate) in the same toolchain area.
