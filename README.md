# DaemonEye

Security-focused process monitoring, written in Rust.

## Overview

DaemonEye is a Rust workspace that collects process information, runs detections against it, and exposes the results through a CLI. The repository contains:

- **procmond**: Privileged process monitoring daemon built on collector-core framework (binary + library)
- **daemoneye-agent**: User-space orchestrator with embedded EventBus broker and RPC service (binary)
- **daemoneye-cli**: Command-line interface for queries and management (binary)
- **daemoneye-lib**: Shared library with protobuf IPC and data models (library)
- **collector-core**: Extensible collection framework with EventSource trait (library)
- **daemoneye-eventbus**: Cross-platform IPC event bus with embedded broker and RPC patterns (library)

The workspace forbids unsafe code, treats clippy warnings as errors, and is covered by unit, property, and snapshot tests plus criterion benchmarks.

## Technology stack

- Language: Rust 2024 Edition (MSRV 1.91)
- Package manager/build: Cargo (workspace)
- Async runtime: tokio
- IPC: interprocess (cross-platform) with protobuf messaging
- Event Bus: daemoneye-eventbus (embedded broker for multi-collector coordination)
- CLI: clap v4
- Database: redb (embedded)
- Logging/telemetry: tracing, tracing-subscriber
- Config: figment (YAML/TOML/JSON + env vars)
- System info: sysinfo
- Benchmarks: criterion
- Testing: cargo-nextest, proptest, insta

## Project structure

```
DaemonEye/
├─ procmond/           # Process monitoring daemon (bin: procmond)
├─ daemoneye-agent/    # Orchestrator (bin: daemoneye-agent)
├─ daemoneye-cli/      # Command-line interface (bin: daemoneye-cli)
├─ daemoneye-lib/      # Shared library code (no bin)
├─ collector-core/     # Collection framework (library)
├─ daemoneye-eventbus/ # Cross-platform IPC event bus (library)
├─ justfile            # Developer tasks and scripts
├─ docs/               # Documentation sources (mdBook)
├─ spec/               # Specifications and design notes
└─ Cargo.toml          # Workspace manifest
```

## Requirements

- Rust toolchain 1.91+ (edition = 2024)
- Cargo (bundled with Rust)
- just task runner (optional but recommended)
- Optional developer tools: cargo-nextest, cargo-llvm-cov, cargo-audit, cargo-deny, cargo-release, goreleaser, mdbook (install via `just install-tools` and `just docs-install`)

## Setup

- Install Rust: <https://rustup.rs>
- (Optional) Install just: <https://github.com/casey/just>
- Initialize developer tools:
  - Windows: `just setup && just install-tools`
  - Unix: `just setup && just install-tools`

If you don't use just, the equivalent cargo commands are listed below.

## Commit signing

