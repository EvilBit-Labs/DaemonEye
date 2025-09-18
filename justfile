# Cross-platform justfile using OS annotations
# Windows uses PowerShell, Unix uses bash

set shell := ["bash", "-c"]
set windows-shell := ["powershell", "-NoProfile", "-Command"]

root := justfile_dir()

# =============================================================================
# GENERAL COMMANDS
# =============================================================================

default:
    @just help

help:
    @just --list

# =============================================================================
# CROSS-PLATFORM HELPERS
# =============================================================================
# Cross-platform helpers using OS annotations
# Each helper has Windows and Unix variants

[windows]
cd-root:
    Set-Location "{{ root }}"

[unix]
cd-root:
    cd "{{ root }}"

[windows]
ensure-dir dir:
    New-Item -ItemType Directory -Force -Path "{{ dir }}" | Out-Null

[unix]
ensure-dir dir:
    /bin/mkdir -p "{{ dir }}"

[windows]
rmrf path:
    if (Test-Path "{{ path }}") { Remove-Item "{{ path }}" -Recurse -Force }

[unix]
rmrf path:
    /bin/rm -rf "{{ path }}"

# =============================================================================
# SETUP AND INITIALIZATION
# =============================================================================

# Development setup
[windows]
setup:
    Set-Location "{{ root }}"
    rustup component add rustfmt clippy llvm-tools-preview
    cargo install cargo-binstall --locked

[unix]
setup:
    cd "{{ root }}"
    rustup component add rustfmt clippy llvm-tools-preview
    cargo install cargo-binstall --locked

# Install development tools (extended setup)
[windows]
install-tools:
    cargo binstall cargo-llvm-cov cargo-audit cargo-deny cargo-dist --locked

[unix]
install-tools:
    cargo binstall cargo-llvm-cov cargo-audit cargo-deny cargo-dist --locked

# Install mdBook and plugins for documentation
[windows]
docs-install:
    cargo binstall mdbook mdbook-admonish mdbook-mermaid mdbook-linkcheck mdbook-toc mdbook-open-on-gh mdbook-tabs mdbook-i18n-helpers

[unix]
docs-install:
    cargo binstall mdbook mdbook-admonish mdbook-mermaid mdbook-linkcheck mdbook-toc mdbook-open-on-gh mdbook-tabs mdbook-i18n-helpers

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

lint-rust: fmt-check
    @cargo clippy --workspace --all-targets --all-features -- -D warnings

lint-rust-min:
    @cargo clippy --workspace --all-targets --no-default-features -- -D warnings

# Format justfile
fmt-justfile:
    @just --fmt --unstable

# Lint justfile formatting
lint-justfile:
    @just --fmt --check --unstable

lint: lint-rust lint-justfile

# Run clippy with fixes
fix:
    cargo clippy --fix --allow-dirty --allow-staged

# Quick development check
check: pre-commit-run lint

pre-commit-run:
    pre-commit run -a

# Format a single file (for pre-commit hooks)
format-files +FILES:
    npx prettier --write --config .prettierrc.json {{ FILES }}

megalinter:
    cd "{{ root }}"
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

# Test justfile cross-platform functionality
[windows]
test-justfile:
    Set-Location "{{ root }}"
    $p = (Get-Location).Path; Write-Host "Current directory: $p"; Write-Host "Expected directory: {{ root }}"

[unix]
test-justfile:
    cd "{{ root }}"
    /bin/echo "Current directory: $(pwd -P)"
    /bin/echo "Expected directory: {{ root }}"

# Test cross-platform file system helpers
[windows]
test-fs:
    Set-Location "{{ root }}"
    @just rmrf tmp/xfstest
    @just ensure-dir tmp/xfstest/sub
    @just rmrf tmp/xfstest

[unix]
test-fs:
    cd "{{ root }}"
    @just rmrf tmp/xfstest
    @just ensure-dir tmp/xfstest/sub
    @just rmrf tmp/xfstest

test-ci:
    cargo nextest run --workspace --all-features

# =============================================================================
# SECURITY AND AUDITING
# =============================================================================

audit:
    cargo audit

deny:
    cargo deny check

# =============================================================================
# CI AND QUALITY ASSURANCE
# =============================================================================

# Generate coverage report
coverage:
    cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

# Check coverage thresholds
coverage-check:
    cargo llvm-cov --all-features --package procmond --package sentinelagent --package sentinelcli --package sentinel-lib --lcov --output-path lcov.info --fail-under-lines 85

# Full local CI parity check
ci-check: pre-commit-run fmt-check lint-rust lint-rust-min test-ci build-release audit coverage-check dist-plan

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
