# T9 · M6 — Privilege & service management (PrivilegeManager + ServiceManager)

**Milestone:** M6 · **Backlog:** T5, T6.1–T6.6

## Scope

**In:**

- `PrivilegeManager` in procmond: optional enhanced privilege requests (CAP_SYS_PTRACE/SeDebugPrivilege/macOS entitlements), immediate post-init drop to minimal retained set with audit logging (replaces `broker_manager.rs` stub).
- **Agent bootstrap exception:** agent may start elevated only for service setup + procmond spawn, then **must drop before broker steady-state/collection**.
- `ServiceManager` trait + `UnixServiceManager`/`WindowsServiceManager`; daemon/SCM modes; `--install/--uninstall/--start/--stop/--status`; collector supervision/restart via eventbus RPC; service logging w/ rotation.
- Deployment: systemd units, launchd plists, Windows installer, config templates, install/uninstall/upgrade; cross-platform service tests.

**Out:** HTTP health endpoint (opt-in, M10); CLI `service` subcommand surface (T10).

## Spec references

- spec:.../811199cd-23dc-4ded-9932-735252ec2438 (Flow 1)
- spec:.../bfe4676f-c557-4668-ac18-6aa1f6bff7a6 (Privilege & service; ServiceManager)
- requirements R6.1–R6.5

## Key touchpoints

- file:procmond/src/security.rs — grow `detect_privileges` into a real `PrivilegeManager` with platform request + immediate post-init drop to declared retained set + audit logging.
- file:daemoneye-agent/src/broker_manager.rs (`drop_privileges()` stub → real), file:daemoneye-agent/src/main.rs — implement the bootstrap exception: elevate only for service setup + procmond spawn, then drop before broker steady-state/collection.
- New `ServiceManager` trait + `UnixServiceManager`/`WindowsServiceManager` in the daemoneye-agent crate; `--install/--uninstall/--start/--stop/--status`; supervision/restart via eventbus RPC (`control.collector.*`); log rotation.
- Deps: `caps` (Linux capability drop, new), `windows-service`/`windows`/`winreg` (present), macOS entitlements. Deployment: systemd units, launchd plists, Windows installer (existing WiX file:daemoneye-agent/wix/main.wxs), config templates.
- Audit integration: privilege changes recorded via T8 ledger.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; `unsafe_code="forbid"` preserved (use safe `caps`/`windows` wrappers).
- Security tests verify procmond retained-set and agent drop-before-steady-state; cross-platform install/start/status/stop/uninstall + supervised restart integration tests.

## Dependencies

T2 (eventbus RPC lifecycle in place).

## Acceptance criteria

- procmond drops to declared retained capability set post-init; agent drops before steady-state; both changes audited (ties to T8).
- Install/start/status/stop/uninstall work on Linux/macOS/Windows; supervised restart of procmond verified.
