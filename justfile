set shell := ["bash", "-eu", "-o", "pipefail", "-c", "--"]

# =============================================================================
# GENERAL COMMANDS
# =============================================================================

default:
    @just help

help:
    @just --list

# =============================================================================
# FORMATTING AND LINTING
# =============================================================================

format: fmt format-docs

format-docs:
    mdformat **/*.md

fmt:
    @cargo fmt --all

fmt-check:
    @cargo fmt --all --check

lint-rust:
    @just fmt-check
    @cargo clippy --workspace --all-targets --all-features -- -D warnings

lint-rust-min:
    @cargo clippy --workspace --all-targets --no-default-features -- -D warnings

lint-just:
    @just --fmt --check --unstable

lint:
    @just lint-rust
    @just lint-just

# =============================================================================
# BUILDING AND TESTING
# =============================================================================

build:
    @cargo build --workspace

build-release:
    @cargo build --workspace --all-features --release

check:
    @cargo check --workspace

test:
    @cargo test --workspace

test-ci:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-nextest >/dev/null 2>&1; then
        echo "nextest detected: running tests with cargo nextest"
        cargo nextest run --workspace --all-features
    else
        echo "cargo-nextest not found: falling back to cargo test"
        cargo test --workspace --all-features --all-targets
    fi

# =============================================================================
# SECURITY AND AUDITING
# =============================================================================

audit:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-audit >/dev/null 2>&1; then
        cargo audit
    else
        echo "cargo-audit not found; skipping security advisory audit. Install with: cargo install cargo-audit"
    fi

deny:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-deny >/dev/null 2>&1; then
        cargo deny check
    else
        echo "cargo-deny not found; skipping license/advisory checks. Install with: cargo install cargo-deny"
    fi

# =============================================================================
# CI AND QUALITY ASSURANCE
# =============================================================================

# Full local CI parity check
ci-check:
    #!/usr/bin/env bash
    set -euo pipefail

    # CI-like environment
    export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
    export CI=true

    echo "==> 1/7: Justfile format check"
    just lint-just

    echo "==> 2/7: Format check (rustfmt)"
    just fmt-check

    echo "==> 3/7: Lint (clippy, all features)"
    just lint-rust

    echo "==> 4/7: Lint (clippy, minimal features)"
    just lint-rust-min

    echo "==> 5/7: Tests (cargo nextest if available, else cargo test)"
    just test-ci

    echo "==> 6/7: Build release"
    just build-release

    echo "==> 7/7: Security audits (cargo-audit)"
    just audit

    echo "==> CI check completed successfully! ✅"

# =============================================================================
# DEVELOPMENT AND EXECUTION
# =============================================================================

run-procmond *args:
    @cargo run -p procmond -- {{ args }}

run-sentinelcli *args:
    @cargo run -p sentinelcli -- {{ args }}

run-sentinelagent *args:
    @cargo run -p sentinelagent -- {{ args }}

# =============================================================================
# DISTRIBUTION AND PACKAGING
# =============================================================================

dist:
    @cargo dist build

dist-check:
    @cargo dist check

dist-plan:
    @cargo dist plan

install:
    @cargo install --path sentinel

# =============================================================================
# RELEASE MANAGEMENT
# =============================================================================

release:
    @cargo release

release-dry-run:
    @cargo release --dry-run

release-patch:
    @cargo release patch

release-minor:
    @cargo release minor

release-major:
    @cargo release major
