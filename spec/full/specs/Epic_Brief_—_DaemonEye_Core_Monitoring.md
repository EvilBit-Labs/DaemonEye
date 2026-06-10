# Epic Brief — DaemonEye Core Monitoring

# Epic Brief — DaemonEye Core Monitoring

## Summary

DaemonEye (OSS Community tier) has a strong, tested **foundation** — collector-core SDK, the daemoneye-eventbus broker (pub/sub + RPC, multi-collector coordination), cross-platform process collection, and procmond↔collector-core integration — but the **value-delivering middle is hollow**. Storage is stubbed (`DatabaseManager` methods are TODOs returning empty/`None`), detection is a substring/category **placeholder** rather than the real SQL→DataFusion engine, the audit ledger is **in-memory only** with a stubbed Merkle inclusion proof, and there is no daemon/service deployment or real privilege drop (`drop_privileges()` is a stub). As a result, DaemonEye **cannot yet run as a deployable, end-to-end, auditable monitoring product** despite the foundation being ready.

This Epic closes that foundation→product gap by finishing the stubbed middle so the existing parts become a usable whole. It re-frames the **entire** file:.kiro/specs/daemoneye-core-monitoring/requirements.md roadmap (R1–R24, which already absorbs the former SQL-to-IPC spec and binds detection to ADR-0006: Apache DataFusion over redb TableProviders). Work is **internally phased**: early phases deliver the **v1.0 bar** (core R1–R10 + R17–R21, plus enabling R11/R14), starting with the four priority areas — **storage, detection engine, audit ledger, and privilege/service management** — followed by alerting delivery, operator CLI, offline/bundles, end-to-end integration, observability, and hardening. Later phases carry **post-v1.0** scope (R22–R24 auto-join/specialty/reactive; F1–F7 future collectors). The codebase is the source of truth; verified spec/backlog/code/doc drift is reconciled along the way as a constraint, not a headline.

## Context & Problem

**Who's affected**

- **Security operators / threat hunters** in airgapped or contested environments who need a passive, offline-capable, cryptographically auditable process-monitoring daemon — and today cannot persist data, run real SQL detections, or deploy it as a service.
- **The solo maintainer** driving toward OSS v1.0, who needs one coherent, dependency-ordered initiative rather than a sprawling, partly-stale task list.

**Where in the product (current pain)**

- **Storage (**file:daemoneye-lib/src/storage.rs**)** — every persist/query method is a TODO; `Vec<u8>` placeholder tables; no schema, indexing, transactions, retention, or migrations. This blocks rule persistence and the agent's detection loop (which currently loads **zero rules**).
- **Detection (**file:daemoneye-lib/src/detection/mod.rs**)** — naive `name.contains("suspicious")` / `cpu_usage > 80` matching; SQL is validated at load time but never executed; the DataFusion-over-redb engine (R17–R21) is unbuilt and unwired into the agent loop.
- **Audit ledger (**file:daemoneye-lib/src/crypto.rs**)** — BLAKE3 hash chain exists but is in-memory, has no persisted `audit_ledger` table, `generate_inclusion_proof()` returns `vec![]`, and there are no Ed25519 checkpoints; architecture also expects procmond ownership.
- **Privilege & service mgmt (**file:daemoneye-agent/src/main.rs**, **file:daemoneye-agent/src/broker_manager.rs**)** — foreground process, Ctrl+C-only shutdown, `--database/--log-level` only; no daemon/service modes, no install/supervision, and privilege dropping is an explicit stub (procmond detects CAP_SYS_PTRACE/root but never drops).

**Constraints (not the headline)**

- Keep spec, file:BACKLOG.md, code, and docs coherent — reconcile residual drift (e.g., AGENTS.md still lists Event Bus = `crossbeam` though the code uses daemoneye-eventbus; `tasks.md` duplicate task numbers) opportunistically as touched.
- Honor project invariants: offline/airgap-first, no inbound network (TCP opt-in only), `unsafe_code = "forbid"`, and performance budgets (\<5% CPU, \<100MB RSS, \<100ms/rule).
- Apply least privilege with one explicit bootstrap exception: daemoneye-agent may start elevated only to install/configure service resources and spawn procmond, then must drop privileges before broker steady-state and before collection begins; procmond retains only its declared minimal steady-state capability set.

**Goal of this Epic** Make DaemonEye a deployable, end-to-end, auditable monitoring daemon at v1.0 — and frame the path beyond it — by completing the stubbed core so the proven foundation becomes a usable product.

**v1.0 success condition** A host can install/start DaemonEye as a supervised service, collect and persist process data, record executable integrity metadata, import signed rule/config bundles offline, run validated SQL detections, generate/store alerts, deliver alerts reliably through configured sinks with local degradation behavior, query results, verify audit integrity locally, and inspect local health/diagnostic status including collection rate, rule evaluation latency, alert delivery health, and redb write latency. \</TRAYCER_SPEC>
