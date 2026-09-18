---
title: A test can sit on the wrong branch when two paths return the same error variant
date: 2026-09-17
category: docs/solutions/best-practices
module: daemoneye-lib
problem_type: best_practice
component: testing_framework
severity: high
applies_when:
  - writing or reviewing a test that guards a safety gate ahead of a destructive or irreversible action
  - two error paths in one function, or in a function and its wrapper, return the same error variant
  - a test asserts only on the error variant (e.g. `matches!(err, SomeVariant { .. })`) and not on a discriminating field
  - a test builds a deliberately corrupted fixture to trigger one specific internal check
  - refactoring removes a wrapper that tests were written against
  - decoding a non-self-describing binary format (postcard, bincode) whose version field is written but never read
tags:
  - test-assertions
  - false-positive-tests
  - safety-gates
  - error-variants
  - redb
  - postcard
  - schema-migration
related_docs:
  - docs/solutions/best-practices/assert-mechanism-not-outcome-in-resilience-tests-2026-06-13.md
---

> Related: [Assert the mechanism, not just the outcome, in resilience integration tests](./assert-mechanism-not-outcome-in-resilience-tests-2026-06-13.md). That entry covers a production fallback satisfying an outcome before the target branch runs. This one covers a test whose fixture was wired to a different branch entirely, where the assertion could not tell the two apart. Consider consolidating if a third instance of "green test, unexercised branch" turns up.

## Problem

`daemoneye-lib/src/storage/schema.rs` implements the schema-rebuild drop gate (spec requirement R15): before `reinit_store` (`daemoneye-lib/src/storage/schema.rs`) deletes and recreates the live redb file, the old store must first be archived into a signed bundle, and that bundle must be proven both authentic and *complete* before the destructive step is allowed to run. The completeness half of that gate lives in `verify_bundle_self`: it reads the bundle and its detached signature back from disk, verifies the signature, then re-derives the archived store's real table set via `enumerate_archived_partitions` and compares it against the manifest embedded in the bundle. A mismatch — a partition the manifest claims exists but the archive does not actually hold — must be rejected with `StorageError::Bucket { message: "incomplete archive: ..." }`.

The test `verify_bundle_rejects_manifest_with_phantom_partition` was believed to exercise that completeness check. It did not. At the time (still visible via the parent of commit `d358ed5`), the test built an inflated manifest with a `"phantom.partition"` entry not present in the real store, but exported the bundle using the *real* (non-inflated) manifest, and then called a since-removed wrapper `verify_bundle(bundle_path, signer, &inflated)` that took the inflated manifest as a separate `expected` parameter and did its own `embedded != expected` equality check on top of `verify_bundle_self`'s internal work. Because the exported bundle's *embedded* manifest was the real one, the completeness comparison inside `verify_bundle_self` (embedded manifest vs. the freshly re-derived archive table set) passed cleanly. The rejection the test observed came entirely from the wrapper's separate equality check against the inflated `expected` argument — a different code path checking a different thing.

Both the completeness gate and the wrapper's equality check return the same `StorageError::Bucket { bucket: "schema-rebuild", .. }` variant, and the test's only assertion was `assert!(matches!(err, StorageError::Bucket { .. }))` — a check on the enum discriminant, which both branches satisfy identically. Nothing in the test could tell the two apart. The result: the gate that stands between the schema-rebuild migration and irreversible destruction of an unarchived partition had zero coverage, hidden behind a test whose name specifically claimed to cover it.

## Investigation

The bug was not found by a failing test — the test was green, and had been since it was written. It surfaced during a refactor (landed in commit `d358ed5`, "refactor(storage): single bundle-verification entry point, and test the gate it guards") that was removing the `verify_bundle(path, signer, expected)` wrapper in favor of calling `verify_bundle_self` directly wherever the production code needed it (see the call sites at `schema.rs` and `schema.rs`, inside `run_rebuild`). Removing the wrapper meant every test that called it had to be rewritten to call `verify_bundle_self` directly, which forced the question: for each such test, which manifest does `verify_bundle_self` actually see, and does the assertion distinguish the branch that's supposed to fire from every other branch that produces the same error variant?

Answering that for `verify_bundle_rejects_manifest_with_phantom_partition` required tracing exactly which manifest got exported into the bundle (`export_bundle` embeds whatever manifest it's handed — `schema.rs`) versus which manifest the old wrapper compared against. The phantom partition had been planted in the `expected` argument, not in the manifest that `export_bundle` actually wrote into the archive. That is the moment the test was shown to be sitting on the wrong branch: it could pass even if `verify_bundle_self`'s completeness check were deleted entirely, because the wrapper's separate equality check would still fire and produce an indistinguishable `StorageError::Bucket`.

The same doubt was then applied to `bitflip_in_bundle_fails_verification`, the sibling test guarding the signature-check half of `verify_bundle_self`. It had the identical structural weakness: it asserted only `matches!(err, StorageError::Bucket { .. })`, so a bitflip that happened to also break the completeness check (or vice versa) would still pass the test without confirming *which* internal check actually caught the corruption.

## Root Cause

Two independent failure modes inside the pre-refactor drop-gating logic collapsed onto the same `StorageError::Bucket` variant with no discriminating field required by the assertions:

1. `verify_bundle_self`'s completeness check (comparing the bundle's *embedded* manifest against the re-derived archive table set).
2. The now-removed `verify_bundle` wrapper's `embedded != expected` equality check (comparing the embedded manifest against a caller-supplied *expected* manifest — what is now `ensure_bundle_is_this_run`, `schema.rs`).

