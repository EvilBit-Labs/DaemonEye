set shell := ["bash", "-eu", "-o", "pipefail", "-c", "--"]

# =============================================================================
# GENERAL COMMANDS
# =============================================================================

default:
    @just help

help:
    @just --list

# =============================================================================
# SETUP AND INITIALIZATION
# =============================================================================

# Development setup
setup:
    cd {{justfile_dir()}}
    rustup component add rustfmt clippy llvm-tools-preview rust-src
    cargo install cargo-nextest --locked || echo "cargo-nextest already installed"

# Install development tools (extended setup)
install-tools:
    cargo install cargo-llvm-cov --locked || echo "cargo-llvm-cov already installed"
    cargo install cargo-audit --locked || echo "cargo-audit already installed"
    cargo install cargo-deny --locked || echo "cargo-deny already installed"
    cargo install cargo-dist --locked || echo "cargo-dist already installed"

# Install mdBook and plugins for documentation
docs-install:
    cargo install mdbook mdbook-admonish mdbook-mermaid mdbook-linkcheck mdbook-toc mdbook-open-on-gh mdbook-tabs mdbook-i18n-helpers

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

# Run clippy with fixes
fix:
    cargo clippy --fix --allow-dirty --allow-staged

# Quick development check
check: pre-commit-run
    just lint
    just test-no-docker

pre-commit-run:
    pre-commit run -a

# Format a single file (for pre-commit hooks)
format-files +FILES:
    npx prettier --write --config .prettierrc.json {{FILES}}

megalinter:
    cd {{justfile_dir()}}
    npx mega-linter-runner --flavor rust


# =============================================================================
# BUILDING AND TESTING
# =============================================================================

build:
    @cargo build --workspace

build-release:
    @cargo build --workspace --all-features --release

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
ci-check: fmt-check lint-rust lint-rust-min test-ci build-release audit

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
