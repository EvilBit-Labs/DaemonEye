# T3 · M2 — Storage engine (redb §11.7 event store + codecs + retention)

**Milestone:** M2 · **Backlog:** T11.2 (#44), T11.3 (#45)

## Scope

**In:**

- Replace `Vec<u8>` placeholders in file:daemoneye-lib/src/storage.rs with typed tables per Tech Plan Data Model: `processes.events` keyed `(ts_ms, seq)` (fixed-width BE) + multimap secondary indexes (`idx:pid/ppid/name/exe_hash`), `scans`, `detection_rules`, `alerts`, `alert_deliveries`, `schema_version`.
- Custom `redb::Value`/`redb::Key` impls over postcard; secondaries store only 16-byte base pointers.
- Single-writer ingest API (agent), CLI read-only handle, ACID transactions, batch/group-commit writes.
- Time-bucket partitioning (configurable; hourly default), retention/cleanup at bucket granularity.
- Schema-version tag with **rebuild-by-WAL-replay** on mismatch (explicit gaps if WAL retention insufficient).

**Out:** DataFusion `TableProvider`s (T6); audit ledger DB (T8).

## Spec references

- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Storage, Data Model)
- file:.kiro/specs/daemoneye-core-monitoring/design.md §11.6–11.7; requirements R1.3, R4.4, R7.4, R20

## Key touchpoints

- file:daemoneye-lib/src/storage.rs — replace stubbed `DatabaseManager` + `Tables` (`Vec<u8>` placeholders) with typed tables, custom `redb::Value`/`redb::Key` impls over `postcard`, single-writer ingest + read-only handle, ACID/group-commit, time-bucket lifecycle, retention/cleanup, `schema_version` + rebuild-from-WAL.
- file:daemoneye-lib/src/models/ — `process.rs`/`alert.rs`/`rule.rs` types for value codecs.
- file:procmond/src/wal.rs — WAL is the rebuild replay source on schema mismatch (postcard-framed entries).
- Deps already present: `redb` 4.x, `postcard`. Benches: file:daemoneye-lib/benches/database_operations.rs.
- Tech Plan Data Model table (event store DB) and design.md §11.6–11.7 are the authoritative layout.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; checked arithmetic on key/offset math.
- Integration tests use real redb temp files (per testing steering); cover CRUD, index scans, retention, migrate/rebuild.
- Validate **> 1,000 records/sec** write rate and correct range/index results via criterion baseline.

## Dependencies

T

## Acceptance criteria

- All `DatabaseManager` methods persist/query real data; rules persist and reload (fixes "zero rules").
- **> 1,000 records/sec** write rate validated; range/index scans return correct rows.
- Schema-version mismatch triggers WAL-replay rebuild with explicit gap reporting; integration tests cover migrate/rebuild paths.
