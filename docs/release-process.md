# DaemonEye Release Process

This document describes how to build and release DaemonEye binaries using GoReleaser with platform-specific configurations.

## Overview

DaemonEye uses separate GoReleaser configurations for each platform to avoid cross-compilation issues:

- **macOS**: `.goreleaser-macos.yaml` - Native builds on macOS
- **Linux**: `.goreleaser-linux.yaml` - Native builds on Linux
- **Windows**: `.goreleaser-windows.yaml` - Native builds on Windows

## Quick Start

### Automated Release (GitHub Actions)

1. Create and push a tag:

   ```bash
   git tag v1.0.0
   git push origin v1.0.0
   ```

2. GitHub Actions will automatically:

   - Build binaries on each platform
   - Create GitHub releases
   - Upload artifacts

### Manual Release

#### macOS

```bash
# On macOS system
goreleaser release --config .goreleaser-macos.yaml
```

#### Linux

```bash
# On Linux system
goreleaser release --config .goreleaser-linux.yaml
```

#### Windows

```bash
# On Windows system
goreleaser release --config .goreleaser-windows.yaml
```

## Configuration Details

### Platform-Specific Builds

Each configuration targets specific architectures:

- **macOS**: `aarch64-apple-darwin`, `x86_64-apple-darwin`
- **Linux**: `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`
- **Windows**: `aarch64-pc-windows-msvc`, `x86_64-pc-windows-msvc`

### Build Tools

- **macOS**: Uses native `cargo` (no cross-compilation issues)
- **Linux**: Uses native `cargo` (no cross-compilation issues)
- **Windows**: Uses native `cargo` (no cross-compilation issues)

### Disabled Features

All configurations disable platform-specific packaging:

- `archive: disable: true`
- `brew: disable: true`
- `snapcraft: disable: true`
- `chocolatey: disable: true`

This ensures only GitHub releases are created.

## CI/CD Integration

### GitHub Actions

The `.github/workflows/release.yml` file provides:

1. **Parallel builds** on all three platforms
2. **Automatic triggering** on tag pushes
3. **Rust toolchain setup** for each platform
4. **GoReleaser installation** and execution

### Other CI Systems

For other CI systems, use the same pattern:

```bash
# Install GoReleaser
curl -sL https://git.io/goreleaser | bash

# Run platform-specific build
goreleaser release --config .goreleaser-<platform>.yaml
```

## Troubleshooting

### Common Issues

1. **Cross-compilation failures**: Use native builders for each platform
2. **Missing system headers**: Ensure proper toolchain setup
3. **Framework linking issues**: Use native cargo instead of cargo-zigbuild

### Debug Commands

```bash
# Test build locally
goreleaser build --config .goreleaser-macos.yaml --snapshot

# Check configuration
goreleaser check --config .goreleaser-macos.yaml

# Verbose output
goreleaser release --config .goreleaser-macos.yaml --verbose
```

## Release Artifacts

Each platform build creates:

- **procmond**: Process monitoring daemon
- **daemoneye-agent**: Detection orchestrator
- **daemoneye-cli**: Command-line interface

All binaries are uploaded to GitHub releases with platform-specific naming.

## Security Considerations

- All builds use `--release` flag for optimized binaries
- Dependencies are locked with `Cargo.lock`
- No cross-compilation reduces attack surface
- Native builds ensure proper system integration
