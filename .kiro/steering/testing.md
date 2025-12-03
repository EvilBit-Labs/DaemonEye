# DaemonEye Testing Strategy

## Three-Tier Testing Architecture

### Unit Testing

- **Scope**: Individual components and algorithms only
- **Tools**: Standard Rust test framework with tokio-test for async utilities
- **Database**: redb with temporary files for isolated testing
- **Mocking**: Trait-based service mocking for external dependencies

### Integration Testing

- **Scope**: Cross-component interaction and realistic scenarios
- **Tools**: insta for snapshot testing and predicates for validation
- **Approach**: Primary testing method with minimal mocking
- **Database**: Real redb instances with test data

### End-to-End Testing (Optional)

- **Scope**: Complete user workflows and system integration
- **Flow**: procmond collection → database population → agent rule execution → CLI querying
- **Environment**: Full system deployment with test data seeding

## Test Framework and Tools

### Test Runner

- **Primary**: cargo-nextest for faster, more reliable test execution
- **Features**: Parallel execution, better output, failure isolation
- **CI Integration**: Structured JSON output for automated analysis

### Quality Tools

- **Coverage**: llvm-cov for coverage measurement and reporting (target: >85%)
- **Property Testing**: proptest for generative testing of edge cases and invariants
- **Fuzz Testing**: Extensive fuzzing for security-critical components (SQL parser, config validation)
- **Snapshot Testing**: insta for deterministic CLI output validation

## Test Execution Environment

### Stable Output Requirements

```bash
# All tests must use stable output environment
NO_COLOR=1 TERM=dumb cargo test --workspace

# Component-specific testing
RUST_BACKTRACE=1 cargo test -p daemoneye-lib --nocapture

# Performance regression testing
cargo bench --baseline previous

# Coverage reporting
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info
```

### Cross-Platform Testing

- **CI Matrix**: Linux, macOS, Windows with multiple Rust versions (stable, beta, MSRV)
- **Architecture**: x86_64 and ARM64 support validation
- **Containers**: Docker and Kubernetes deployment testing

## Quality Gates

### Pre-commit Requirements

1. `cargo clippy -- -D warnings` (zero warnings)
2. `cargo fmt --all --check` (formatting validation)
3. `cargo test --workspace` (all tests pass)
4. `just lint-just` (justfile syntax validation)
5. No new `unsafe` code without explicit approval
6. Performance benchmarks within acceptable ranges

### Performance Testing

- **Critical Path Benchmarks**: Use criterion for database operations, detection rules, process enumeration
- **Regression Detection**: Automated performance comparison against baselines
- **Load Testing**: Validate performance targets with 10k+ process datasets

## Test Organization

### Test Structure

```rust,ignore
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[tokio::test]
    async fn test_process_collection() {
        // Example test implementation
        let result = "test output";
        assert_eq!(result, "test output");
    }
}
```

### Integration Test Organization

1. **Unit Tests**: Test individual components with mocked dependencies
2. **Integration Tests**: Use testcontainers for database operations
3. **End-to-End Tests**: Full system testing with sample data

## Contribution Checklists for Common Tasks

### Adding a New Detection Rule

**Files to Touch:**

- `daemoneye-lib/src/detection.rs` - Rule implementation
- `daemoneye-lib/src/models.rs` - Rule data structure
- `daemoneye-agent/src/rules/` - Rule registration
- `tests/integration/` - Rule validation tests

**Checklist:**

1. [ ] Define rule structure with SQL query and metadata
2. [ ] Implement rule validation with AST parsing (sqlparser)
3. [ ] Add comprehensive unit tests with mock data
4. [ ] Create integration test via daemoneye-agent
5. [ ] Validate alert delivery through daemoneye-cli
6. [ ] Run `cargo clippy -- -D warnings` (zero warnings)
7. [ ] Performance test with criterion if rule is complex
8. [ ] Document rule purpose and expected matches in rustdoc

### Adding a New CLI Option

**Files to Touch:**

- `daemoneye-cli/src/main.rs` - CLI argument definition
- `daemoneye-lib/src/config.rs` - Configuration handling
- `tests/integration/` - CLI behavior tests

**Checklist:**

1. [ ] Update clap derive structures with new argument
2. [ ] Implement configuration handling in daemoneye-lib
3. [ ] Add help text and default values
4. [ ] Create insta snapshot tests for CLI output
5. [ ] Test both short and long option forms
6. [ ] Validate with `NO_COLOR=1 TERM=dumb` environment
7. [ ] Update shell completion generation if applicable
8. [ ] Document option in help text and rustdoc

### Optimizing Database Write Path

**Files to Touch:**

- `daemoneye-lib/src/storage.rs` - Database operations
- `procmond/src/collector.rs` - Collection logic
- `benches/` - Performance benchmarks

**Checklist:**

1. [ ] Establish baseline with criterion benchmarks
2. [ ] Implement optimization with redb best practices
3. [ ] Add batch processing if applicable
4. [ ] Validate performance targets (>1000 records/sec)
5. [ ] Test with 10k+ process datasets
6. [ ] Run memory profiling to check for leaks
7. [ ] Ensure ACID transaction guarantees maintained
8. [ ] Document performance characteristics
