# Cross-platform justfile using OS annotations
# Windows uses PowerShell, Unix uses bash

set shell := ["bash", "-cu"]
set windows-shell := ["powershell", "-NoProfile", "-Command"]
set dotenv-load := true
set ignore-comments := true

# Use mise to manage all dev tools (go, pre-commit, uv, etc.)
# See mise.toml for tool versions

mise_exec := "mise exec --"
root := justfile_dir()

# =============================================================================
# GENERAL COMMANDS
# =============================================================================

default:
    @just --list

# =============================================================================
# CROSS-PLATFORM HELPERS (private)
# =============================================================================

[private]
[windows]
ensure-dir dir:
    New-Item -ItemType Directory -Force -Path "{{ dir }}" | Out-Null

[private]
[unix]
ensure-dir dir:
    /bin/mkdir -p "{{ dir }}"

[private]
[windows]
rmrf path:
    if (Test-Path "{{ path }}") { Remove-Item "{{ path }}" -Recurse -Force }

[private]
[unix]
rmrf path:
    /bin/rm -rf "{{ path }}"

# =============================================================================
# SETUP AND INITIALIZATION
# =============================================================================

# Development setup - mise handles all tool installation via mise.toml
setup:
    mise install

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
    @{{ mise_exec }} cargo fmt --all

fmt-check:
    @{{ mise_exec }} cargo fmt --all --check

lint-rust: fmt-check
    @{{ mise_exec }} cargo clippy --workspace --all-targets --all-features -- -D warnings

lint-rust-min:
    @{{ mise_exec }} cargo clippy --workspace --all-targets --no-default-features -- -D warnings

# Check documentation compiles without warnings
[windows]
lint-docs:
    $env:RUSTDOCFLAGS='-D warnings'; @{{ mise_exec }} cargo doc --no-deps --document-private-items

[unix]
lint-docs:
    RUSTDOCFLAGS='-D warnings' {{ mise_exec }} cargo doc --no-deps --document-private-items

# Format justfile
fmt-justfile:
    @{{ mise_exec }} just --fmt --unstable

# Lint justfile formatting
lint-justfile:
    @{{ mise_exec }} just --fmt --check --unstable

lint: lint-rust lint-docs lint-justfile

# Run clippy with fixes
fix:
    @{{ mise_exec }} cargo clippy --fix --allow-dirty --allow-staged

# Quick development check
check: pre-commit-run lint

pre-commit-run:
    @{{ mise_exec }} pre-commit run -a

# Format a single file (for pre-commit hooks)
format-files +FILES:
    @{{ mise_exec }} prettier --write --config .prettierrc.json {{ FILES }}

# =============================================================================
# BUILDING AND TESTING
# =============================================================================

build:
    @{{ mise_exec }} cargo build --workspace

build-release:
    @{{ mise_exec }} cargo build --workspace --release

test:
    @{{ mise_exec }} cargo nextest run --workspace --no-capture

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
    @{{ mise_exec }} cargo nextest run --workspace --profile ci --no-capture

# Run comprehensive tests (includes performance and security)
test-comprehensive:
    @{{ mise_exec }} cargo nextest run --workspace --no-capture --package collector-core

# Run comprehensive tests including ignored/slow tests
test-comprehensive-full:
    @{{ mise_exec }} cargo nextest run --workspace --no-capture --package collector-core -- --ignored

# Run all tests including ignored/slow tests across workspace
test-all:
    @{{ mise_exec }} cargo nextest run --workspace --no-capture -- --ignored

# Run only fast unit tests
test-fast:
    @{{ mise_exec }} cargo nextest run --workspace --no-capture --lib --bins

# Run performance-critical tests
test-performance:
    @{{ mise_exec }} cargo nextest run --package collector-core --no-capture --test performance_critical_test

# Run security-critical tests
test-security:
    @{{ mise_exec }} cargo nextest run --package collector-core --no-capture --test security_critical_test

# =============================================================================
# BENCHMARKING
# =============================================================================

# Run all benchmarks
bench:
    @{{ mise_exec }} cargo bench --workspace

# Run specific benchmark suites
bench-process:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench process_collection

bench-database:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench database_operations

