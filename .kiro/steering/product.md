# SentinelD Product Overview

SentinelD is a security-focused, high-performance process monitoring system built for cybersecurity professionals, threat hunters, and security operations centers. **Its primary purpose is to detect suspicious activity on systems by monitoring abnormal process behavior and patterns.** This is a complete Rust 2024 rewrite of a proven Python prototype, delivering enterprise-grade performance with audit-grade integrity.

## Core Mission

Detect and alert on suspicious system activity through continuous process monitoring, behavioral analysis, and pattern recognition. Provide security operations teams with a reliable, high-performance threat detection solution that operates independently of external dependencies while maintaining audit-grade integrity and operator-centric workflows.

## Key Value Propositions

- **Audit-Grade Integrity**: Tamper-evident, cryptographically chained logs suitable for compliance and forensics
- **Offline-First Operation**: Full functionality without internet access, perfect for airgapped environments
- **Security-First Architecture**: Privilege separation, sandboxed execution, and minimal attack surface
- **High Performance**: <5% CPU overhead while monitoring 10,000+ processes with sub-second enumeration
- **Operator-Centric Design**: Built for operators, by operators, with workflows optimized for contested environments

## Three-Component Architecture

1. **procmond** (Privileged Process Collector): Runs with elevated privileges, focused solely on process monitoring with minimal attack surface
2. **sentinelagent** (User-Space Orchestrator): Operates in user space with minimal privileges for alerting and network delivery
3. **sentinelcli** (Command-Line Interface): Local interface for data queries, result exports, and service configuration

## Target Users

- SOC Analysts monitoring fleet infrastructure for process anomalies
- Security Operations & Incident Response Teams investigating compromised systems
- System Reliability Engineers requiring low-overhead monitoring
- Blue Team Security Engineers integrating with existing security infrastructure
- DevSecOps Teams embedding security monitoring in deployments

## Key Features

### Threat Detection Capabilities

- **Process Behavior Analysis**: Detect process hollowing, executable integrity violations, suspicious parent-child relationships
- **Anomaly Detection**: Identify unusual resource consumption patterns and suspicious process name duplications
- **SQL-Based Detection Engine**: Flexible rule creation using standard SQL queries with sandboxed execution
- **Built-in Detection Rules**: Comprehensive library covering common threat patterns and attack techniques

### System Integration

- Cross-platform support (Linux, macOS, Windows) with native OS integration
- Multi-channel alerting (stdout, syslog, webhooks, email) for SIEM integration
- Tamper-evident audit logging with cryptographic integrity for forensic analysis
- Resource-bounded operation with graceful degradation under load
