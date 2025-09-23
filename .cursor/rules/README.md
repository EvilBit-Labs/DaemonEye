# Cursor Rules Organization

This directory contains Cursor rules organized by category to improve maintainability and discoverability.

## Directory Structure

### `github-actions/`

GitHub Actions workflow best practices and patterns:

- `github-actions-deployment.mdc` - Deployment strategies and environment management
- `github-actions-performance.mdc` - Performance optimization and caching
- `github-actions-security.mdc` - Security best practices and secret management
- `github-actions-structure.mdc` - Workflow organization and structure
- `github-actions-testing.mdc` - Testing strategy and test pipeline patterns
- `github-actions-troubleshooting.mdc` - Common issues and debugging techniques

### `rust/`

Rust-specific coding standards and practices:

- `cargo-toml.mdc` - Cargo.toml configuration and dependency management
- `error-handling.mdc` - Error handling patterns and best practices
- `rust-standards.mdc` - General Rust coding standards and conventions
- `ipc-communication.mdc` - Interprocess communication patterns and standards
- `collector-core.mdc` - Collector-core framework implementation guidelines
- `performance-optimization.mdc` - Performance optimization and benchmarking
- `error-handling-patterns.mdc` - Comprehensive error handling patterns
- `configuration-management.mdc` - Configuration loading and validation
- `async-patterns.mdc` - Async/await patterns and best practices
- `database-operations.mdc` - Database operations with redb

### `project/`

Project-level organization and documentation:

- `documentation-standards.mdc` - Documentation requirements and standards
- `justfile-standards.mdc` - Justfile task runner conventions
- `workspace-structure.mdc` - Workspace organization and structure

### `testing/`

Testing strategies and quality assurance:

- `testing-standards.mdc` - Testing requirements and patterns

### `security/`

Security-focused rules and enterprise features:

- `enterprise-features.mdc` - Enterprise tier features and requirements
- `security-standards.mdc` - Security best practices and threat model

### `deployment/`

Deployment, performance, and operational concerns:

- `cli-design.mdc` - CLI design patterns and user experience
- `database-standards.mdc` - Database design and storage patterns
- `performance-standards.mdc` - Performance requirements and optimization

## Rule Metadata

Each rule file uses frontmatter metadata to control when it applies:

- `alwaysApply: true` - Applied to every request
- `globs: *.rs,*.toml` - Applied to specific file patterns
- `description: "..."` - Manual application by description

## Usage

Rules are automatically applied based on their metadata. The AI assistant will use these rules to provide context-aware guidance when working with different parts of the codebase.

## Adding New Rules

1. Create a new `.mdc` file in the appropriate subdirectory
2. Add frontmatter metadata to control application scope
3. Use Markdown format with Cursor-specific extensions
4. Reference files using `[filename.ext](mdc:filename.ext)` format
5. Follow the existing naming conventions and structure
