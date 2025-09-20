# SentinelD Development Guidelines

## AI Assistant Behavior Rules

### Core Principles

- **No Auto-Commits**: Never commit code without explicit permission. Always present diffs for approval.
- **Security-First**: This is a security-critical system. All changes must maintain the principle of least privilege and undergo thorough security review.
- **Zero-Warnings Policy**: All code must pass `cargo clippy -- -D warnings` with no exceptions.
- **Operator-Centric Design**: Built for operators, by operators. Prioritize workflows efficient in contested/airgapped environments.
- **Testing Required**: All code changes must include appropriate tests to ensure quality and correctness.
- **Linter Restrictions**: Never remove clippy restrictions or allow linters marked as `deny` without explicit permission. All `-D warnings` and `deny` attributes must be preserved.

### Rule Precedence Hierarchy

1. **Project Rules** (.cursor/rules/, AGENTS.md) - Highest precedence
2. **Steering Documents** (.kiro/steering/, specs/) - Architectural authority
3. **Technical Specifications** (.kiro/specs/) - Implementation requirements
4. **Embedded defaults** - Lowest precedence

## Development Workflow

### Task Runner (justfile)

All development tasks use the `just` command runner with DRY principles:

```bash
# Formatting
just fmt          # Format all code
just fmt-check    # Check formatting (CI-friendly)

# Linting (composed recipe)
just lint         # Runs fmt-check + clippy + lint-just
just lint-rust    # Clippy with strict warnings
just lint-just    # Lint justfile syntax

# Building and Testing
just build        # Build all binaries with features
just check        # Quick check without build
just test         # Run all tests

# Component Execution
just run-procmond [args]      # Run procmond with optional args
just run-sentinelcli [args]   # Run sentinelcli with optional args
just run-sentinelagent [args] # Run sentinelagent with optional args
```

### Justfile Conventions

- Compose complex tasks by calling `@just <subrecipe>`
- Keep paths relative to project directory
- Use consistent argument patterns with defaults
- Include `lint-just` recipe to validate justfile syntax with `just --fmt --check --unstable`
- No hardcoded paths outside project directory for portability

### Git Workflow

- Use conventional commits format
- Create feature branches for new work
- Ensure all tests pass before merging
- No commits without explicit permission

## Quality Assurance

### Pre-commit Requirements

1. `cargo clippy -- -D warnings` (zero warnings)
2. `cargo fmt --all --check` (formatting validation)
3. `cargo test --workspace` (all tests pass)
4. `just lint-just` (justfile syntax validation)
5. No new `unsafe` code without explicit approval
6. Performance benchmarks within acceptable ranges

### Test Execution

```bash
# All tests must use stable output environment
NO_COLOR=1 TERM=dumb cargo test --workspace

# Component-specific testing
RUST_BACKTRACE=1 cargo test -p sentinel-lib --nocapture

# Performance regression testing
cargo bench --baseline previous

# Coverage reporting
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info
```

## Documentation Standards

### Architectural Documentation

- **Diagrams**: All architectural and flow diagrams must use Mermaid
- **Markdown**: Prettier is configured to ignore Markdown files (.prettierignore), use mdformat
- **API Documentation**: Comprehensive rustdoc comments for all public interfaces
- **Cross-references**: Use relative links and maintain link hygiene
- **Examples**: Include code examples in rustdoc with `cargo test --doc`

### Code Documentation

All public APIs must include comprehensive rustdoc comments with:

- Purpose and functionality description
- Security considerations
- Performance characteristics
- Usage examples (with `no_run` attribute when appropriate)
- Error conditions and return types

## CI/CD and Reviews

### Continuous Integration

- **Platform**: GitHub Actions with matrix testing (Linux, macOS, Windows)
- **Rust Versions**: stable, beta, MSRV (1.70+)
- **Quality Checks**: fmt-check, clippy strict, comprehensive test suite
- **Performance**: Benchmark regression detection with criterion
- **Security**: Dependency scanning, SLSA provenance (Enterprise)

### Code Review Process

- **Primary Review Tool**: coderabbit.ai (preferred over GitHub Copilot)
- **Review Requirements**: Security focus, performance impact assessment
- **Single Maintainer**: UncleSp1d3r operates as sole maintainer with appropriate push restrictions

### Commit and Release Management

#### Commit Standards

- **Format**: Conventional Commits specification
- **Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`
- **Scopes**: `(auth)`, `(api)`, `(cli)`, `(models)`, `(detection)`, `(alerting)`, etc.
- **Breaking Changes**: Indicated with `!` in header or `BREAKING CHANGE:` in footer

#### Release Process

- **Versioning**: Semantic Versioning (SemVer)
- **Milestones**: Named as version numbers (e.g., `v1.0`) with descriptive context
- **Automation**: `cargo release` for automated version management
- **Distribution**: Platform-specific packages with code signing (Business/Enterprise)

#### Git Workflow

- **No Auto-Commits**: Never commit automatically without explicit permission
- **Branch Strategy**: Feature branches with PR review process
- **Protection**: Single maintainer with push restrictions and branch protection
