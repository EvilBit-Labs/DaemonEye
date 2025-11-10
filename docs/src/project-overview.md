# DaemonEye Project Overview

## Mission Statement

DaemonEye is a security-focused, high-performance process monitoring system designed to detect suspicious activity on systems through continuous process monitoring, behavioral analysis, and pattern recognition. **Its primary purpose is to detect suspicious activity on systems by monitoring abnormal process behavior and patterns.**

This is a complete Rust 2024 rewrite of a proven Python prototype, delivering enterprise-grade performance with audit-grade integrity while maintaining the security-first, offline-capable philosophy.

## Core Mission

Detect and alert on suspicious system activity through continuous process monitoring, behavioral analysis, and pattern recognition. Provide security operations teams with a reliable, high-performance threat detection solution that operates independently of external dependencies while maintaining audit-grade integrity and operator-centric workflows.

## Key Value Propositions

### **Audit-Grade Integrity**

- Certificate Transparency-style Merkle tree with inclusion proofs suitable for compliance and forensics
- BLAKE3 hashing for fast, cryptographically secure hash computation
- Optional Ed25519 signatures for enhanced integrity verification
- Append-only audit ledger with monotonic sequence numbers

### **Offline-First Operation**

- Full functionality without internet access, perfect for airgapped environments
- Local rule caching ensures detection continues during network outages
- Buffered alert delivery with persistent queue for reliability
- Bundle-based configuration and rule distribution system

### **Security-First Architecture**

- Privilege separation with minimal attack surface
- Sandboxed execution and minimal privileges
- Zero unsafe code goal with comprehensive safety verification
- SQL injection prevention with AST validation

### **High Performance**

- \<5% CPU overhead while monitoring 10,000+ processes
- Sub-second process enumeration for large systems
- > 1,000 records/second database write rate
- \<100ms alert latency per detection rule

### **Operator-Centric Design**

- Built for operators, by operators
- Workflows optimized for contested environments
- Comprehensive CLI with multiple output formats
- Color support with NO_COLOR and TERM=dumb handling

## Three-Component Architecture

DaemonEye implements a **three-component security architecture** with strict privilege separation:

### 1. **procmond** (Privileged Process Collector)

- **Purpose**: Minimal privileged component for secure process data collection
- **Security**: Runs with elevated privileges, drops them immediately after initialization
- **Network**: No network access whatsoever
- **Database**: Write-only access to audit ledger
- **Features**: Process enumeration, executable hashing, Certificate Transparency-style audit ledger

### 2. **daemoneye-agent** (Detection Orchestrator)

- **Purpose**: User-space detection rule execution and alert management
- **Security**: Minimal privileges, outbound-only network connections
- **Database**: Read/write access to event store
- **Features**: SQL-based detection engine, multi-channel alerting, procmond lifecycle management

### 3. **daemoneye-cli** (Operator Interface)

- **Purpose**: Command-line interface for queries, management, and diagnostics
- **Security**: No network access, read-only database operations
- **Features**: JSON/table output, color handling, shell completions, system health monitoring

### 4. **daemoneye-lib** (Shared Core)

- **Purpose**: Common functionality shared across all components
- **Modules**: config, models, storage, detection, alerting, crypto, telemetry
- **Security**: Trait-based abstractions with security boundaries

## Target Users

### **Primary Users**

- **SOC Analysts**: Monitoring fleet infrastructure for process anomalies
- **Security Operations & Incident Response Teams**: Investigating compromised systems
- **System Reliability Engineers**: Requiring low-overhead monitoring
- **Blue Team Security Engineers**: Integrating with existing security infrastructure
- **DevSecOps Teams**: Embedding security monitoring in deployments

### **Organizational Context**

- **Small Teams**: Core tier for basic process monitoring
- **Consultancies**: Business tier for client management and reporting
- **Enterprises**: Enterprise tier for large-scale, federated deployments
- **Government/Military**: Airgapped environments with strict compliance requirements

## Key Features

### **Threat Detection Capabilities**

#### Process Behavior Analysis

- Detect process hollowing and executable integrity violations
- Identify suspicious parent-child relationships
- Monitor process lifecycle events in real-time
- Track process memory usage and CPU consumption patterns

#### Anomaly Detection

- Identify unusual resource consumption patterns
- Detect suspicious process name duplications
- Monitor for process injection techniques
- Track unusual network activity patterns

#### SQL-Based Detection Engine

- Flexible rule creation using standard SQL queries
- Sandboxed execution with resource limits
- AST validation to prevent injection attacks
- Comprehensive library of built-in detection rules

#### Built-in Detection Rules

- Common malware tactics, techniques, and procedures (TTPs)
- MITRE ATT&CK framework coverage
- Process hollowing and injection detection
- Suspicious network activity patterns

### **System Integration**

#### Cross-Platform Support

