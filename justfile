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
    @just mdformat-install
    Write-Host "Note: You may need to restart your shell for pipx PATH changes to take effect"

[unix]
setup:
    cd "{{ root }}"
    rustup component add rustfmt clippy llvm-tools-preview
    cargo install cargo-binstall --locked
    @just mdformat-install
    echo "Note: You may need to restart your shell for pipx PATH changes to take effect"

# Install development tools (extended setup)
[windows]
install-tools:
    cargo binstall --disable-telemetry cargo-llvm-cov cargo-audit cargo-deny cargo-dist cargo-release cargo-cyclonedx cargo-auditable cargo-nextest --locked

[unix]
install-tools:
    cargo binstall --disable-telemetry cargo-llvm-cov cargo-audit cargo-deny cargo-dist cargo-release cargo-cyclonedx cargo-auditable cargo-nextest --locked

# Install mdBook and plugins for documentation
[windows]
docs-install:
    cargo binstall mdbook mdbook-admonish mdbook-mermaid mdbook-linkcheck mdbook-toc mdbook-open-on-gh mdbook-tabs mdbook-i18n-helpers

[unix]
docs-install:
    cargo binstall mdbook mdbook-admonish mdbook-mermaid mdbook-linkcheck mdbook-toc mdbook-open-on-gh mdbook-tabs mdbook-i18n-helpers

# Install pipx for Python tool management
[windows]
pipx-install:
    python -m pip install --user pipx
    python -m pipx ensurepath

[unix]
pipx-install:
    python3 -m pip install --user pipx
    python3 -m pipx ensurepath

# Install mdformat and extensions for markdown formatting
[windows]
mdformat-install:
    @just pipx-install
    pipx install mdformat
    pipx inject mdformat mdformat-gfm mdformat-frontmatter mdformat-footnote mdformat-simple-breaks mdformat-gfm-alerts mdformat-toc mdformat-wikilink mdformat-tables

[unix]
mdformat-install:
    @just pipx-install
    pipx install mdformat
    pipx inject mdformat mdformat-gfm mdformat-frontmatter mdformat-footnote mdformat-simple-breaks mdformat-gfm-alerts mdformat-toc mdformat-wikilink mdformat-tables

# =============================================================================
# FORMATTING AND LINTING
# =============================================================================

format: fmt format-docs

[windows]
format-docs:
    @if (Get-Command mdformat -ErrorAction SilentlyContinue) { Get-ChildItem -Recurse -Filter "*.md" | Where-Object { $_.FullName -notmatch "\\target\\" -and $_.FullName -notmatch "\\node_modules\\" } | ForEach-Object { mdformat $_.FullName } } else { Write-Host "mdformat not found. Run 'just mdformat-install' first." }

[unix]
format-docs:
    cd "{{ root }}"
    @if command -v mdformat >/dev/null 2>&1; then find . -type f -name "*.md" -not -path "./target/*" -not -path "./node_modules/*" -exec mdformat {} + ; else echo "mdformat not found. Run 'just mdformat-install' first."; fi

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
    @cargo build --workspace --release

test:
    @cargo nextest run --workspace --no-capture

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
    cargo nextest run --workspace --no-capture

# Run comprehensive tests (includes performance and security)
test-comprehensive:
    cargo nextest run --workspace --no-capture --package collector-core

# Run comprehensive tests including ignored/slow tests
test-comprehensive-full:
    cargo nextest run --workspace --no-capture --package collector-core -- --ignored

# Run all tests including ignored/slow tests across workspace
test-all:
    cargo nextest run --workspace --no-capture -- --ignored

# Run only fast unit tests
test-fast:
    cargo nextest run --workspace --no-capture --lib --bins

# Run performance-critical tests
test-performance:
    cargo nextest run --package collector-core --no-capture --test performance_critical_test

# Run security-critical tests
test-security:
    cargo nextest run --package collector-core --no-capture --test security_critical_test

# =============================================================================
# BENCHMARKING
# =============================================================================

# Run all benchmarks
bench:
    @cargo bench --workspace

# Run specific benchmark suites
bench-process:
    @cargo bench -p daemoneye-lib --bench process_collection

bench-database:
    @cargo bench -p daemoneye-lib --bench database_operations

bench-detection:
    @cargo bench -p daemoneye-lib --bench detection_engine

bench-ipc:
    @cargo bench -p daemoneye-lib --bench ipc_communication

bench-ipc-comprehensive:
    @cargo bench -p daemoneye-lib --bench ipc_performance_comprehensive

bench-ipc-validation:
    @cargo bench -p daemoneye-lib --bench ipc_client_validation_benchmarks

bench-alerts:
    @cargo bench -p daemoneye-lib --bench alert_processing

bench-crypto:
    @cargo bench -p daemoneye-lib --bench cryptographic_operations

# Run benchmarks with HTML output (Criterion generates HTML by default)
bench-html:
    @cargo bench -p daemoneye-lib

# Run benchmarks and save results to benchmark.json
bench-save:
    @cargo bench -p daemoneye-lib -- --save-baseline baseline

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
    cargo llvm-cov --workspace --lcov --output-path lcov.info

# Check coverage thresholds
coverage-check:
    cargo llvm-cov --workspace --lcov --output-path lcov.info --fail-under-lines 9.7

# Full local CI parity check
ci-check: pre-commit-run fmt-check lint-rust lint-rust-min test-ci build-release audit coverage-check dist-plan

# =============================================================================
# DEVELOPMENT AND EXECUTION
# =============================================================================

run-procmond *args:
    @cargo run -p procmond -- {{ args }}

run-daemoneye-cli *args:
    @cargo run -p daemoneye-cli -- {{ args }}

run-daemoneye-agent *args:
    @cargo run -p daemoneye-agent -- {{ args }}

# =============================================================================
# DISTRIBUTION AND PACKAGING
# =============================================================================

dist:
    @dist build

dist-check:
    @dist check

dist-plan:
    @dist plan

install:
    @cargo install --path .

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
