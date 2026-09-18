# T9 · M6 — Privilege & service management (PrivilegeManager + ServiceManager)

**Milestone:** M6 · **Backlog:** T5, T6.1–T6.6

## Scope

**In:**

- `PrivilegeManager` in procmond: optional enhanced privilege requests (CAP_SYS_PTRACE/SeDebugPrivilege/macOS entitlements), immediate post-init drop to minimal retained set with audit logging (replaces the `detect_privileges` stub in `security.rs`).
- **Agent bootstrap exception:** agent may start elevated only for service setup + procmond spawn, then **must drop before broker steady-state/collection**. The drop is **fail-closed**: if it fails, startup aborts. Continuing elevated with a warning (today's behavior in file:daemoneye-agent/src/main.rs) is not acceptable.
- The agent's drop must reach a dedicated non-root identity, not only a reduced capability set — a Linux capability drop alone leaves the process running as its original UID. Because the post-drop agent must still be able to restart a crashed procmond, install must leave procmond launchable with its required privileges by an unprivileged parent (e.g. file capabilities on the binary on Linux, with the equivalent named for the Windows service and launchd paths); state the mechanism per platform.
- `ServiceManager` trait + `UnixServiceManager`/`WindowsServiceManager`; daemon/SCM modes; `--install/--uninstall/--start/--stop/--status`; collector supervision/restart via eventbus RPC; service logging w/ rotation.
- Deployment: systemd units, launchd plists, Windows installer, config templates, install/uninstall/upgrade; cross-platform service tests.

**Out:** HTTP health endpoint (opt-in, M10); CLI `service` subcommand surface (T10).

## Spec references

- spec/full/specs/Core_Flows\_—\_DaemonEye_Operator_Journeys.md (Flow 1)
- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(Privilege & service; ServiceManager)
- requirements R6.1–R6.5

## Key touchpoints

- file:procmond/src/security.rs — grow `detect_privileges` into a real `PrivilegeManager` with platform request + immediate post-init drop to declared retained set + audit logging.
- file:daemoneye-agent/src/broker_manager/state_machine.rs (`drop_privileges()` stub → real), file:daemoneye-agent/src/main.rs — implement the bootstrap exception: elevate only for service setup + procmond spawn, then drop before broker steady-state/collection.
- **Before writing the platform layer, evaluate the `service-manager` crate** (license, maintenance, and whether it covers install/uninstall/start/stop/status plus systemd/launchd/SCM unit generation). Adopt it if it fits; the eventbus-RPC supervision layer stays DaemonEye's own code either way. If it does not fit, record why and hand-roll.
- `ServiceManager` trait + `UnixServiceManager`/`WindowsServiceManager` in the daemoneye-agent crate; `--install/--uninstall/--start/--stop/--status`; supervision/restart via eventbus RPC (`control.collector.*`); log rotation.
- Deps: `caps` (Linux capability drop, new), `windows-service`/`windows`/`winreg` (present), macOS entitlements. Deployment: systemd units, launchd plists, Windows installer (existing WiX file:daemoneye-agent/wix/main.wxs), config templates.
- Audit integration: privilege changes recorded via T8 ledger.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; `unsafe_code="forbid"` preserved (use safe `caps`/`windows` wrappers).
- Security tests verify procmond retained-set and agent drop-before-steady-state; cross-platform install/start/status/stop/uninstall + supervised restart integration tests.

## Dependencies

T2 (eventbus RPC lifecycle in place), T8 (audit ledger — the privilege-change audit criterion is unverifiable without it).

## Acceptance criteria

- procmond drops to declared retained capability set post-init; agent drops before steady-state to a non-root identity; a failed drop aborts startup rather than continuing elevated; both changes audited (ties to T8).
- Supervised restart of a crashed procmond is demonstrated with the agent already unprivileged.
- Install/start/status/stop/uninstall work on Linux/macOS/Windows; supervised restart of procmond verified.