- **Linux**: Native process enumeration with eBPF integration (Enterprise tier)
- **macOS**: EndpointSecurity framework integration (Enterprise tier)
- **Windows**: ETW integration and Windows API access (Enterprise tier)
- **Container Support**: Kubernetes DaemonSet deployment

#### Multi-Channel Alerting

- **Local Outputs**: stdout, syslog, file output
- **Network Outputs**: webhooks, email, Kafka
- **SIEM Integration**: Splunk HEC, Elasticsearch, CEF format
- **Enterprise Integration**: STIX/TAXII feeds, federated Security Centers

#### Certificate Transparency Audit Logging

- Cryptographic integrity for forensic analysis
- Certificate Transparency-style Merkle tree with rs-merkle
- Ed25519 digital signatures and inclusion proofs
- Millisecond-precision timestamps

#### Resource-Bounded Operation

- Graceful degradation under load
- Memory pressure detection and response
- CPU throttling under high load conditions
- Circuit breaker patterns for external dependencies

## Technology Stack

### **Core Technologies**

- **Language**: Rust 2024 Edition (MSRV: 1.91+)
- **Safety**: `unsafe_code = "forbid"` at workspace level with comprehensive linting
- **Quality**: `warnings = "deny"` with zero-warnings policy enforced by CI
- **Async Runtime**: Tokio with full feature set for I/O and task management
- **Collection Framework**: collector-core for extensible event source management

### **Database Layer**

- **Core**: redb (pure Rust embedded database) for optimal performance and security
- **Features**: Concurrent access, ACID transactions, zero-copy deserialization
- **Configuration**: Separate event store and audit ledger with different durability settings
- **Business/Enterprise**: PostgreSQL for centralized data aggregation

### **CLI Framework**

- **clap v4**: Derive macros with shell completions (bash, zsh, fish, PowerShell)
- **Terminal**: Automatic color detection, NO_COLOR and TERM=dumb support
- **Output**: JSON and human-readable formats with configurable formatting
- **Arguments**: Comprehensive argument parsing with validation and help generation

### **Configuration Management**

- **Hierarchical loading**: Embedded defaults → System files → User files → Environment → CLI flags
- **Formats**: YAML, JSON, TOML support via figment and config crates
- **Validation**: Comprehensive validation with detailed error messages

### **Error Handling**

- **Libraries**: thiserror for structured error types
- **Applications**: anyhow for error context and chains
- **Pattern**: Graceful degradation with detailed error context

### **Logging & Observability**

- **Structured Logging**: tracing ecosystem with JSON output
- **Metrics**: Optional Prometheus integration
- **Performance**: Built-in performance monitoring and resource tracking

## Performance Requirements

### **System Performance**

- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Process Enumeration**: \<5 seconds for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: \<100ms per detection rule execution

### **Scalability**

- **Single Agent**: Monitor 10,000+ processes with minimal overhead
- **Fleet Management**: Support for 1,000+ agents per Security Center
- **Regional Centers**: Aggregate data from multiple regional deployments
- **Enterprise Federation**: Hierarchical data aggregation and query distribution

## Security Principles

### **Principle of Least Privilege**

- Only procmond runs with elevated privileges when necessary
- Immediate privilege drop after initialization
- Detection and alerting run in user space
- Component-specific database access patterns

### **Defense in Depth**

- Multiple layers of security controls
- Input validation at all boundaries
- Sandboxed rule execution
- Cryptographic integrity verification

### **Zero Trust Architecture**

- Mutual TLS authentication between components
- Certificate-based agent registration
- No implicit trust relationships
- Continuous verification and validation

## License Model

### **Dual-License Strategy**

- **Core Components**: Apache 2.0 licensed (procmond, daemoneye-agent, daemoneye-cli, daemoneye-lib)
- **Business Tier Features**: $199/site one-time license (Security Center, GUI, enhanced connectors, curated rules)
- **Enterprise Tier Features**: Custom pricing (kernel monitoring, federation, STIX/TAXII integration)

### **Feature Gating**

- Compile-time feature gates for tier-specific functionality
- Runtime license validation with cryptographic signatures
- Graceful degradation when license is invalid or expired
- Site restriction validation for license compliance

## Getting Started

### **Quick Start**

1. **Install**: Download and install DaemonEye for your platform
2. **Configure**: Set up basic configuration and detection rules
3. **Deploy**: Start the monitoring services
4. **Monitor**: Use the CLI to query data and manage alerts

### **Next Steps**

- Read the [Architecture Overview](./architecture-overview.md) to understand the system design
- Follow the [Getting Started Guide](./getting-started.md) for detailed setup instructions
- Review the [Operator Guide](../user-guides/operator-guide.md) for day-to-day usage
- Explore the [Configuration Guide](../user-guides/configuration.md) for advanced configuration

---

*DaemonEye represents the next generation of process monitoring, combining the security and performance benefits of Rust with proven threat detection techniques to provide a comprehensive solution for modern security operations.*
