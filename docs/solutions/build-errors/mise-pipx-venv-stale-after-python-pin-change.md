---
title: 'mise pipx tools keep their old interpreter after the Python pin moves (fix: reinstall the tool)'
date: 2026-09-21
category: build-errors
module: mise-toolchain
problem_type: build_error
component: tooling
symptoms:
  - "`just format-docs` fails with: Error: 'exclude' patterns are only available on Python 3.13+"
  - The error names a Python version requirement that `python3 --version` already satisfies
  - Only the mise-managed tool fails; the same feature works when the tool is run under system Python
  - A sibling check that shells out to the same tool through pre-commit passes, because pre-commit builds its own environment
root_cause: incomplete_setup
resolution_type: environment_setup
severity: high
tags: [mise, pipx, uvx, python, mdformat, toolchain, stale-venv]
---

# mise pipx tools keep their old interpreter after the Python pin moves

## Problem

`just format-docs` fails against a Python version requirement the machine already meets, because the mise-managed tool is running on an interpreter that is no longer the pinned one.

## Symptoms

```console
$ just format-docs
Error: 'exclude' patterns are only available on Python 3.13+.

Please remove the 'exclude' list from your .mdformat.toml or upgrade Python
version.

error: recipe `format-docs` failed on line 85 with exit code 1

$ python3 --version
Python 3.14.7
```

The message asks for 3.13+ and the shell already has 3.14.7, which is what makes it look like a config problem in `.mdformat.toml` rather than an environment one.

## What Didn't Work

Reading the error at face value points at two dead ends. Removing the `exclude` list from `.mdformat.toml` would "fix" it by giving up tree-wide exclusions the repo depends on. Checking `python3 --version` appears to rule out a version problem entirely.

[GOTCHAS §2.1](../../../GOTCHAS.md) also points away from this, and correctly for its own case: it states in bold that the `--uploaded-prior-to` pipx failure is **not** a Python-version problem. That guidance is right about that bug and will steer you wrong about this one. The two are different failures in the same subsystem.

## Solution

Read the interpreter the tool's own venv was built with, not the one on `PATH`:

```console
$ head -1 ~/.local/share/mise/installs/pipx-mdformat/1.0.0/bin/mdformat
#!/Users/…/mise/installs/pipx-mdformat/1.0.0/mdformat/bin/python

$ ~/.local/share/mise/installs/pipx-mdformat/1.0.0/mdformat/bin/python --version
Python 3.11.15
```

`mise.toml` pins `python = "3.14.7"`. The venv was built when the pin was older and nothing rebuilds it on a pin change. Reinstall the tool:

```bash
mise uninstall "pipx:mdformat@1.0.0"
mise install   "pipx:mdformat@1.0.0"
```

## Why This Works

A `pipx:`/`uvx:` tool is installed once into its own virtualenv, and that venv hard-codes the interpreter available at install time in its shebang. Changing `python` in `mise.toml` changes what mise provides to *your shell* and to newly installed tools; it does not migrate venvs that already exist. The tool keeps running on the old interpreter indefinitely, and every feature gated on a newer Python fails with a message that reads like a config error.

The gap widens silently: the pin moves in a commit, the tool keeps working for every feature that does not need the new version, and the failure surfaces much later when someone adds a config key like `exclude`.

## Prevention

**Check the venv, not the shell, when a mise-managed tool reports a version requirement.** Read the interpreter behind the tool's *active* version, which is what `latest` points at — not every version directory, since old tool versions linger on disk and will report their own older interpreters:

```bash
for d in ~/.local/share/mise/installs/pipx-*/; do
  tgt="$d/latest"; [ -L "$tgt" ] || continue
  py=$(find "$tgt/" -maxdepth 3 -path '*/bin/python3' 2>/dev/null | head -1)
  [ -n "$py" ] && printf '%-26s %-10s %s\n' "$(basename "$d")" "$(readlink "$tgt")" "$("$py" --version 2>&1)"
done
```

This repo pins two pipx tools, `mdformat` and `pre-commit`, and both read 3.14.7 after the fix. Other entries under that install root come from other projects' mise configs and answer to whatever those configs pin, so a stale reading there is not this repo's problem to fix — check which config owns a tool before rebuilding it.

**Rebuild pipx tools when the Python pin changes.** Treat a `python = ` edit in `mise.toml` as touching every `pipx:` entry in `[tools]`, not just the interpreter. A `mise uninstall`/`install` pass over them at that moment costs seconds and avoids a failure that surfaces months later with a misleading message.

**Do not reach for the config first.** The error named `.mdformat.toml` and the fix was in neither that file nor the repo. When a tool's stated requirement contradicts what the shell reports, the tool and the shell are not running the same interpreter.

## Related

- [mise pipx tool install fails with `no such option: --uploaded-prior-to`](mise-pipx-uploaded-prior-to-pip-uvx-fix.md) — the other mise/pipx failure. Same subsystem, different cause, and its guidance explicitly rules out the Python version that is the cause here.