The workspace requires signed commits and uses a rebase-first sync strategy (configured via VS Code settings). See [`docs/src/contributing.md`](docs/src/contributing.md#commit-signing-and-gpg-setup) for GPG and git setup before contributing.

## Build and run

With just:

- Build all: `just build`
- Build release: `just build-release`
- Run procmond: `just run-procmond -- --help`
- Run agent: `just run-daemoneye-agent -- --help`
- Run CLI: `just run-daemoneye-cli -- --help`

With cargo (no just):

- Build all: `cargo build --workspace`
- Build release: `cargo build --workspace --release`
- Run procmond: `cargo run -p procmond -- --help`
- Run agent: `cargo run -p daemoneye-agent -- --help`
- Run CLI: `cargo run -p daemoneye-cli -- --help`

Notes:

- Binaries produced: procmond, daemoneye-agent, daemoneye-cli
- Distribution helpers exist via GoReleaser (see justfile: `goreleaser-*`).

## Configuration

Configuration is provided by daemoneye-lib using Figment with this precedence (highest to lowest):

1. Command-line flags (component binaries)
2. Environment variables with component prefix
3. User config file: ~/.config/daemoneye/config.yaml
4. System config file: /etc/daemoneye/config.yaml
5. Built-in defaults

Environment variable prefixes by component (replace dashes with underscores and uppercase):

- PROCMOND\_\*
- DAEMONEYE_AGENT\_\*
- DAEMONEYE_CLI\_\*

Nested keys can be provided using double underscores. Example: `PROCMOND_APP__SCAN_INTERVAL_MS=15000`.

Collector-specific environment overrides (used by collector-core) are additionally supported using a component-scoped prefix of the form "{COMPONENT}_COLLECTOR_...". Examples:

- PROCMOND_COLLECTOR_MAX_EVENT_SOURCES
- PROCMOND_COLLECTOR_EVENT_BUFFER_SIZE
- PROCMOND_COLLECTOR_ENABLE_TELEMETRY
- PROCMOND_COLLECTOR_DEBUG_LOGGING

Agent-specific environment toggle (used in tests/dev):

- DAEMONEYE_AGENT_TEST_MODE=1: enables test mode paths in the agent

Default configuration values are defined in code. See daemoneye-lib/src/config.rs and collector-core/src/config.rs for details and additional fields (database path, logging level/format, alert sinks, etc.).

## Scripts and developer tasks

Common tasks (from justfile):

- Format: `just fmt` (Rust), `just format-docs` (Markdown)
- Lint: `just lint` (clippy, docs, justfile), `just lint-rust-min`
- Tests: `just test` (nextest), `just test-all`, `just test-fast`
- Coverage: `just coverage`, `just coverage-check`
- Benches: `just bench`, plus component-specific bench tasks
- Run bins: `just run-procmond`, `just run-daemoneye-agent`, `just run-daemoneye-cli`
- Security: `just security-scan` (cargo-audit and cargo-deny)
- Dist/release: `just dist`, `just dist-plan`, `just release-*`

## Testing

- Runner: cargo-nextest (used by just tasks)
- Property-based tests: proptest
- Snapshot tests: insta
- Coverage: cargo-llvm-cov (generates lcov.info)

Run tests:

- With just: `just test` or `just test-all`
- With cargo-nextest directly: `cargo nextest run --workspace`

Benchmarks use criterion. Run via `just bench` or `cargo bench`.

## Environment variables

Summary of notable env vars:

- PROCMOND\_\* / DAEMONEYE_AGENT\_\* / DAEMONEYE_CLI\_\*: general config via Figment (use `__` for nested keys)
- PROCMOND_COLLECTOR_MAX_EVENT_SOURCES, PROCMOND_COLLECTOR_EVENT_BUFFER_SIZE, PROCMOND_COLLECTOR_ENABLE_TELEMETRY, PROCMOND_COLLECTOR_DEBUG_LOGGING: collector-core overrides
- DAEMONEYE_AGENT_TEST_MODE: agent test mode

Additional OS/environment variables may be referenced in tests for compatibility checks (e.g., HOSTNAME, USER, OS), but they are not required for normal operation.

## Platform notes

- **Unix**: IPC over Unix domain sockets via the `interprocess` crate, using protobuf messages.
- **Windows**: IPC over named pipes via the `interprocess` crate, using protobuf messages.
- **Event bus**: `daemoneye-eventbus` is local-only pub/sub between collectors and the agent on the same host. Topics are hierarchical and messages carry correlation metadata.
- **Embedded broker**: `daemoneye-agent` runs the EventBus broker in-process. Workflow tracking and forensic correlation are built on top of it.
- **RPC services**: Collector lifecycle (start, stop, restart, health checks) is exposed over RPC on the bus with request timeouts and correlation IDs.
- **Multi-collector coordination**: Tasks are distributed by topic and routed by collector capability; results are aggregated across collector types.

## Documentation

- Docs sources live under ./docs (mdBook). Build locally with `just docs-install` then `mdbook build docs`.

## License

Apache License 2.0. See LICENSE for full text.

## Roadmap and status

This README describes what is in the repository today (workspace crates, justfile tasks, configuration surface). Several features in the spec, including some alerting sinks and centralized management, are not yet implemented.

Open items for this README:

- Document the concrete command-line flags for each binary once they stabilize.
- Add an end-to-end quickstart with an example config and sample output.
- Publish prebuilt binaries via GoReleaser and link them here.
