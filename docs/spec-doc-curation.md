# Spec Document Curation

> Tracking doc for reconciling spec/design/architecture documents that live **outside `.kiro/`** against the canonical `.kiro/specs/` + `.kiro/steering/` source of truth. Created 2026-06-09. `.kiro/` always wins on conflict.

## How this was produced

Swept the repo for spec/design/ADR/architecture documents outside `.kiro/`, then compared each against the relevant canonical `.kiro/` content to classify it as **EXTENDS** (adds detail `.kiro` lacks) / **CONFLICTS** (contradicts `.kiro` or reality) / **SUPERSEDES** / **DUPLICATE** / **INDEPENDENT** / **STALE** / **EPHEMERAL**. Excluded from scope: instruction/policy files (AGENTS.md, CLAUDE.md, `.github/*`), READMEs, `docs/solutions/**`, `todos/**`, `.full-review/**`, and mdbook guide pages.

## Cross-cutting findings

1. **Serialization was mis-documented everywhere.** Actual wire format is **postcard** (only serde codec in `daemoneye-eventbus`; `bincode` appears nowhere in source). `.kiro/design.md` said "Bincode" (**fixed**), `docs/src/technical/eventbus-architecture.md` says bincode, and `daemoneye-eventbus/docs/message-schemas.md` describes a protobuf envelope.
2. **Detection engine documented as fiction.** `core-monitoring.md`, `architecture.md`, and `system-architecture.md` show SQL running directly against redb via `db.prepare(...)` / `SqliteAuditLogger` — redb has no such API. Canonical model (ADR-0006) routes SQL through **DataFusion `TableProvider`s**. `core-monitoring.md` also uses the pre-0.62 `SelectItem::Wildcard` unit variant (the regression class fixed in `rule.rs`).
3. **ADR-0006 + upcoming-specs merged into the main spec** (done by maintainer 2026-06-09). The prior "create an ADR-0006 file" / "delete upcoming-specs" recommendations are moot. The detailed `spec/daemon_eye_spec_sql_to_ipc_detection_architecture.md` is now a *mirror* of canonical content to reconcile against the main spec, not the sole ADR home.
4. **Open-core leakage** in `docs/src/project-overview.md` (Business/Enterprise PostgreSQL line; false mTLS/cert-registration claim) and `docs/src/security.md` (JWT/RBAC/TLS-listener/firewall config that contradicts the outbound-only / no-inbound-network model).
5. **`docs/plans/` is gitignored** — those implementation plans are local-only working artifacts, already effectively archived; left in place (the message-broker plan is cited by code comments in `procmond/src/main.rs` and `daemoneye-eventbus/tests/e2e_multi_collector.rs`).

## Actions completed

- [x] Archived 3 tracked eventbus build-out artifacts → `docs/archive/eventbus-buildout/`
- [x] Fixed `.kiro/specs/daemoneye-core-monitoring/design.md`: Bincode → postcard
- [x] (maintainer) Merged ADR-0006 + upcoming-specs into the main `.kiro` spec; deleted `upcoming-specs/`
- [x] `.kiro/steering/tech.md` — added eventbus **postcard** serialization line (with protobuf = CLI-IPC-only clarification) and a **Detection SQL Execution** subsection (DataFusion over redb TableProviders, ADR-0006, [Planned] status)
- [x] `.kiro/steering/structure.md` — detection-engine and Access Patterns lines updated to the DataFusion/TableProvider model (no more "database connections" framing)
- [x] `spec/adr-ipc-interprocess.md` — status flipped to **Accepted** (Phase 1 implemented; Phase 2 evaluation pending); broken `DaemonEye-core-monitoring` link case fixed (ADR-home relocation still open below)

## Conflict fixes — remaining

