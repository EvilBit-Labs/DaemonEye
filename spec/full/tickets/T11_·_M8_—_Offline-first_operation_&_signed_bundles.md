# T11 · M8 — Offline-first operation & signed bundles

**Milestone:** M8 · **Backlog:** T19 (#59)

## Scope

**In:**

- Ensure all core functionality works without network; graceful alert-delivery degradation.
- Signed bundle config/rule distribution: Ed25519 signature verification against trusted keys, reject unsigned/invalid, per-conflict resolution (`--yes` applies default policy), atomic all-or-nothing apply with rollback; re-validate affected rules post-apply.
- Airgapped integration tests.

**Out:** Bundle authoring/signing tooling beyond verification/apply.

## Spec references

- spec:.../811199cd-23dc-4ded-9932-735252ec2438 (Flow 8)
- requirements R9.1, R9.2, R9.4, R9.5

## Key touchpoints

- New bundle module in `daemoneye-lib` — Ed25519 signature verification against configured trusted keys (reuse signing/verify primitives from T8 file:daemoneye-lib/src/crypto.rs); reject unsigned/invalid; per-conflict resolution with `--yes` default policy; atomic all-or-nothing apply + rollback.
- file:daemoneye-lib/src/storage.rs — atomic apply via redb transactions (T3); post-apply rule re-validation via the planner (T5) with unhealthy-rule surfacing.
- file:daemoneye-lib/src/config.rs — config layering for bundled config; offline/no-network operation verified end-to-end.
- Import + resulting changes recorded via T8 audit ledger.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean.
- Airgapped integration tests: unsigned/tampered bundle rejected with clear reason; valid bundle applies atomically or rolls back; offline verification works with no network.

## Dependencies

T3 (atomic redb apply), T5 (rule re-validation), T8 (Ed25519 verify primitives).

## Acceptance criteria

- Unsigned/invalid bundles rejected with clear reason; valid bundle applies atomically or rolls back; import + changes audited; verified offline.
