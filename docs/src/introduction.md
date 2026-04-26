# DaemonEye Documentation

Welcome to the DaemonEye documentation. DaemonEye is a high-performance, security-focused process monitoring system built in Rust. The product is pre-release; this documentation covers architecture, design, technical specifications, security, testing, and contribution guidelines.

## What is DaemonEye?

DaemonEye is an agent-centric system monitoring tool designed for cybersecurity professionals, threat hunters, and security operations centers. It provides process monitoring, SQL-based threat detection, and alert delivery, with a focus on silent observation and audit-grade integrity.

## Key Features

- **Real-time Process Monitoring**: Continuous monitoring of system processes with minimal performance impact
- **SQL-Based Detection**: Detection rules expressed in a SQL-like DSL with AST validation
- **Cross-Platform Support**: Linux and Windows primary; macOS and FreeBSD secondary
- **Audit-Grade Integrity**: BLAKE3 hash-chained audit ledger; Ed25519-signed events
- **Air-Gap Friendly**: Fully functional offline; no automatic egress
- **Security-Focused**: Built with security best practices and minimal attack surface

DaemonEye is distributed as open-core. This repository contains the Community tier — the agent-side foundation. Commercial tiers (fleet management, GUI, federation, kernel-level collectors) extend this foundation and are sold separately through evilbitlabs.io; they are not in this repo.

## Three-Component Security Architecture

DaemonEye follows a three-component security architecture:

1. **procmond (Collector)**: Privileged process monitoring daemon built on the collector-core framework with minimal attack surface
2. **daemoneye-agent (Orchestrator)**: User-space orchestrator with:
   - Embedded EventBus broker for multi-collector coordination via topic-based pub/sub messaging
   - RPC service for collector lifecycle management (start/stop/restart/health checks)
   - IPC server for CLI communication using protobuf over Unix sockets/named pipes
   - Alert management with multi-channel delivery
3. **daemoneye-cli**: Command-line interface for queries and system management

This separation ensures robust security by isolating privileged operations from network functionality while enabling scalable multi-collector architectures with RPC-based lifecycle management.

## Documentation Structure

This documentation is organized into several sections:

- **[Getting Started](./getting-started.md)**: Prerequisites, privilege model, and architectural orientation
- **[Project Overview](./project-overview.md)**: Detailed project information and value propositions
- **[Architecture](./architecture.md)**: System architecture and design principles
- **[Technical Documentation](./technical.md)**: Technical specifications and implementation details
- **[Security](./security.md)**: Security model and design overview
- **[Testing](./testing.md)**: Testing strategies and guidelines
- **[Contributing](./contributing.md)**: Contribution guidelines and development setup

> **Note:** Installation, deployment, operator, CLI, and API reference documentation will be published with the v1.0.0 release. For the latest project status, see the repository README.

## Getting Help

If you need help with DaemonEye:

1. Check the [Getting Started](./getting-started.md) guide
2. Review the [Architecture](./architecture.md) and [Technical Documentation](./technical.md)
3. Consult the [Security](./security.md) section for the security model
4. Join the community discussions on GitHub
5. Contact support for commercial assistance

## License

The DaemonEye components in this repository — procmond, daemoneye-agent, daemoneye-cli, daemoneye-lib, collector-core, daemoneye-eventbus — are licensed under Apache 2.0. Commercial extensions ship separately; see evilbitlabs.io for details.

---

*This documentation tracks the development state of the DaemonEye Community tier and is updated as the codebase evolves.*