- [ ] `docs/src/technical/eventbus-architecture.md` — bincode → postcard
- [ ] `daemoneye-eventbus/docs/message-schemas.md` — reconcile protobuf-envelope description with the postcard/struct reality (rewrite, or mark protobuf "Planned / not the current wire format")
- [ ] `docs/src/project-overview.md` — scrub "Business/Enterprise: PostgreSQL…" line; reconcile false "mTLS between components / certificate-based agent registration"; mark Merkle inclusion proofs In Progress
- [ ] `docs/src/security.md` — remove/flag fictional JWT/RBAC/TLS-listener/firewall config blocks (no inbound network exists)
- [ ] `docs/src/technical/core-monitoring.md` — remove `db.prepare`/`SelectItem::Wildcard` fiction; point to ADR-0006 DataFusion model
- [ ] `docs/src/technical/system-architecture.md` — fix `SqliteAuditLogger` name + direct-redb-SQL snippet (ADR-0006); make this the canonical published architecture page
- [ ] `docs/src/architecture.md` — fix "Certificate Transparency" → BLAKE3 hash-chained ledger; six-crate + eventbus model; **dedupe with system-architecture.md** (collapse to one)
- [ ] `docs/src/technical/query-pipeline.md` — refresh Phase-2 to the DataFusion model
- [ ] `docs/crossbeam-to-daemoneye-eventbus-migration.md` — reframe to single-bus end state (R14, 2026-06-09: full crossbeam removal, no dual-bus), or demote to historical log with a banner
- [ ] `docs/embedded-broker-architecture.md` — add the canonical broker auth-token model (32-byte `getrandom` token, `0400` file by path, publisher authorization on `control.*`) + restart-recovery section

## Dedupe candidates

- [ ] `daemoneye-eventbus/docs/topic-hierarchy.md` ↔ `topic-hierarchy-design.md` (keep API-accurate former; salvage the matcher truth-table note)
- [ ] `docs/src/architecture.md` ↔ `docs/src/technical/system-architecture.md` (collapse to one)
- [ ] `docs/src/security.md` ↔ `docs/src/technical/security_design_overview.md` (two full threat-model treatments)

## Keep — healthy EXTENDS / reference docs (no action beyond noted nits)

`spec/daemon_eye_spec_sql_to_ipc_detection_architecture.md` (reconcile vs merged main spec) · `spec/adr-ipc-interprocess.md` (flip status to Accepted; fix broken requirements.md link; decide ADR home) · `docs/protobuf-ipc.md` (verify `prost` version vs Cargo.toml) · `docs/src/technical/ipc-implementation.md` (cleanest doc; optional 107-byte `sun_path` note) · `docs/src/technical/sql-dialect-reference.md` (fix stale flat-`processes` schema fragment) · `docs/src/architecture/collector-core-framework.md` (reconcile `Collector::new` sync/async) · `docs/src/technical/macos-process-collector.md` · `docs/src/technical/windows-process-collector.md` (drop stale Task 5.x framing) · `daemoneye-eventbus/docs/{correlation-metadata,integration-guide,process-management,rpc-patterns,task-distribution}.md` · `docs/src/technical/rpc-eventbus-architecture.md` (fix `tags` → `correlation_tags`) · `docs/src/technical/security_design_overview.md` (reconcile aspirational FIPS/FedRAMP claims with impl status)

## Structural follow-ups

- [ ] Decide an **ADR home** (`docs/adr/` or `.kiro/adr/`) and relocate/renumber `spec/adr-ipc-interprocess.md` — note: the maintainer's ADR registry is numbered externally (ADR-0006/0007 exist there); public mirrors live in `spec/` today, so relocation should preserve that numbering
- [x] Update AGENTS.md Source-of-Truth Map if it still calls the SQL-to-IPC merge "pending" — done 2026-06-09 (row now points at R17–R24, notes `upcoming-specs/` deleted)
- [x] `BACKLOG.md` Task 7 section regenerated from the updated `tasks.md` — done 2026-06-09 (M3 = T7.1–T7.10 DataFusion-first, fold map updated, F6/F7 reframed)
