# Contributing to DaemonEye

Thank you for your interest in contributing to DaemonEye! This guide will help you get started with contributing to the project.

---

## Table of Contents

[TOC]

---

## Getting Started

### Prerequisites

Before contributing to DaemonEye, ensure you have:

- **Rust 1.85+**: Latest stable Rust toolchain
- **Git**: Version control system
- **Docker**: For containerized testing (optional)
- **Just**: Task runner (install with `cargo install just`)
- **Editor**: VS Code with Rust extension (recommended)

### Fork and Clone

1. Fork the repository on GitHub

2. Clone your fork locally:

   ```bash
   git clone https://github.com/your-username/daemoneye.git
   cd daemoneye
   ```

3. Add the upstream repository:

   ```bash
   git remote add upstream https://github.com/EvilBit-Labs/daemoneye.git
   ```

### Development Setup

1. Install dependencies:

   ```bash
   just setup
   ```

2. Run tests to ensure everything works:

   ```bash
   just test
   ```

3. Build the project:

   ```bash
   just build
   ```

## Development Environment

### Project Structure

```text
DaemonEye/
├── procmond/           # Process monitoring daemon
├── daemoneye-agent/      # Agent orchestrator
├── sentinelcli/        # CLI interface
├── sentinel-lib/       # Shared library
├── docs/               # Documentation
├── tests/              # Integration tests
├── examples/           # Example configurations
├── justfile            # Task runner
├── Cargo.toml          # Workspace configuration
└── README.md           # Project README
```

### Workspace Configuration

DaemonEye uses a Cargo workspace with the following structure:

```toml
[workspace]
resolver = "2"
members = [
  "procmond",
  "daemoneye-agent",
  "sentinelcli",
  "sentinel-lib",
]

[workspace.dependencies]
tokio = { version = "1.0", features = ["full"] }
clap = { version = "4.0", features = ["derive", "completion"] }
serde = { version = "1.0", features = ["derive"] }
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "sqlite"] }
sysinfo = "0.30"
tracing = "0.1"
thiserror = "1.0"
anyhow = "1.0"
```

### Development Commands

Use the `just` task runner for common development tasks:

```bash
# Setup development environment
just setup

# Build the project
just build

# Run tests
just test

# Run linting
just lint

# Format code
just fmt

# Run benchmarks
just bench

# Generate documentation
just docs

# Clean build artifacts
just clean
```

## Code Standards

### Rust Standards

DaemonEye follows strict Rust coding standards:

1. **Edition**: Always use Rust 2024 Edition
2. **Linting**: Zero warnings policy with `cargo clippy -- -D warnings`
3. **Safety**: `unsafe_code = "forbid"` enforced at workspace level
4. **Formatting**: Standard `rustfmt` with 119 character line length
5. **Error Handling**: Use `thiserror` for structured errors, `anyhow` for error context

### Code Style

````rust
// Use thiserror for library errors
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Permission denied accessing process {pid}")]
    PermissionDenied { pid: u32 },

    #[error("Process {pid} no longer exists")]
    ProcessNotFound { pid: u32 },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

// Use anyhow for application error context
use anyhow::{Context, Result};

pub async fn collect_processes() -> Result<Vec<ProcessInfo>> {
    let processes = sysinfo::System::new_all()
        .processes()
        .values()
        .map(|p| ProcessInfo::from(p))
        .collect::<Vec<_>>();

    Ok(processes)
        .context("Failed to collect process information")
}

// Document all public APIs
/// Collects information about all running processes.
///
/// # Returns
///
/// A vector of `ProcessInfo` structs containing details about each process.
///
/// # Errors
///
/// This function will return an error if:
/// - System information cannot be accessed
/// - Process enumeration fails
/// - Memory allocation fails
///
/// # Examples
///
/// ```rust
/// use sentinel_lib::collector::ProcessCollector;
///
/// let collector = ProcessCollector::new();
/// let processes = collector.collect_processes().await?;
/// println!("Found {} processes", processes.len());
/// ```
pub async fn collect_processes() -> Result<Vec<ProcessInfo>, CollectionError> {
    // Implementation
}
````

