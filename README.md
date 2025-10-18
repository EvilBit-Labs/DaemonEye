# DaemonEye

Security-focused, high-performance process monitoring — implemented in Rust.

Badges: CI, coverage, and maintainability are configured in the repo. See the badges at the top of this file when viewed on GitHub.

## Overview

DaemonEye is a Rust workspace with multiple components for collecting process information, orchestrating detections, and interacting via a CLI. The repository contains:

- procmond: Privileged process monitoring daemon (binary + library)
- daemoneye-agent: User-space orchestrator (binary)
- daemoneye-cli: Command-line interface (binary)
- daemoneye-lib: Shared library (no binary)
- collector-core: Extensible collection framework (library)

Security and reliability are emphasized via strict linting, no unsafe code, and comprehensive tests and benches.

## Technology stack

- Language: Rust 2024 Edition (MSRV 1.85)
- Package manager/build: Cargo (workspace)
- Async runtime: tokio
- IPC: interprocess (cross-platform) with the daemoneye-eventbus embedded broker
- CLI: clap v4
- Database: redb (embedded)
- Logging/telemetry: tracing, tracing-subscriber
- Config: figment (TOML files + env vars)
- System info: sysinfo
- Benchmarks: criterion
- Testing: cargo-nextest, proptest, insta

## Project structure

```
DaemonEye/
├─ procmond/         # Process monitoring daemon (bin: procmond)
├─ daemoneye-agent/  # Orchestrator (bin: daemoneye-agent)
├─ daemoneye-cli/    # Command-line interface (bin: daemoneye-cli)
├─ daemoneye-lib/    # Shared library code (no bin)
├─ collector-core/   # Collection framework (library)
├─ justfile          # Developer tasks and scripts
├─ docs/             # Documentation sources (mdBook)
├─ spec/             # Specifications and design notes
└─ Cargo.toml        # Workspace manifest
```

## Requirements

- Rust toolchain 1.85+ (edition = 2024)
- Cargo (bundled with Rust)
- just task runner (optional but recommended)
- Optional developer tools: cargo-nextest, cargo-llvm-cov, cargo-audit, cargo-deny, cargo-dist, cargo-release, mdbook (install via `just install-tools` and `just docs-install`)

## Setup

- Install Rust: https://rustup.rs
- (Optional) Install just: https://github.com/casey/just
- Initialize developer tools:
  - Windows: `just setup && just install-tools`
  - Unix: `just setup && just install-tools`

If you don’t use just, you can run the equivalent cargo commands shown below.

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
- Distribution helpers exist via cargo-dist and GoReleaser (see justfile: `dist`, `goreleaser-*`).

## Configuration

Configuration is provided by daemoneye-lib using Figment with this precedence (highest to lowest):

1. Command-line flags (component binaries)
2. Environment variables with component prefix
3. User config file: ~/.config/daemoneye/config.toml
4. System config file: /etc/daemoneye/config.toml
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

- DAEMONEYE_AGENT_TEST_MODE=1 — enables test mode paths in the agent

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

- PROCMOND\_\* / DAEMONEYE_AGENT\_\* / DAEMONEYE_CLI\_\* — general config via Figment (use `__` for nested keys)
- PROCMOND_COLLECTOR_MAX_EVENT_SOURCES, PROCMOND_COLLECTOR_EVENT_BUFFER_SIZE, PROCMOND_COLLECTOR_ENABLE_TELEMETRY, PROCMOND_COLLECTOR_DEBUG_LOGGING — collector-core overrides
- DAEMONEYE_AGENT_TEST_MODE — agent test mode

Additional OS/environment variables may be referenced in tests for compatibility checks (e.g., HOSTNAME, USER, OS), but they are not required for normal operation.

## Platform notes

- Unix: IPC is provided via Unix domain sockets using interprocess + the embedded daemoneye-eventbus broker.
- Windows: IPC uses named pipes through interprocess with daemoneye-eventbus for broker functionality.

## Documentation

- Docs sources live under ./docs (mdBook). Building locally: `just docs-install` then `mdbook build docs`.
- Some deep links in older README versions may not exist yet. TODO: Audit and update docs links once the book structure stabilizes.

## License

Apache License 2.0. See LICENSE for full text.

## Roadmap and status

This README reflects the current repository configuration (workspace crates, tasks, and configs). Some features described in earlier drafts (e.g., certain alerting sinks or centralized management) may be in-progress.

- TODO: Document concrete command-line flags for each binary (once stabilized)
- TODO: Add end-to-end quickstart with example config and sample output
- TODO: Publish prebuilt binaries via cargo-dist/GoReleaser and link here