bench-detection:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench detection_engine

bench-ipc:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench ipc_communication

bench-ipc-comprehensive:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench ipc_performance_comprehensive

bench-ipc-validation:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench ipc_client_validation_benchmarks

bench-alerts:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench alert_processing

bench-crypto:
    @{{ mise_exec }} cargo bench -p daemoneye-lib --bench cryptographic_operations

# Run benchmarks with HTML output (Criterion generates HTML by default)
bench-html:
    @{{ mise_exec }} cargo bench -p daemoneye-lib

# Run benchmarks and save results to benchmark.json
bench-save:
    @{{ mise_exec }} cargo bench -p daemoneye-lib -- --save-baseline baseline

# =============================================================================
# SECURITY AND AUDITING
# =============================================================================

# Supply-chain security checks
audit-deps:
    @{{ mise_exec }} cargo audit

deny-deps:
    @{{ mise_exec }} cargo deny check

# Composed security scan
security-scan: audit-deps deny-deps

# Legacy aliases (backward compatibility)
audit: audit-deps

deny: deny-deps

# =============================================================================
# CI AND QUALITY ASSURANCE
# =============================================================================

# Generate coverage report using nextest with coverage profile
coverage:
    @{{ mise_exec }} cargo llvm-cov nextest --workspace --profile coverage --lcov --output-path lcov.info

# Alias for coverage generation
test-coverage: coverage

# Check coverage thresholds

# TODO: Raise threshold to 80% once test coverage reaches target (see TESTING.md)
coverage-check:
    @{{ mise_exec }} cargo llvm-cov nextest --workspace --profile coverage --lcov --output-path lcov.info --fail-under-lines 9.7

# Full local CI parity check
ci-check: pre-commit-run fmt-check lint-rust lint-rust-min test-ci build-release security-scan coverage-check dist-plan

# =============================================================================
# DEVELOPMENT AND EXECUTION
# =============================================================================

run-procmond *args:
    @{{ mise_exec }} cargo run -p procmond -- {{ args }}

run-daemoneye-cli *args:
    @{{ mise_exec }} cargo run -p daemoneye-cli -- {{ args }}

run-daemoneye-agent *args:
    @{{ mise_exec }} cargo run -p daemoneye-agent -- {{ args }}

# =============================================================================
# DISTRIBUTION AND PACKAGING
# =============================================================================

dist:
    @{{ mise_exec }} dist build

dist-check:
    @{{ mise_exec }} dist check

dist-plan:
    @{{ mise_exec }} dist plan

install:
    @{{ mise_exec }} cargo install --path .

# =============================================================================
# GORELEASER TESTING
# =============================================================================

# Test GoReleaser configuration
goreleaser-check:
    @{{ mise_exec }} goreleaser check

# Build binaries locally with GoReleaser (test build process)
[windows]
goreleaser-build:
    @{{ mise_exec }} goreleaser build --clean

[unix]
goreleaser-build:
    #!/bin/bash
    set -euo pipefail
    # Compute and export SDK-related env for macOS; no-ops on non-mac Unix
    if command -v xcrun >/dev/null 2>&1; then
        SDKROOT_PATH=$(xcrun --sdk macosx --show-sdk-path)
        export SDKROOT="${SDKROOT_PATH}"
        export MACOSX_DEPLOYMENT_TARGET="11.0"
        # Help cargo-zigbuild/zig locate Apple SDK frameworks
        export CARGO_ZIGBUILD_SYSROOT="${SDKROOT_PATH}"
        # Ensure the system linker sees the correct syslibroot and frameworks
        export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-syslibroot,${SDKROOT_PATH} -C link-arg=-F${SDKROOT_PATH}/System/Library/Frameworks"
    fi
    @{{ mise_exec }} goreleaser build --clean

# Run snapshot release (test full pipeline without publishing)
[windows]
goreleaser-snapshot:
    @{{ mise_exec }} goreleaser release --snapshot --clean