### Naming Conventions

- **Functions**: `snake_case`
- **Variables**: `snake_case`
- **Types**: `PascalCase`
- **Constants**: `SCREAMING_SNAKE_CASE`
- **Modules**: `snake_case`
- **Files**: `snake_case.rs`

### Documentation Standards

All public APIs must be documented with rustdoc comments:

````rust
/// A process information structure containing details about a running process.
///
/// This structure provides comprehensive information about a process including
/// its PID, name, executable path, command line arguments, and resource usage.
///
/// # Examples
///
/// ```rust
/// use sentinel_lib::ProcessInfo;
///
/// let process = ProcessInfo {
///     pid: 1234,
///     name: "example".to_string(),
///     executable_path: Some("/usr/bin/example".to_string()),
///     command_line: Some("example --arg value".to_string()),
///     start_time: Some(Utc::now()),
///     cpu_usage: Some(0.5),
///     memory_usage: Some(1024),
///     status: ProcessStatus::Running,
///     executable_hash: Some("abc123".to_string()),
///     collection_time: Utc::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessInfo {
    /// The process ID (PID) of the process
    pub pid: u32,

    /// The name of the process
    pub name: String,

    /// The full path to the process executable
    pub executable_path: Option<String>,

    /// The command line arguments used to start the process
    pub command_line: Option<String>,

    /// The time when the process was started
    pub start_time: Option<DateTime<Utc>>,

    /// The current CPU usage percentage
    pub cpu_usage: Option<f64>,

    /// The current memory usage in bytes
    pub memory_usage: Option<u64>,

    /// The current status of the process
    pub status: ProcessStatus,

    /// The SHA-256 hash of the process executable
    pub executable_hash: Option<String>,

    /// The time when this information was collected
    pub collection_time: DateTime<Utc>,
}
````

## Testing Requirements

### Test Coverage

All code must have comprehensive test coverage:

- **Unit Tests**: Test individual functions and methods
- **Integration Tests**: Test component interactions
- **End-to-End Tests**: Test complete workflows
- **Property Tests**: Test with random inputs
- **Fuzz Tests**: Test with malformed inputs

### Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_process_collection() {
        let collector = ProcessCollector::new();
        let processes = collector.collect_processes().await.unwrap();

        assert!(!processes.is_empty());
        assert!(processes.iter().any(|p| p.pid > 0));
    }

    #[test]
    fn test_process_info_serialization() {
        let process = ProcessInfo::default();
        let serialized = serde_json::to_string(&process).unwrap();
        let deserialized: ProcessInfo = serde_json::from_str(&serialized).unwrap();
        assert_eq!(process, deserialized);
    }
}
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_process_collection

# Run tests with output
cargo test -- --nocapture

# Run integration tests
cargo test --test integration

# Run benchmarks
cargo bench

# Run fuzz tests
cargo fuzz run process_info
```

## Pull Request Process

### Before Submitting

1. **Sync with upstream**:

   ```bash
   git fetch upstream
   git checkout main
   git merge upstream/main
   ```

2. **Create a feature branch**:

   ```bash
   git checkout -b feature/your-feature-name
   ```

3. **Make your changes**:

   - Write code following the coding standards
   - Add comprehensive tests
   - Update documentation
   - Run all tests and linting

4. **Commit your changes**:

   ```bash
   git add .
   git commit -m "feat: add new feature description"
   ```

### Commit Message Format

Use conventional commits format:

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

**Types**:

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes
- `refactor`: Code refactoring
- `test`: Test changes
- `chore`: Build process or auxiliary tool changes

**Examples**:

```text
feat(collector): add process filtering by name

Add ability to filter processes by name pattern using regex.
This improves performance when monitoring specific processes.

Closes #123
```

```text
fix(database): resolve memory leak in query execution

Fix memory leak that occurred when executing large queries.
The issue was caused by not properly cleaning up prepared statements.

