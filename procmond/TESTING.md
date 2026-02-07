# procmond Test Suite Documentation

This document describes the comprehensive test suite for procmond, including test strategy, coverage targets, execution instructions, and CI/CD integration.

---

## Table of Contents

1. [Test Strategy](#test-strategy)
2. [Coverage Targets](#coverage-targets)
3. [Test Categories](#test-categories)
4. [Running Tests](#running-tests)
5. [CI/CD Integration](#cicd-integration)
6. [Troubleshooting](#troubleshooting)

---

## Test Strategy

### Testing Pyramid

procmond follows a layered testing approach:

```text
                 /\
                /  \
               / E2E \          <- Optional full system tests
              /--------\
             /Integration\       <- Cross-component verification
            /--------------\
           /   Unit Tests   \    <- Core component coverage (>80%)
          /------------------\
         /  Performance Tests \  <- Criterion benchmarks
        /----------------------\
       /   Security & Chaos     \<- Resilience verification
      /--------------------------\
```

### Testing Principles

1. **Security-First**: Security tests verify defenses against privilege escalation, injection attacks, and DoS vectors.
2. **Cross-Platform**: Tests run on Linux, macOS, and Windows to ensure consistent behavior.
3. **Deterministic**: Tests use isolated temp directories and mock actors for reproducibility.
4. **Performance-Aware**: Benchmarks establish baselines and verify performance budgets.
5. **Chaos Engineering**: Resilience tests verify behavior under adverse conditions.

### Test Tools

| Tool          | Purpose                    | Configuration          |
| ------------- | -------------------------- | ---------------------- |
| cargo-nextest | Parallel test runner       | `.config/nextest.toml` |
| insta         | Snapshot testing           | `tests/snapshots/`     |
| criterion     | Benchmarking               | `benches/`             |
| llvm-cov      | Coverage measurement       | Cargo workspace        |
| proptest      | Property-based testing     | Dev dependency         |
| tempfile      | Isolated test environments | Dev dependency         |

---

## Coverage Targets

### Unit Test Coverage

| Component            | Target | Description                               |
| -------------------- | ------ | ----------------------------------------- |
| WriteAheadLog (WAL)  | >80%   | Crash recovery, persistence, rotation     |
| EventBusConnector    | >80%   | Event publishing, buffering, backpressure |
| RpcServiceHandler    | >80%   | RPC operations, health checks, config     |
| RegistrationManager  | >80%   | Agent registration, heartbeat, recovery   |
| Actor Pattern        | >80%   | State machine, message passing            |
| ConfigurationManager | >80%   | Config parsing, validation, updates       |

### Critical Path Coverage

| Path                        | Target | Tests                               |
| --------------------------- | ------ | ----------------------------------- |
| Process collection pipeline | >90%   | Collection, transformation, publish |
| WAL write/replay cycle      | >90%   | Persistence, recovery, ordering     |
| RPC health check flow       | >90%   | Request handling, actor interaction |
| Graceful shutdown           | >90%   | State cleanup, resource release     |

### Measuring Coverage

Generate coverage report:

```bash
# Full coverage report with HTML output
cargo llvm-cov --all-features --workspace --html

# LCOV format for CI integration
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

# Coverage with nextest runner (recommended)
cargo llvm-cov nextest --workspace --profile coverage

# Check coverage threshold (CI) - currently 9.7%, target is 80%
cargo llvm-cov nextest --workspace --profile coverage --fail-under-lines 9.7
```

View coverage report:

```bash
# Open HTML report in browser
open target/llvm-cov/html/index.html
```

---

## Test Categories

### Unit Tests (src/ modules)

Unit tests are embedded in source files using `#[cfg(test)]` modules.

**Location**: `procmond/src/*.rs`

**Scope**:

- Individual function behavior
- Error handling paths
- Edge cases and boundary conditions

**Example components tested**:

- `wal.rs` - WAL entry creation, checksum, rotation
- `event_bus_connector.rs` - Buffer management, backpressure signals
- `rpc_service.rs` - Request parsing, response formatting
- `registration.rs` - Registration state machine
- `monitor_collector.rs` - Actor message handling

### Integration Tests

Integration tests verify cross-component behavior.

**Location**: `procmond/tests/`

| Test File                         | Scope                                       |
| --------------------------------- | ------------------------------------------- |
| `event_bus_integration_tests.rs`  | Event publishing, WAL integration, ordering |
| `rpc_integration_tests.rs`        | RPC lifecycle operations, config updates    |
| `cross_platform_tests.rs`         | Platform-specific collectors, core fields   |
| `lifecycle_tracking_tests.rs`     | Process start/stop/modify detection         |
| `actor_mode_integration_tests.rs` | Actor pattern coordination                  |

### Chaos Tests

Chaos tests verify resilience under adverse conditions.

**Location**: `procmond/tests/chaos_tests.rs`

| Category            | Tests                                             |
| ------------------- | ------------------------------------------------- |
| Connection Failures | Broker unavailability, reconnection, event replay |
| Backpressure        | Buffer fill, adaptive intervals, WAL persistence  |
| Resource Limits     | Memory budget, WAL rotation, operation timing     |
| Concurrent Ops      | Multiple RPC requests, config updates, shutdown   |

### Security Tests

Security tests verify defenses against attack vectors.

**Location**: `procmond/tests/security_tests.rs`

| Category             | Tests                                          |
| -------------------- | ---------------------------------------------- |
| Privilege Escalation | Unauthorized operations, state transitions     |
| Injection Attacks    | Malicious process names, command lines, paths  |
| DoS Attacks          | RPC flooding, event flooding, channel overflow |
| Data Sanitization    | Secret patterns, sensitive command args        |

### Performance Benchmarks

Criterion benchmarks establish performance baselines.

**Location**: `procmond/benches/`

| Benchmark File                    | Measurements                              |
| --------------------------------- | ----------------------------------------- |
| `performance_benchmarks.rs`       | WAL write/replay, serialization, combined |
| `process_collector_benchmarks.rs` | Process enumeration performance           |

**Performance Budgets**:

- Process enumeration: < 5s for 10,000+ processes
- DB writes: > 1,000 records/sec
- Alert latency: < 100ms per rule
- CPU usage: < 5% sustained
- Memory: < 100 MB resident

---

## Running Tests

### Quick Reference

```bash
# Run all tests
just test

# Run tests with output
cargo nextest run --package procmond --no-capture

# Run specific test file
cargo nextest run --package procmond --test chaos_tests

# Run specific test
cargo nextest run --package procmond -- test_backpressure_buffer_fill
```

### Running by Category

#### Unit Tests

```bash
# All unit tests (lib target)
cargo nextest run --package procmond --lib

# Unit tests with output
cargo nextest run --package procmond --lib --no-capture
```

#### Integration Tests

```bash
# All integration tests
cargo nextest run --package procmond --test '*'

# Specific integration test suite
cargo nextest run --package procmond --test event_bus_integration_tests
cargo nextest run --package procmond --test rpc_integration_tests
cargo nextest run --package procmond --test cross_platform_tests
cargo nextest run --package procmond --test lifecycle_tracking_tests
```

#### Chaos Tests

```bash
# All chaos tests
cargo nextest run --package procmond --test chaos_tests

# Specific chaos category (by test name pattern)
cargo nextest run --package procmond -- test_connection_failure
cargo nextest run --package procmond -- test_backpressure
cargo nextest run --package procmond -- test_resource_limits
cargo nextest run --package procmond -- test_concurrent
```

#### Security Tests

```bash
# All security tests
cargo nextest run --package procmond --test security_tests

# Specific security category
cargo nextest run --package procmond -- test_privilege
cargo nextest run --package procmond -- test_injection
cargo nextest run --package procmond -- test_dos
cargo nextest run --package procmond -- test_sanitization
```

#### Performance Benchmarks

```bash
# All benchmarks
cargo bench --package procmond

# Specific benchmark suite
cargo bench --package procmond --bench performance_benchmarks

# WAL benchmarks only
cargo bench --package procmond --bench performance_benchmarks -- wal_

# Save baseline for comparison
cargo bench --package procmond -- --save-baseline main

# Compare against baseline
cargo bench --package procmond -- --baseline main
```

### Test Profiles

The project defines nextest profiles for different scenarios:

```bash
# Default profile (fail-fast, quick feedback)
cargo nextest run --package procmond

# CI profile (no fail-fast, retries, full results)
cargo nextest run --package procmond --profile ci

# Coverage profile (single-threaded, deterministic)
cargo llvm-cov nextest --package procmond --profile coverage
```

### Environment Variables

Control test behavior with environment variables:

```bash
# Disable colored output (CI-friendly)
NO_COLOR=1 TERM=dumb cargo nextest run --package procmond

# Enable debug logging
RUST_LOG=debug cargo nextest run --package procmond

# Show backtraces on failure
RUST_BACKTRACE=1 cargo nextest run --package procmond
```

---

## CI/CD Integration

### GitHub Actions Workflow

Tests run automatically on:

- Push to `main` branch
- Pull requests to `main` branch
- Manual workflow dispatch

**Workflow file**: `.github/workflows/ci.yml`

### CI Jobs

| Job                   | Description                             |
| --------------------- | --------------------------------------- |
| `quality`             | Format check, clippy linting            |
| `test`                | Run all tests with nextest (Linux)      |
| `test-cross-platform` | Platform matrix (Linux, macOS, Windows) |
| `coverage`            | Generate and upload coverage reports    |

### Platform Matrix

| Platform | Runner       | Status  |
| -------- | ------------ | ------- |
| Linux    | ubuntu-22.04 | Primary |
| macOS    | macos-15     | Primary |
| Windows  | windows-2022 | Primary |

### Coverage Reporting

Coverage reports are uploaded to:

- **Codecov**: For coverage visualization and PR comments
- **Qlty**: For quality metrics tracking

### CI Commands

```bash
# Replicate CI locally
just ci-check

# Run CI test profile
just test-ci

# Generate coverage (CI format)
just coverage
```

### Test Artifacts

CI generates the following artifacts:

| Artifact          | Location                      | Purpose                 |
| ----------------- | ----------------------------- | ----------------------- |
| JUnit XML reports | `target/nextest/ci/junit.xml` | Test result integration |
| Coverage LCOV     | `lcov.info`                   | Coverage uploads        |
| Coverage HTML     | `target/llvm-cov/html/`       | Local coverage review   |
| Benchmark results | `target/criterion/`           | Performance tracking    |

---

## Troubleshooting

### Common Issues

#### Tests Hang or Timeout

```bash
# Increase timeout
cargo nextest run --package procmond --no-capture --slow-timeout 5m

# Run single-threaded
cargo nextest run --package procmond -j 1
```

#### Flaky Tests

```bash
# Enable retries
cargo nextest run --package procmond --retries 3

# Run specific test in isolation
cargo nextest run --package procmond -- test_name -j 1
```

#### Platform-Specific Failures

```bash
# Skip platform-specific tests
cargo nextest run --package procmond --test cross_platform_tests -- --skip linux
cargo nextest run --package procmond --test cross_platform_tests -- --skip macos
cargo nextest run --package procmond --test cross_platform_tests -- --skip windows
```

#### Coverage Not Generating

```bash
# Ensure llvm-tools is installed
rustup component add llvm-tools

# Install cargo-llvm-cov
cargo install cargo-llvm-cov

# Clean and regenerate
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace
```

### Debugging Tests

```bash
# Run with verbose output
cargo nextest run --package procmond --no-capture

# Run with debug logging
RUST_LOG=debug cargo nextest run --package procmond

# Run with backtraces
RUST_BACKTRACE=full cargo nextest run --package procmond
```

### Performance Issues

```bash
# Check system resources during collection benchmarks
cargo bench --package procmond --bench process_collector_benchmarks -- --verbose

# Profile with flamegraph (requires cargo-flamegraph)
cargo flamegraph --bench performance_benchmarks
```

---

## Appendix: Test File Summary

```text
procmond/
├── src/
│   ├── wal.rs                     # WAL unit tests (mod tests)
│   ├── event_bus_connector.rs     # EventBus unit tests
│   ├── rpc_service.rs             # RPC unit tests
│   ├── registration.rs            # Registration unit tests
│   └── monitor_collector.rs       # Actor unit tests
├── tests/
│   ├── chaos_tests.rs             # Chaos/resilience tests
│   ├── security_tests.rs          # Security tests
│   ├── event_bus_integration_tests.rs
│   ├── rpc_integration_tests.rs
│   ├── cross_platform_tests.rs
│   ├── lifecycle_tracking_tests.rs
│   ├── actor_mode_integration_tests.rs
│   ├── integration_tests.rs
│   ├── cli.rs                     # CLI snapshot tests
│   └── snapshots/                 # Insta snapshots
└── benches/
    ├── performance_benchmarks.rs  # Criterion benchmarks
    └── process_collector_benchmarks.rs
```

---

**Last Updated**: 2026-02-03 **Maintainer**: DaemonEye Team
