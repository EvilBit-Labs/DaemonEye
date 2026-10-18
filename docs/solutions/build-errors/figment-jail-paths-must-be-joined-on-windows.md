---
title: A figment Jail path formatted with `/` names a file that does not exist on Windows
date: 2026-10-14
category: build-errors
module: daemoneye-lib/tests (figment::Jail config tests)
problem_type: test_failure
component: testing_framework
symptoms:
  - The Windows leg of the CI test matrix fails on `ca_file_must_exist_parse_and_avoid_traversal` while Linux, macOS and every local run pass
  - 'The assertion expected a `contains no PEM certificate` validation message but got `ca_file cannot be read: NotFound` for a file the test had just created'
  - The Linux and macOS legs of the same run show `cancelled`, not their own failure
root_cause: wrong_api
resolution_type: test_fix
severity: medium
tags: [figment, jail, windows, verbatim-path, path-join, ci-matrix, cross-platform, nextest]
---

# A figment Jail path formatted with `/` names a file that does not exist on Windows

## Problem

A config test created files inside a `figment::Jail`, then referenced them by string-formatting `jail.directory()` with a `/` and the file name. On the Windows CI runner the jail directory is a verbatim path, where `/` is not a separator, so every formatted path pointed at a file that did not exist. The test failed on Windows only, and the matrix cancelled the other two operating-system legs with it.

## Symptoms

- CI run on branch `T7` for PR #291: `test-cross-platform (windows-2022, Windows)` failed; `(ubuntu-22.04, Linux)` and `(macos-15, macOS)` reported `cancelled`.

- The failure:

  ```text
  thread 'ca_file_must_exist_parse_and_avoid_traversal' panicked at daemoneye-lib\tests\alerting_config.rs:
  assertion failed: failure(&webhook(&at("empty.pem"))).contains("certificate")
  ```

- The earlier assertion in the same test, that a missing file's message contains `ca_file`, passed, because the validator's read-error message in `validate_ca_file` (`daemoneye-lib/src/alerting/config.rs`) also contains `ca_file`. Only the assertion that depends on the file actually being read reached the mismatch.

## What Didn't Work

- Reading the test and the validator for a Windows-specific branch. `validate_ca_file` (`daemoneye-lib/src/alerting/config.rs`) has none: it rejects `..` components, reads the file, and scans for the PEM marker. Nothing in it treats platforms differently, so the code under test was not the place to look.
- Suspecting the traversal check. The jail directory contains no `..` component on any platform, and the message that came back was the read error, not the traversal error.

## Solution

Build the path with `Path::join` instead of formatting a string:

```rust
// Before
let dir = jail.directory().to_string_lossy().into_owned();
let at = |name: &str| json!({"ca_file": format!("{dir}/{name}")});

// After (daemoneye-lib/tests/alerting_config.rs, `ca_file_must_exist_parse_and_avoid_traversal`)
let at = |name: &str| json!({"ca_file": jail.directory().join(name).to_string_lossy()});
```

Fixed in PR #291; this write-up ships in the same PR.

## Why This Works

`figment::Jail` canonicalises its temporary directory, and on Windows canonicalisation yields a verbatim path with the `\\?\` prefix. Rust's `std::path` documents that in a verbatim path only `\` separates components and `/` is an ordinary character. So a formatted path such as `\\?\C:\...\.tmpAbc/empty.pem` (illustrative, not a file in this repository) has a single final component, `.tmpAbc/empty.pem`, which does not exist. `std::fs::read` returns `NotFound`, the validator reports the read error, and the assertion that expected the PEM-marker message fails. `Path::join` uses the platform separator, so the joined path resolves on every leg of the matrix.

Nothing about this is specific to `ca_file`: any test that formats `jail.directory()` with `/` and then expects the file to be readable has the same bug, and it only shows on the Windows leg.

## Prevention

- Treat `jail.directory()` as a `Path`, never as a string prefix. Use `join`, and convert to a string only at the point the config needs one.
- A test that passes locally and fails only on `windows-2022` with a `NotFound` for a file the test created is this trap until proven otherwise. Check for `format!` or string concatenation on a path before reading any production code.
- When one leg of `test-cross-platform` fails, the other legs show `cancelled`; that is GitHub's default matrix fail-fast (`ci.yml` sets no override), not a second failure. Read the leg that says `failure`.
- The same rule applies to any canonicalised path on Windows (`std::fs::canonicalize`, `tempfile` paths that were canonicalised), not only figment.

## Related Issues

- [GOTCHAS.md §1.1](../../../GOTCHAS.md): Windows-only behaviour validates only in CI; the macOS host cannot cross-compile the Windows target.
- [Assert which gate fired when error variants collide](../best-practices/assert-which-gate-fired-when-error-variants-collide-2026-09-17.md): the `ca_file`-substring assertion passing on the wrong error path is the same shape.