Fixes #456
```

### Pull Request Guidelines

1. **Title**: Clear, descriptive title
2. **Description**: Detailed description of changes
3. **Tests**: Ensure all tests pass
4. **Documentation**: Update relevant documentation
5. **Breaking Changes**: Clearly mark any breaking changes
6. **Related Issues**: Link to related issues

### Pull Request Template

```markdown
## Description

Brief description of the changes made.

## Type of Change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Performance improvement
- [ ] Code refactoring

## Testing

- [ ] Unit tests pass
- [ ] Integration tests pass
- [ ] End-to-end tests pass
- [ ] Manual testing completed
- [ ] Performance tests pass

## Checklist

- [ ] Code follows the project's style guidelines
- [ ] Self-review of code completed
- [ ] Code is properly commented
- [ ] Documentation updated
- [ ] No new warnings introduced
- [ ] Breaking changes documented

## Related Issues

Closes #123
Fixes #456
```

## Issue Reporting

### Bug Reports

When reporting bugs, please include:

1. **Environment**: OS, Rust version, DaemonEye version
2. **Steps to Reproduce**: Clear, numbered steps
3. **Expected Behavior**: What should happen
4. **Actual Behavior**: What actually happens
5. **Logs**: Relevant log output
6. **Screenshots**: If applicable

### Feature Requests

When requesting features, please include:

1. **Use Case**: Why is this feature needed?
2. **Proposed Solution**: How should it work?
3. **Alternatives**: Other solutions considered
4. **Additional Context**: Any other relevant information

### Issue Templates

Use the provided issue templates:

- Bug Report: `.github/ISSUE_TEMPLATE/bug_report.md`
- Feature Request: `.github/ISSUE_TEMPLATE/feature_request.md`
- Question: `.github/ISSUE_TEMPLATE/question.md`

## Documentation

### Documentation Standards

- Keep documentation up to date
- Use clear, concise language
- Include code examples
- Follow markdown best practices
- Use consistent formatting

### Documentation Structure

```text
docs/
├── src/
│   ├── introduction.md
│   ├── getting-started.md
│   ├── architecture/
│   ├── technical/
│   ├── user-guides/
│   ├── api-reference/
│   ├── deployment/
│   ├── security.md
│   ├── testing.md
│   └── contributing.md
├── book.toml
└── README.md
```

### Building Documentation

```bash
# Install mdbook
cargo install mdbook

# Build documentation
mdbook build

# Serve documentation locally
mdbook serve
```

## Community Guidelines

### Code of Conduct

We are committed to providing a welcoming and inclusive environment for all contributors. Please:

- Be respectful and constructive
- Focus on what is best for the community
- Show empathy towards other community members
- Accept constructive criticism gracefully
- Help others learn and grow

### Communication

- **GitHub Issues**: For bug reports and feature requests
- **GitHub Discussions**: For questions and general discussion
- **Pull Requests**: For code contributions
- **Discord**: For real-time chat (invite link in README)

### Recognition

Contributors are recognized in:

- CONTRIBUTORS.md file
- Release notes
- Project documentation
- Community highlights

## Development Workflow

### Branch Strategy

- `main`: Production-ready code
- `develop`: Integration branch for features
- `feature/*`: Feature development branches
- `bugfix/*`: Bug fix branches
- `hotfix/*`: Critical bug fixes

### Release Process

1. **Version Bumping**: Update version numbers
2. **Changelog**: Update CHANGELOG.md
3. **Documentation**: Update documentation
4. **Testing**: Run full test suite
5. **Release**: Create GitHub release
6. **Distribution**: Publish to package managers

### Continuous Integration

All pull requests must pass:

- Unit tests
- Integration tests
- Linting checks
- Security scans
- Performance benchmarks
- Documentation builds

## Getting Help

If you need help contributing:

1. **Check Documentation**: Review this guide and other docs
2. **Search Issues**: Look for similar issues or discussions
3. **Ask Questions**: Use GitHub Discussions or Discord
4. **Contact Maintainers**: Reach out to project maintainers

## License

By contributing to DaemonEye, you agree that your contributions will be licensed under the same license as the project (Apache 2.0 for core features, commercial license for business/enterprise features).

---

*Thank you for contributing to DaemonEye! Your contributions help make the project better for everyone.*
