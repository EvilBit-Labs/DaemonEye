# SentinelD Distribution Package

This package serves as a **distribution bundle** that combines all SentinelD components into a single crate for easier installation and deployment.

## Bundling Strategy

### Purpose

- **Single Installation**: Users can install all components with `cargo install sentinel`
- **Simplified Distribution**: One package to manage instead of three separate packages
- **Consistent Versioning**: All components share the same version and release cycle
- **Reduced Complexity**: Users don't need to know about individual component packages

### Architecture

The sentinel package bundles three core components:

- **procmond**: Privileged process collector (minimal attack surface)
- **sentinelagent**: User-space orchestrator for detection and alerting
- **sentinelcli**: Command-line interface for operators
- **sentinel-lib**: Shared library providing common functionality

### Development Workflow

1. **Individual Development**: Each component is developed in its own package:

   - `procmond/` - Process collection daemon
   - `sentinelagent/` - Detection and alerting orchestrator
   - `sentinelcli/` - Command-line interface
   - `sentinel-lib/` - Shared library

2. **Bundling Process**: Changes are synchronized to this package:

   - Individual package implementations are copied to `src/bin/*.rs`
   - Dependencies are aggregated in `Cargo.toml`
   - Tests are added to validate bundled behavior

3. **Distribution**: This package serves as the distribution artifact:

   - Published to crates.io as the `sentinel` package
   - Users install with `cargo install sentinel`
   - All three binaries are available after installation

## Installation

```bash
# Install all SentinelD components
cargo install sentinel

# This provides three binaries:
# - procmond
# - sentinelagent
# - sentinelcli
```

## Usage

After installation, you can use any of the bundled components:

```bash
# Process monitoring daemon
procmond --help

# Detection and alerting orchestrator
sentinelagent --help

# Command-line interface
sentinelcli --help
```

## Development

### For Contributors

- **Individual Packages**: Develop features in the individual packages (`procmond/`, `sentinelagent/`, `sentinelcli/`)
- **Synchronization**: Copy changes to the corresponding `src/bin/*.rs` files in this package
- **Testing**: Add tests to `tests/` directory to validate bundled behavior
- **Dependencies**: Update `Cargo.toml` if new dependencies are needed

### For Maintainers

- **Version Management**: This package inherits version from workspace
- **Release Process**: Publish this package to crates.io for distribution
- **Dependency Updates**: Keep dependencies synchronized with individual packages

## Security Model

The bundled package maintains the same security architecture as individual components:

- **Privilege Separation**: Only procmond runs with elevated privileges
- **Network Security**: No inbound connections, outbound-only for alerts
- **Data Protection**: Comprehensive audit logging and data validation
- **Input Validation**: All inputs validated with serde and typed models

## Deployment Tiers

- **Free Tier**: Standalone agents (procmond + sentinelagent + sentinelcli)
- **Business Tier**: + Security Center + Enterprise integrations ($199/site)
- **Enterprise Tier**: + Kernel monitoring + Federated architecture + Advanced SIEM

## Testing

The bundled package includes tests to ensure:

- Binary behavior matches individual packages
- Error handling works correctly
- CLI argument parsing functions properly
- Database error handling is appropriate

Run tests with:

```bash
cargo test
```

## License

Apache-2.0 License - see [LICENSE](../../LICENSE) for details.
