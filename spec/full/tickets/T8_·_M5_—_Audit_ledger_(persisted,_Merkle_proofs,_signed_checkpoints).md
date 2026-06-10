# T8 · M5 — Audit ledger (persisted, Merkle proofs, signed checkpoints)

**Milestone:** M5 · **Backlog:** T10 (#42)

## Scope

**In:**

- Move ledger persistence into procmond: append-only `AUDIT_LEDGER`/`AUDIT_CHECKPOINTS`/`AUDIT_FRONTIER` in a separate redb (write-only handle, `0640`, `daemoneye` group).
- Real `rs_merkle` inclusion proofs (replace `vec![]` stub in file:daemoneye-lib/src/crypto.rs), MMR-frontier checkpoints, optional Ed25519-signed checkpoints (key `0400`, `secrecy`/`zeroize`).
- Chain verification + recovery/consistency check; audit hooks in `ProcessMessageHandler` for security-relevant events.
- Agent opens ledger read-only and serves CLI audit verify/prove/checkpoint over IPC (CLI never opens ledger directly).

**Out:** CLI command surface itself (T10) beyond the agent-side API.

## Spec references

- spec:.../811199cd-23dc-4ded-9932-735252ec2438 (Flow 6)
- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Audit ledger DB, read path)
- requirements R7.1–R7.6

## Key touchpoints

- file:daemoneye-lib/src/crypto.rs — `AuditLedger`/`AuditEntry`/`Blake3Hasher`; replace `generate_inclusion_proof()` `vec![]` stub with real `rs_merkle` proofs; add MMR-frontier checkpoints + optional Ed25519-signed checkpoints.
- procmond — new audit-writer module owning the write-only handle; audit hooks in the `ProcessMessageHandler`/collection path; separate redb file (`AUDIT_LEDGER`/`AUDIT_CHECKPOINTS`/`AUDIT_FRONTIER`), `0640`, `daemoneye` group.
- Agent opens ledger **read-only** and serves CLI audit verify/prove/checkpoint over IPC (file:daemoneye-lib/src/ipc/); CLI never opens the ledger directly.
- Deps present: `rs_merkle`, `blake3`. New deps: `ed25519-dalek` (signing), `secrecy`/`zeroize` (key handling); key file `0400` under procmond UID.
- Reference: Tech Plan "Audit ledger DB" + design.md "Audit Ledger (redb + rs-merkle)".

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; new crypto deps pass `cargo deny`/`cargo audit`.
- Crypto tests: chain verify, tamper detection pinpointing first bad entry, offline inclusion-proof verification, checkpoint signature verify, recovery from frontier; canonical-serialization stability test.

## Dependencies

T3 (redb codecs/patterns established).

## Acceptance criteria

- Tamper-evident chain verifies; inclusion proofs verify offline from `(root, index, total, leaf_hash, siblings[])`; signed checkpoints exportable.
- Tampering detected and pinpointed to first bad entry; recovery replays from latest checkpoint frontier.
