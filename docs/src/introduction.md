# DaemonEye Documentation

Welcome to the DaemonEye documentation! This comprehensive guide covers everything you need to know about DaemonEye, a high-performance, security-focused process monitoring system built in Rust.

## What is DaemonEye?

DaemonEye is a complete rewrite of the Python prototype, designed for cybersecurity professionals, threat hunters, and security operations centers. It provides real-time process monitoring, threat detection, and alerting capabilities across multiple platforms.

## Key Features

- **Real-time Process Monitoring**: Continuous monitoring of system processes with minimal performance impact
- **Threat Detection**: SQL-based detection rules with hot-reloading capabilities
- **Multi-tier Architecture**: Core, Business, and Enterprise tiers with different feature sets
- **Cross-platform Support**: Linux, macOS, and Windows support
- **Container Ready**: Docker and Kubernetes deployment options
- **Security Focused**: Built with security best practices and minimal attack surface

## Three-Component Security Architecture

DaemonEye follows a robust three-component security architecture:

1. **ProcMonD (Collector)**: Privileged process monitoring daemon with minimal attack surface
2. **SentinelAgent (Orchestrator)**: User-space process for alerting and network operations
3. **SentinelCLI**: Command-line interface for queries and configuration

This separation ensures robust security by isolating privileged operations from network functionality.

## Documentation Structure

This documentation is organized into several sections:

- **[Getting Started](./getting-started.md)**: Quick start guide for new users
- **[Project Overview](./project-overview.md)**: Detailed project information and features
- **[Architecture](./architecture.md)**: System architecture and design principles
- **[Technical Documentation](./technical.md)**: Technical specifications and implementation details
- **[User Guides](./user-guides.md)**: Comprehensive user and operator guides
- **[API Reference](./api-reference.md)**: Complete API documentation
- **[Deployment](./deployment.md)**: Installation and deployment guides
- **[Security](./security.md)**: Security considerations and best practices
- **[Testing](./testing.md)**: Testing strategies and guidelines
- **[Contributing](./contributing.md)**: Contribution guidelines and development setup

## Quick Links

- [Installation Guide](./deployment/installation.md)
- [Configuration Guide](./user-guides/configuration.md)
- [Operator Guide](./user-guides/operator-guide.md)
- [API Reference](./api-reference/core-api.md)
- [Docker Deployment](./deployment/docker.md)
- [Kubernetes Deployment](./deployment/kubernetes.md)

## Getting Help

If you need help with DaemonEye:

1. Check the [Getting Started](./getting-started.md) guide
2. Review the [Troubleshooting](./user-guides/operator-guide.md#troubleshooting) section
3. Consult the [API Reference](./api-reference/core-api.md) for technical details
4. Join our community discussions on GitHub
5. Contact support for commercial assistance

## License

DaemonEye follows a dual-license strategy:

- **Core Components**: Apache 2.0 licensed (procmond, daemoneye-agent, sentinelcli, sentinel-lib)
- **Business Tier Features**: $199/site one-time license (Security Center, GUI, enhanced connectors, curated rules)
- **Enterprise Tier Features**: Custom pricing (kernel monitoring, federation, STIX/TAXII integration)

---

*This documentation is continuously updated. For the latest information, always refer to the most recent version.*
