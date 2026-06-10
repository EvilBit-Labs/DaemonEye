# DaemonEye Backlog

> Ordered, trackable view of remaining work for the OSS Community tier, toward **v1.0**.

**Source of truth:** `.kiro/specs/daemoneye-core-monitoring/tasks.md`. This file is a *synthesis* of that spec — it de-duplicates overlapping tasks, folds superseded ones, and sequences the remainder by dependency. When the two disagree on detail, the spec wins; when they disagree on *order*, this file reflects the 2026-06-09 design review (see [Sequencing notes](#sequencing-notes--open-questions)).

**How to use it:** work top-to-bottom. Milestones are dependency-ordered — later milestones assume earlier ones land first. Within a milestone, items are listed in suggested execution order. Check the box when a task's spec acceptance criteria are met and tests pass.

**Traceability:** each line keeps its `tasks.md` number (e.g. `T12.3`) and GitHub issue (`#NN`) where one exists, so you can jump back to the full requirement text and `_Requirements:_` mapping.

**Legend:** `[ ]` not started · `[~]` partially done · `[x]` done. v1.0 dates are aspirational (sole maintainer) — this file tracks *order and dependency*, not deadlines.

---

## Status at a glance

| Area                                                                                                    | State                                                                                                   |
| ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Foundation (workspace, data models, collector-core, eventbus, process collection, procmond integration) | ✅ done                                                                                                 |
| Storage & IPC backbone                                                                                  | 🔄 partial — DB ops + agent/collector IPC remain                                                        |
| SQL-to-IPC detection engine                                                                             | ❌ not started — spec merged into core spec 2026-06-09 (R17–R24, Task 7.1–7.10, ADR-0006 DataFusion)    |
| Alerting delivery                                                                                       | 🔄 partial — gen + stdout/file + parallel delivery done; network sinks, reliability, correlation remain |
| Audit ledger                                                                                            | ❌ not started (#42)                                                                                    |
| Privilege & service management                                                                          | ❌ not started                                                                                          |
| Operator CLI                                                                                            | 🔄 partial — base CLI done; query/management/health remain                                              |
| Integration, observability, hardening                                                                   | ❌ not started (gated on the above)                                                                     |

---

## ✅ Completed foundation

Already landed (see `tasks.md` for detail). Listed so the backlog shows the floor we build on:

- **T1** Monitor Collector testing & validation (unit, integration, property, chaos, perf, e2e, security) — incl. GH #89 stack-overflow fix
- **T2 (2.1–2.6)** Full migration from crossbeam to **daemoneye-eventbus** message broker — crate, transport, embedded broker, RPC lifecycle, multi-collector coordination, topic hierarchy, correlation metadata
- **T4** procmond ↔ collector-core integration over daemoneye-eventbus
- **T11.1** Basic `DatabaseManager` structure & error handling — #43
- **T12.1** Basic `DetectionEngine` + sqlparser AST validation — #47
- **T13** Alert generation & management (`AlertManager`, dedup, severity) — #51
- **T14.1** `AlertSink` trait + stdout/file sinks + formatting — #52
- **T14.3** Parallel alert delivery + delivery tracking — #54
- **T16.1** Basic clap CLI + DB stats + output formats — #56
- **T3 (core)** Executable hashing: `HashComputer` trait + procmond `--compute-hashes` (remaining scope tracked in M1)

---

## 🎯 Road to v1.0 (ordered)

### M1 — Finish in-flight foundation

- [ ] **T3 (remaining)** Integrity hashing: add criterion baselines for hashing impact on enumeration; verify graceful handling of missing/inaccessible executables without failing enumeration. *R2.1, 2.2, 2.4*
- [ ] **T2.7** Remove crossbeam `HighPerformanceEventBus` (`high_performance_event_bus.rs` + dep) once the eventbus in-process path meets the **R14 AC4** no-regression perf budget. Decision 2026-06-09: single event-routing layer, no dual-bus end state.

### M2 — Storage & IPC backbone (unblocks detection, query, integration)

> Design review: IPC (15.1/15.2) are prerequisites for end-to-end integration (M9) — bring them forward, ahead of alerting work.

- [ ] **T15.1** Implement `CollectorIpcServer` in `collector-core/src/ipc.rs` — capability negotiation, task routing, connection mgmt, tests. — #78. *R11.1, 11.2, 12.1, 12.2*
- [ ] **T15.2** Complete `IpcClientManager` in daemoneye-agent — reconnection, capability negotiation, task distribution/result collection; compatible with procmond `ProcessMessageHandler`. — #77. *R3.1, 3.2, 11.1, 11.2*
- [ ] **T11.2** Real redb operations — replace placeholder TODOs, serialization for ProcessRecord/DetectionRule/Alert, indexing, transactions, integration tests. — #44. *R1.3, 4.4, 7.4*
- [ ] **T11.3** redb migration system — schema versioning, shared `Arc` handle, recovery/rollback, retention/cleanup. — #45. *R1.3, 4.4, 7.4*

### M3 — SQL-to-IPC detection engine (core value)

> Canonical breakdown: **T7.1–T7.10** in `tasks.md` (merged 2026-06-09 from the former `upcoming-specs/sql-to-ipc-detection-engine/`, rewritten per **ADR-0006 DataFusion**; governing requirements **R17–R21**). MVP gate order: 7.1 → 7.8; 7.9–7.10 parallel after 7.6. Folds the older T12.2/T12.3/T12.5 framing; T12.4 survives as the collector-side counterpart.

- [ ] **T7.1** SQL analyzer module — refactor `daemoneye-lib/src/detection/sql_to_ipc.rs`, sqlparser AST validation, function allowlist, subquery depth limit, typed `CollectionRequirements` migration. *R3, R17*
- [ ] **T7.2** Regex compile/cache — linear-time `regex` crate w/ size limits, AST-before-compile, pattern-keyed LRU, latency flag/disable + metrics. *R17*
- [ ] **T7.3** Static schema catalog — authenticated startup registration (peer creds/spawn token), table-reference validation, re-validate/re-plan on descriptor change w/ unhealthy-rule surfacing. *R19*
- [ ] **T7.4** Pushdown planner — capability-based protobuf `DetectionTask`s, conformance vectors (unverified ops run agent-side), TTL renewal + re-issue on collector re-registration. *R18*
- [ ] **T7.5** redb detection storage — time-partitioned tables, fixed-width keys, multimap secondary indexes (spec §11.7), batch/group-commit writes. *R20*
- [ ] **T7.6** DataFusion integration — per-collector redb `TableProvider`s w/ filter/projection pushdown, `SessionContext` allowlist + memory limits + cardinality caps, derived-SQL only, binary-size/memory gate vs \<100MB budget. *R3, R20*
- [ ] **T7.7** Degradation markers & reliability — completeness markers ("no match" vs "could not fully evaluate"), disconnect handling, (task_id, seq_no) replay dedup w/ grace period, shed counting, seq-no recovery. *R20, R21*
- [ ] **T7.8** Agent integration — extract `DetectionEngine` trait, replace placeholder detection path, wire `ResilientIpcClient` + `BrokerManager` capability negotiation. *R3, R18, R19*
- [ ] **T7.9** Detection metrics & execution plans — tracing ecosystem (no Prometheus crate in v1.0), on-demand plan output. *R10*
- [ ] **T7.10** Operator rule-authoring docs — dialect + documented-extension policy, first-rule tutorial, regex guidance, CLI rule testing. *R8, R17*
- [ ] **T12.4** Extend collector-core for SQL-to-IPC task handling — `EventSource` accepts `DetectionTask`, capability advertisement, schema-registry integration, task validation (collector-side counterpart of T7.3/T7.4). *R18, R19*

### M4 — Alerting delivery (detection → operator)

> Alert generation (T13) is done; this milestone gets alerts *out* reliably. T26.1 folds into T14.2 + T15.

- [ ] **T14.2** Network alert sinks — `WebhookSink` (HTTP POST + auth), `SyslogSink`, `EmailSink` (SMTP + templates), mock-endpoint integration tests. — #53. *R5.1, 5.2*
- [ ] **T15** Delivery reliability — per-sink circuit breakers, exponential backoff + jitter retries, dead-letter queue, success-rate metrics, chaos tests. — #55. *R5.2, 5.3, 5.4, 5.5*
- [ ] **T26.2** Alert correlation & context enrichment — cross-rule/cross-collector correlation via eventbus, process-ancestry/system-state enrichment, severity escalation, forensic metadata, timeline reconstruction. *R5.x, 15.4, 16.3*

### M5 — Audit ledger (forensic integrity)

- [ ] **T10** Tamper-evident audit logging — `AuditChain` (BLAKE3 + rs_merkle), append-only ledger w/ monotonic sequence in separate redb, entry structure, chain verification, periodic Ed25519 checkpoints, **Merkle inclusion proofs** (currently stubbed in `crypto.rs`), recovery/consistency checking, audit hooks in `ProcessMessageHandler`. — #42. **Supersedes T8.** *R7.1–7.5*

### M6 — Privilege & service management (deployable daemon)

- [ ] **T5** Privilege management & boundaries — `PrivilegeManager`, optional enhanced privilege requests (CAP_SYS_PTRACE / SeDebugPrivilege / macOS entitlements), immediate drop after init w/ audit logging, security tests. *R6.1, 6.2, 6.4, 6.5*
- [ ] **T6.1** Cross-platform service infrastructure — `ServiceManager` trait, `UnixServiceManager` / `WindowsServiceManager`, service config struct, agent service wrapper. *R6.x*
- [ ] **T6.2** Agent as system service + collector supervision — Unix daemon + Windows SCM, collector lifecycle/health/restart via eventbus RPC, graceful shutdown via control topics. *R6.x, 11.x, 15.2, 15.5*
- [ ] **T6.3** Integrate service modes into agent `main.rs` — interactive + service modes, `--install/--uninstall/--start/--stop/--status`, service logging w/ rotation. *R6.x, 11.x, 14.2, 14.5*
- [ ] **T6.4** Service monitoring & health reporting — health endpoints, component health aggregation via broker metrics, recovery actions. *R10.x, 15.2*
- [ ] **T6.5** Deployment & config management — systemd units, launchd plists, Windows installer, config templates, install/uninstall/upgrade procedures, per-platform docs. *R6.x*
- [ ] **T6.6** Service testing & validation — install/start/stop on all platforms, failure/recovery, upgrade w/ migration, security, baselines; validate GH #89 acceptance criteria. *All requirements verification*

### M7 — Operator CLI

> T16.1 base is done. **T9.1/T9.2** (older framing) fold into **T16.3** (query) and **T17** (management).

- [ ] **T16.2** CLI testing & robustness — insta snapshot tests, error handling, config-file support, shell completions (bash/zsh/fish/PowerShell). — #79. *R8.1, 8.2, 10.5*
- [ ] **T16.3** Advanced query — IPC client to daemoneye-agent, single-query execution + explain/validate/history, streaming/pagination, parameter binding, NO_COLOR/TERM handling. — #80. *(interactive REPL/shell features deferrable to post-v1.0 per design review)* *R8.1, 8.2, 10.5*
- [ ] **T17** Management commands — `rules`, `alerts`, `health`, `data`, `config`, `service` subcommands; YAML config hierarchy; snapshot tests. — #57. *R8.2–8.5*
- [ ] **T18** Health monitoring & diagnostics — `HealthChecker`, color-coded overview, baseline metrics reporting, config validation w/ guidance. — #58. *R8.4, 8.5, 10.2, 10.3*

### M8 — Offline-first & bundles

- [ ] **T19** Offline operation + bundle support — all core functionality works without network, graceful alert-delivery degradation, signed bundle config/rule distribution w/ validation + atomic apply, airgapped integration tests. — #59. *R9.1, 9.2, 9.4, 9.5*

### M9 — End-to-end integration (wires M2–M8 together)

- [ ] **T25.1** Wire IPC between components via collector-core — task distribution/result collection through runtime, preserve protobuf+CRC32 framing. — #63
- [ ] **T25.2** Rule translation & execution pipeline — connect SQL-to-IPC (M3) + collector-core event sources to alert generation. — #63
- [ ] **T25.3** Alert generation → delivery pipeline — dedup, priority, delivery-status tracking, e2e tests. — #63
- [ ] **T25.4** Unified configuration & service management across all three components. — #63

### M10 — Observability

- [ ] **T21** Prometheus-compatible metrics (collection rate, detection latency, alert delivery), structured logging w/ correlation IDs, localhost-only HTTP health endpoints, resource-utilization + error-rate tracking, scraping-compat tests. — #60. **Folds T27.1/T27.2.** *R10.1–10.4*

### M11 — Hardening: tests, benchmarks, stress, security

> Design review: benchmarks (T20) and stress tests (T23) target code that doesn't fully exist yet — keep them **after M9 integration**. T20.1 (process enum) and T20.3 (IPC) may start earlier against existing code.

- [ ] **T22.1** Unit coverage >85% w/ llvm-cov, mocks, edge cases. — #61
- [ ] **T22.2** Integration & CLI snapshot tests (insta), cross-component IPC, e2e workflows. — #61
- [ ] **T22.3** Performance & property-based tests (proptest) for baselines on critical paths. — #61
- [ ] **T22.4** CI matrix & quality gates — OS/arch/Rust-version matrix, fmt/clippy/audit/deny, coverage, SLSA provenance, signed release pipeline. — #61
- [ ] **T20.1–T20.8** Criterion benchmark suite — process enum, hashing, IPC, DB, alerting, SQL/detection, collector-core, plus regression detection + CI integration (T20.8).
- [ ] **T23.1–T23.6** Stress & load testing — collector-core, process enum, DB, alert delivery, IPC, system-wide integration.
- [ ] **T24** Advanced security testing — SQL-injection vectors, privilege-boundary verification, cargo-fuzz, Miri/AddressSanitizer, IPC pen-testing, audit-chain tampering detection. — #62

---

## 🔭 Post-v1.0 roadmap

Strategic extensions on the established collector-core + SQL-to-IPC patterns:

- [ ] **F1** Network collector (netmond) — `network_connections`, `network_interfaces`
- [ ] **F2** Filesystem collector (fsmond) — `file_events`, `file_metadata`
- [ ] **F3** Performance collector (perfmond) — `system_metrics`, `resource_usage`
- [ ] **F4** Triggerable collectors — binary hasher, memory analyzer, YARA scanner, PE analyzer
- [ ] **F5** Specialty collectors for advanced pattern matching (YARA / network / PE analysis) — **depends on SQL-to-IPC engine (M3)**; signing constraints per **R23**
- [ ] **F6** Advanced cascade/correlation integration — AUTO JOIN (R22) and reactive cascading analysis (R24, bounded queues + depth-5 circuit breakers)
- [ ] **F7** Sigma→SQL rule import converter — community detection-content interop (deliberate v1.0 exclusion; see requirements.md Open Questions)
- [ ] **T16.3 (REPL slice)** Interactive query shell — syntax highlighting, auto-completion, command history

---

## Sequencing notes & open questions

From the **2026-06-09 design review** (`tasks.md` › *Deferred / Open Questions*):

- **IPC before alerting/DB.** T15.1/T15.2 are prerequisites for Task 25 integration but were sequenced after the alerting stack — pulled forward into **M2** here.
- **Defer benchmarks & stress.** Most of T20 (except 20.1/20.3) and all of T23 target code that doesn't exist yet — placed in **M11**, after end-to-end integration.
- **CLI REPL post-v1.0.** Interactive shell features of T16.3 are split out to the post-v1.0 roadmap; single-query execution stays in **M7**.
- **Crossbeam end-state (resolved).** Full removal (R14 end-state option a). daemoneye-eventbus is the single routing layer; in-process needs use plain tokio channels. Crossbeam `HighPerformanceEventBus` is a sanctioned *transitional* hot path until the eventbus in-process route meets R14 AC4 — then removed (**T2.7 / M1**).
- **SQL-to-IPC task ownership (resolved 2026-06-09).** The former `sql-to-ipc-detection-engine` upcoming-spec was merged into the core spec and deleted: requirements became **R17–R24**, the design lives in design.md's "Detection Engine (SQL-to-IPC, ADR-0006)" section, and the post-ADR task breakdown is **T7.1–T7.10** (reflected in M3 above).

### Duplicate / superseded task map

`tasks.md` accreted overlapping items over time. This backlog uses the canonical task and folds the rest:

| Area                 | Canonical               | Folded / superseded                                                       |
| -------------------- | ----------------------- | ------------------------------------------------------------------------- |
| Audit ledger         | **T10** (#42)           | T8 (superseded)                                                           |
| SQL-to-IPC engine    | **T7.1–7.10** (+T12.4)  | T12.2, T12.3, T12.5 (older framing); former upcoming-spec tasks (deleted) |
| Alerting             | **T14.2 + T15 + T26.2** | T26.1 (≈ T14.2 + T15), T9 alerting bits                                   |
| Observability        | **T21** (#60)           | T27.1, T27.2                                                              |
| CLI query/management | **T16.3 + T17**         | T9.1, T9.2                                                                |