A test that wants to prove check (1) fired must plant the discrepancy in the bundle's *embedded* manifest (the one written by `export_bundle`), not in a separate `expected` argument compared by a different function. `verify_bundle_rejects_manifest_with_phantom_partition` planted it in the wrong place, so it silently tested check (2) while believing it tested check (1). Because `assert!(matches!(err, StorageError::Bucket { .. }))` only inspects the enum variant, not the `message` field that actually names which check failed, the test had no way to notice it was on the wrong branch.

## Solution

The fix, now in the tree, does two things: it makes each test exercise the correct code path with a manifest defect planted where the corresponding function will actually see it, and it upgrades every assertion to check the discriminating detail — the error `message` — rather than just the outer variant.

`verify_bundle_rejects_manifest_with_phantom_partition` now builds the inflated manifest and passes *that* into `export_bundle` as the manifest to embed, so the bundle's own embedded manifest — the thing `verify_bundle_self` reads back and compares against the re-derived archive — is the one carrying the phantom partition:

```rust
let inflated = BundleManifest {
    format_version: BUNDLE_FORMAT_VERSION,
    schema_version: from_version,
    written_at_ms: 1,
    partitions, // includes "phantom.partition"
};
let bundle_path = default_bundle_path(&path);
export_bundle(&path, &bundle_path, &inflated, &signer).unwrap();

let err = verify_bundle_self(&bundle_path, &signer).unwrap_err();
let StorageError::Bucket { ref message, .. } = err else {
    panic!("expected a bucket error, got {err:?}");
};
assert!(
    message.contains("incomplete archive"),
    "expected the completeness gate to reject it, got: {message}"
);
```

It now calls `verify_bundle_self` directly (the wrapper is gone) and asserts on `message.contains("incomplete archive")` — the literal substring `verify_bundle_self` emits only from its completeness branch, so a pass here is only possible if that specific check ran and rejected.

Its sibling, `bitflip_in_bundle_fails_verification`, got the equivalent treatment: it now asserts `message.contains("signature verification failed")`, the literal string from the signature-check branch. With both tests keyed to disjoint substrings, neither can pass by the other's branch firing instead.

The equality check that used to live inside the `verify_bundle` wrapper survived the refactor, but not as dead weight — it was pulled out into its own named function, `ensure_bundle_is_this_run`, with its own direct test (`a_different_signed_archive_is_not_accepted_as_this_run`, `schema.rs`). It is not redundant with the signature check: `verify_bundle_self`'s signature check proves the bundle on disk is *a* validly-signed archive, not that it is *this run's* archive. A bundle swapped in between the export write and the read-back — for example an older archive signed with the same key — would satisfy the signature and the completeness check alike, and since the very next step (`reinit_store`) destroys the live store, that substitution would destroy data whose only surviving copy was the archive that got swapped away. `ensure_bundle_is_this_run` is called immediately after `verify_bundle_self` at the export-time call site specifically to close that gap.

Separately, but discovered via the same "postcard is not self-describing" scrutiny that motivated auditing these tests, `split_bundle` now gates on `manifest.format_version != BUNDLE_FORMAT_VERSION` before trusting any other field. Postcard's binary format has no embedded type tag, so a manifest written under a different envelope layout doesn't fail to decode — it decodes into plausible-looking nonsense, and previously that nonsense would flow downstream and surface as a misleading "incomplete archive" completeness error rather than an honest version-mismatch error. The regression test `bundle_with_an_unsupported_format_version_is_rejected` pins this by asserting the specific message `"unsupported bundle format version"`.

## Prevention / How to Apply

**The generalizable rule:** when two error paths inside one function (or across a function and the wrapper that calls it) return the same error variant, an assertion on the variant alone — `assert!(matches!(err, SomeVariant { .. }))` — cannot prove which path fired. A test can then sit permanently on the wrong branch while its name, its comments, and its green CI status all claim otherwise. This is especially dangerous for tests that exist specifically to guard a safety gate ahead of an irreversible action (here: destroying the live store via `reinit_store`), because the whole point of the test is proof that the gate — and not some unrelated check — is what stops the bad case.

Concrete practices to apply going forward, illustrated by this fix:

- **Assert on the discriminating detail, not the discriminant.** Match the concrete error variant, then assert a substring of its `message` (or another field) that is unique to the branch under test — as both `verify_bundle_rejects_manifest_with_phantom_partition` and `bitflip_in_bundle_fails_verification` now do with disjoint substrings (`"incomplete archive"` vs. `"signature verification failed"`). If two sibling tests could each pass via the other's branch, the assertions aren't discriminating enough yet.
- **When a test builds a deliberately-corrupted fixture to trigger a specific internal check, verify the corruption lands in the value that specific check actually reads** — not in a parameter consumed by a different function nearby. Trace the call graph from the test's fixture construction through to the function under test before trusting that the fixture exercises what the test name claims.
- **Treat a passing test around a safety/destructive gate with the same suspicion as a failing one** when refactoring nearby code. The trigger here wasn't a bug report; it was a refactor removing a wrapper, which forced re-deriving each caller's manifest wiring and exposed that one test's fixture didn't match its target function. Removing indirection (the wrapper) is itself a good moment to audit whether tests built against that indirection still test what they claim.
- **Don't discard an equality check just because a nearby check makes it look redundant.** `ensure_bundle_is_this_run`'s `embedded == expected` check looks superficially subsumed by `verify_bundle_self`'s signature+completeness checks, but proves a different property (this-run identity vs. authenticity+completeness). Keep it named, keep it tested independently, and document in a comment why it isn't redundant — as `schema.rs` now does — so a future simplification pass doesn't remove it by the same "looks redundant" mistake.
- **When a binary format is not self-describing (postcard, bincode, etc.), gate on an explicit format/version field before trusting decoded content**, and add a targeted test (`bundle_with_an_unsupported_format_version_is_rejected`) that pins the version-mismatch error message distinctly from whatever downstream error garbage decoding would otherwise produce.
