# 🛡️ SentinelD — High-Performance Security Process Monitoring

**SentinelD** is a security-focused, high-performance process monitoring system built for cybersecurity professionals, threat hunters, and security operations centers. This is a complete **Rust 2024 rewrite** of the proven Python prototype, delivering enterprise-grade performance with **audit-grade integrity**.

## 🏛️ Architecture Overview

**SentinelD** is a three-component security package designed for robust, secure, and auditable system monitoring:

```text
SentinelD/
├── procmond/         # 🔒 Privileged Process Collector
├── sentinelcli/      # 💻 Command-Line Interface
├── sentinelagent/    # 📡 User-Space Orchestrator
└── sentinel-lib/     # ⚙️ Shared Library Components
```

### Component Roles

- **🔒 ProcMonD (Collector):** Runs with elevated privileges, focused solely on process monitoring with minimal attack surface. Writes to tamper-evident, append-only audit logs and emits events via secure IPC.
- **📡 SentinelAgent (Orchestrator):** Operates in user space with minimal privileges. Receives real-time events from ProcMonD for alerting and network delivery.
- **💻 SentinelCLI:** Local command-line interface for data queries, result exports, and service configuration through the orchestrator.

This separation ensures **robust security**: ProcMonD remains isolated and hardened, while orchestration/network tasks are delegated to low-privilege processes.

## 🎯 Key Features

| Feature | Description |
|---------|-------------|
| 🦀 **Rust Performance** | Memory-safe, high-performance rewrite with <5% CPU overhead |
| 🔍 **Cross-Platform** | Linux, macOS, and Windows support with native OS integration |
| 📊 **SQL Detection Engine** | Flexible anomaly detection using standard SQL queries |
| 🗄️ **Audit-Grade Integrity** | Tamper-evident, hash-chained, verifiable log storage |
| 📡 **Multi-Channel Alerting** | stdout, syslog, webhooks, email with delivery guarantees |
| ⚡ **High-Performance** | Handle 10k+ processes with bounded queues and backpressure |
| 🔐 **Security-First Design** | Principle of least privilege, sandboxed rule execution |
| 🌐 **Offline-Capable** | No external dependencies for core functionality |

## 🚀 Getting Started

### Prerequisites

- Rust 1.85+ (2024 Edition support)
- Just task runner
- SQLite 3.42+

### Quick Start

```bash
# Build all components
just build

# Run linting and tests
just lint && just test

# Start process monitoring (demo mode)
just run-procmond --once

# Launch CLI interface
just run-sentinelcli --help

# Start orchestrator agent
just run-sentinelagent
```

### Example Usage

```bash
# Start daemon with configuration
procmond run --config /etc/sentineld/config.yaml

# Run single-shot monitoring (smoke test)
procmond run --once --output /tmp/process_snapshot.json

# Query historical process data
sentinelcli query --sql "SELECT * FROM processes WHERE name = 'suspicious_proc'"

# Test alert delivery
sentinelcli alerts send-test

# Check system health
procmond self-check --verbose
```

## 🧠 Detection Capabilities

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

## 📤 Alert Integration

| Channel | Format | Use Case |
|---------|--------|----------|
| **stdout/stderr** | JSON, Plain Text | Development, debugging |
| **Syslog** | RFC5424, JSON | SIEM integration |
| **Webhooks** | JSON POST | Security orchestration |
| **Email** | HTML, Plain Text | Incident notifications |
| **File Output** | JSON, CEF | Log aggregation, archival |

## ⚙️ Technology Stack

- **Language:** Rust 2024 Edition (MSRV: 1.70+)
- **Async Runtime:** Tokio for I/O and task management
- **Database:** SQLite 3.42+ with WAL mode
- **CLI Framework:** clap v4 with derive macros and shell completions
- **Process Enumeration:** sysinfo crate with platform-specific optimizations
- **Logging:** tracing ecosystem with structured JSON output

## 🔧 Development

This project follows strict Rust coding standards:

- **Linting:** `cargo clippy -- -D warnings` (zero warnings policy)
- **Formatting:** `rustfmt` with consistent code style
- **Testing:** Comprehensive unit and integration test coverage
- **Safety:** `unsafe_code = "forbid"` in workspace lints
- **Performance:** <100MB memory, <5% CPU, <5s for 10k+ processes

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

## 👥 Target Users

- **SOC Analysts** monitoring fleet infrastructure for process anomalies
- **Incident Responders** investigating compromised systems
- **Red Team Operators** detecting defensive monitoring
- **Security Engineers** integrating with SIEM platforms
- **System Administrators** maintaining security visibility
- **DevSecOps Teams** embedding security monitoring in deployments

## 📚 Documentation

For comprehensive documentation, see:

- [Technical Architecture](../DevelopmentDocs/Product_Plans/procmond/docs/02-Architecture.md)
- [CLI Specification](../DevelopmentDocs/Product_Plans/procmond/docs/05-CLI-Spec.md)
- [Database Schema](../DevelopmentDocs/Product_Plans/procmond/docs/03-Database-Schema.md)
- [API Design](../DevelopmentDocs/Product_Plans/procmond/docs/04-API-Design.md)

## 📄 License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

---

**SentinelD** — When your process monitoring actually matters. 🛡️