[unix]
goreleaser-snapshot:
    #!/bin/bash
    set -euo pipefail
    # Compute and export SDK-related env for macOS; no-ops on non-mac Unix
    if command -v xcrun >/dev/null 2>&1; then
        SDKROOT_PATH=$(xcrun --sdk macosx --show-sdk-path)
        export SDKROOT="${SDKROOT_PATH}"
        export MACOSX_DEPLOYMENT_TARGET="11.0"
        # Help cargo-zigbuild/zig locate Apple SDK frameworks
        export CARGO_ZIGBUILD_SYSROOT="${SDKROOT_PATH}"
        # Ensure the system linker sees the correct syslibroot and frameworks
        export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-syslibroot,${SDKROOT_PATH} -C link-arg=-F${SDKROOT_PATH}/System/Library/Frameworks"
    fi
    @{{ mise_exec }} goreleaser release --snapshot --clean

# Test GoReleaser with specific target
[windows]
goreleaser-build-target target:
    @{{ mise_exec }}    goreleaser build --clean --single-target {{ target }}

[unix]
goreleaser-build-target target:
    #!/bin/bash
    set -euo pipefail
    # Compute and export SDK-related env for macOS; no-ops on non-mac Unix
    if command -v xcrun >/dev/null 2>&1; then
        SDKROOT_PATH=$(xcrun --sdk macosx --show-sdk-path)
        export SDKROOT="${SDKROOT_PATH}"
        export MACOSX_DEPLOYMENT_TARGET="11.0"
        # Help cargo-zigbuild/zig locate Apple SDK frameworks
        export CARGO_ZIGBUILD_SYSROOT="${SDKROOT_PATH}"
        # Ensure the system linker sees the correct syslibroot and frameworks
        export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-syslibroot,${SDKROOT_PATH} -C link-arg=-F${SDKROOT_PATH}/System/Library/Frameworks"
    fi
    @{{ mise_exec }} goreleaser build --clean --single-target {{ target }}

# Clean GoReleaser artifacts
goreleaser-clean:
    @just rmrf dist

# =============================================================================
# PLATFORM-SPECIFIC RELEASE TESTING
# =============================================================================

# Test macOS release configuration
[windows]
goreleaser-test-macos:
    @echo "⚠️  Skipping macOS test (not on macOS)"

[unix]
goreleaser-test-macos:
    #!/bin/bash
    set -euo pipefail
    if [[ "$OSTYPE" == "darwin"* ]]; then
        echo "🍎 Testing macOS configuration..."
        {{ mise_exec }} goreleaser build --config .goreleaser-macos.yaml --snapshot --clean
        echo "✅ macOS build successful"
    else
        echo "⚠️  Skipping macOS test (not on macOS)"
    fi

# Test Linux release configuration
[windows]
goreleaser-test-linux:
    @echo "⚠️  Skipping Linux test (not on Linux)"

[unix]
goreleaser-test-linux:
    #!/bin/bash
    set -euo pipefail
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        echo "🐧 Testing Linux configuration..."
        {{ mise_exec }} goreleaser build --config .goreleaser-linux.yaml --snapshot --clean
        echo "✅ Linux build successful"
    else
        echo "⚠️  Skipping Linux test (not on Linux)"
    fi

# Test Windows release configuration
[windows]
goreleaser-test-windows:
    @echo "🪟 Testing Windows configuration..."
    @{{ mise_exec }} goreleaser build --config .goreleaser-windows.yaml --snapshot --clean
    @echo "✅ Windows build successful"

[unix]
goreleaser-test-windows:
    @echo "⚠️  Skipping Windows test (not on Windows)"

# Test all platform configurations (skips incompatible platforms)
goreleaser-test-all: goreleaser-test-macos goreleaser-test-linux goreleaser-test-windows
    @echo "🎉 All platform tests completed!"

# Test specific platform configuration
goreleaser-test-platform platform:
    @{{ mise_exec }} goreleaser build --config .goreleaser-{{ platform }}.yaml --snapshot --clean
    @echo "✅ {{ platform }} build successful"

# =============================================================================
# RELEASE MANAGEMENT
# =============================================================================

release:
    @{{ mise_exec }} cargo release

release-dry-run:
    @{{ mise_exec }} cargo release --dry-run

release-patch:
    @{{ mise_exec }} cargo release patch

release-minor:
    @{{ mise_exec }} cargo release minor

release-major:
    @{{ mise_exec }} cargo release major
