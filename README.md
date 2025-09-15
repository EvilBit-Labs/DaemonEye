# :shield: SentinelD — High-Performance Security Process Monitoring

**SentinelD** is a security-focused, high-performance process monitoring system built for cybersecurity professionals, threat hunters, and security operations centers. This is a complete **Rust 2024 rewrite** of the proven Python prototype, delivering enterprise-grade performance with **audit-grade integrity**.

## :classical_building: Architecture Overview

**SentinelD** is a three-component security package designed for robust, secure, and auditable system monitoring:

```text
SentinelD/
├── procmond/         # :lock: Privileged Process Collector
├── sentinelcli/      # :computer: Command-Line Interface
├── sentinelagent/    # :satellite: User-Space Orchestrator
└── sentinel-lib/     # :gear: Shared Library Components
```

### Component Roles

- **:lock: ProcMonD (Collector):** Runs with elevated privileges, focused solely on process monitoring with minimal attack surface. Writes to Certificate Transparency-style audit ledger and communicates via protobuf IPC with sentinelagent.
- **:satellite: SentinelAgent (Orchestrator):** Operates in user space with minimal privileges. Manages procmond lifecycle, executes SQL detection rules, and handles alert delivery. Translates complex SQL rules into simple protobuf tasks for procmond.
- **:computer: SentinelCLI:** Local command-line interface for data queries, result exports, and service configuration. Communicates with sentinelagent for all operations.

This separation ensures **robust security**: ProcMonD remains isolated and hardened, while orchestration/network tasks are delegated to low-privilege processes.

## :dart: Key Features

| Feature                                             | Description                                                      |
| --------------------------------------------------- | ---------------------------------------------------------------- |
| :crab: **Rust Performance**                         | Memory-safe, high-performance rewrite with \<5% CPU overhead     |
| :mag: **Cross-Platform**                            | Linux, macOS, and Windows support with native OS integration     |
| :chart_with_upwards_trend: **SQL Detection Engine** | Flexible anomaly detection using standard SQL queries            |
| :file_cabinet: **Audit-Grade Integrity**            | Certificate Transparency-style Merkle tree with inclusion proofs |
| :satellite: **Multi-Channel Alerting**              | stdout, syslog, webhooks, email with delivery guarantees         |
| :zap: **High-Performance**                          | Handle 10k+ processes with bounded queues and backpressure       |
| :lock: **Security-First Design**                    | Principle of least privilege, sandboxed rule execution           |
| :globe_with_meridians: **Offline-Capable**          | No external dependencies for core functionality                  |

## :gift: Free Forever

The **Free Tier** of SentinelD is completely free forever with no time limits or feature restrictions. This includes:

- Full process monitoring and detection capabilities
- All built-in detection rules and SQL-based custom rules
- Complete alerting system (stdout, syslog, webhooks, email)
- Local data storage and querying
- Cross-platform support (Linux, macOS, Windows)
- Offline operation with no external dependencies

**Future Business and Enterprise tiers** will add centralized management, advanced integrations, and kernel-level monitoring for organizations that need these capabilities, but the core functionality will always remain free.

## :rocket: Getting Started

### Prerequisites

- Rust 1.85+ (2024 Edition support)
- Just task runner

### Quick Start

```bash
# Build all components
just build

# Run linting and tests
just lint && just test

# Start orchestrator agent (manages procmond automatically)
just run-sentinelagent

# Launch CLI interface
just run-sentinelcli --help

# Run single-shot collection (for testing)
just run-sentinelagent --once
```

### Example Usage

```bash
# Start the orchestrator (manages procmond automatically)
sentinelagent --config /etc/sentineld/config.yaml

# Query historical process data through orchestrator
sentinelcli query --sql "SELECT * FROM processes WHERE name = 'suspicious_proc'"

# Test alert delivery
sentinelcli alerts send-test

# Check system health
sentinelcli health-check --verbose

# Export data for analysis
sentinelcli export --format json --output /tmp/process_data.json
```

## :brain: Detection Capabilities

**Built-in Detection Rules:**

- Process hollowing detection (processes without executables)
- Executable integrity violations (file modifications during runtime)
- Suspicious process name duplications
- Unusual parent-child process relationships
- Anomalous resource consumption patterns

**Custom Rule Support:**

- SQL-based detection logic with sandboxed execution
- Hot-reloadable rules with metadata and versioning
- Performance monitoring and optimization hints

## :outbox_tray: Alert Integration

| Channel           | Format           | Use Case                  |
| ----------------- | ---------------- | ------------------------- |
| **stdout/stderr** | JSON, Plain Text | Development, debugging    |
| **Syslog**        | RFC5424, JSON    | SIEM integration          |
| **Webhooks**      | JSON POST        | Security orchestration    |
| **Email**         | HTML, Plain Text | Incident notifications    |
| **File Output**   | JSON, CEF        | Log aggregation, archival |

## :gear: Technology Stack

- **Language:** Rust 2024 Edition (MSRV: 1.70+)
- **Async Runtime:** Tokio for I/O and task management
- **Database:** redb pure Rust embedded database for optimal performance and security
- **CLI Framework:** clap v4 with derive macros and shell completions
- **Process Enumeration:** sysinfo crate with platform-specific optimizations
- **Logging:** tracing ecosystem with structured JSON output

## :wrench: Development

This project follows strict Rust coding standards:

- **Linting:** `cargo clippy -- -D warnings` (zero warnings policy)
- **Formatting:** `rustfmt` with consistent code style
- **Testing:** Comprehensive unit and integration test coverage
- **Safety:** `unsafe_code = "forbid"` in workspace lints
- **Performance:** \<100MB memory, \<5% CPU, \<5s for 10k+ processes

### Available Commands

```bash
# Development workflow
just fmt          # Format code
just lint         # Run clippy with strict warnings
just test         # Run all tests
just build        # Build workspace

# Component execution
just run-procmond --once --verbose      # Run process monitor with flags
just run-sentinelcli --help             # Run CLI interface
just run-sentinelagent --config /path   # Run orchestrator agent
```

## :busts_in_silhouette: Target Users

- **SOC Analysts** monitoring fleet infrastructure for process anomalies
- **Incident Responders** investigating compromised systems
- **Red Team Operators** detecting defensive monitoring
- **Security Engineers** integrating with SIEM platforms
- **System Administrators** maintaining security visibility
- **DevSecOps Teams** embedding security monitoring in deployments

## :books: Documentation

For comprehensive documentation, see:

- [Project Overview](docs/book/project-overview.html) - High-level overview and getting started
- [System Architecture](docs/book/architecture/system-architecture.html) - Three-component architecture details
- [User Guides](docs/book/user-guides.html) - Configuration and operator guides
- [API Reference](docs/book/api-reference.html) - Core API documentation
- [Deployment Guide](docs/book/deployment.html) - Installation and deployment options

## :page_facing_up: License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

---

**SentinelD** — When your process monitoring actually matters. :shield:
