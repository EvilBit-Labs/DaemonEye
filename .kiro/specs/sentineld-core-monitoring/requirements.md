# Requirements Document

## Introduction

SentinelD is a security-focused, high-performance process monitoring system designed to detect suspicious activity on systems through continuous process monitoring, behavioral analysis, and pattern recognition. The system provides security operations teams with a reliable, high-performance threat detection solution that operates independently of external dependencies while maintaining audit-grade integrity.

This specification covers the core monitoring functionality including the collector-core framework, process enumeration, detection rule execution, and alert generation across the three-component architecture: procmond (privileged collector), sentinelagent (detection orchestrator), and sentinelcli (operator interface). The collector-core framework enables extensible monitoring capabilities across multiple domains (process, network, filesystem, performance) while maintaining shared infrastructure and operational consistency.

## Requirements

### Requirement 1

**User Story:** As a security operations analyst, I want to continuously monitor all system processes with minimal performance impact, so that I can detect suspicious activity without degrading system performance.

#### Acceptance Criteria

1. WHEN the system starts THEN procmond SHALL enumerate all accessible processes within 5 seconds for systems with up to 10,000 processes
2. WHEN collecting process data THEN the system SHALL maintain less than 5% sustained CPU usage during continuous monitoring
3. WHEN enumerating processes THEN the system SHALL collect minimum metadata including PID, PPID, name, executable path, and command line arguments
4. WHEN enhanced privileges are available THEN the system SHALL collect additional metadata including memory usage, CPU percentage, and start time
5. WHEN a process is inaccessible THEN the system SHALL log the issue and continue enumeration without failing

### Requirement 2

**User Story:** As a security engineer, I want to verify executable integrity through cryptographic hashing, so that I can detect tampered or malicious binaries.

#### Acceptance Criteria

1. WHEN enumerating processes THEN the system SHALL compute SHA-256 hashes for accessible executable files
2. WHEN an executable file is missing or inaccessible THEN the system SHALL record this state without failing the enumeration
3. WHEN storing process data THEN the system SHALL include hash_algorithm field set to 'sha256' with the computed hash value
4. WHEN hashing executables THEN the system SHALL complete the operation without significantly impacting enumeration performance
5. WHEN system processes restrict file access THEN the system SHALL skip hashing gracefully and continue processing

### Requirement 3

**User Story:** As a threat hunter, I want to execute SQL-based detection rules against process data, so that I can identify suspicious patterns and behaviors using flexible queries.

#### Acceptance Criteria

1. WHEN executing detection rules THEN the system SHALL parse SQL queries using AST validation to prevent injection attacks
2. WHEN validating SQL THEN the system SHALL only allow SELECT statements with approved functions (COUNT, SUM, AVG, MIN, MAX, LENGTH, SUBSTR, datetime functions)
3. WHEN executing queries THEN the system SHALL use prepared statements with read-only database connections
4. WHEN a detection rule executes THEN the system SHALL complete within 30 seconds or timeout with appropriate logging
5. WHEN SQL contains forbidden constructs THEN the system SHALL reject the query and log the attempt for audit purposes

### Requirement 4

**User Story:** As a SOC analyst, I want to receive structured alerts when suspicious activity is detected, so that I can respond quickly to potential threats.

#### Acceptance Criteria

1. WHEN a detection rule matches suspicious activity THEN the system SHALL generate an alert with timestamp, severity, rule_id, title, and description
2. WHEN generating alerts THEN the system SHALL include affected process details including PID, name, and executable path
3. WHEN creating alerts THEN the system SHALL support four severity levels: low, medium, high, and critical
4. WHEN multiple similar alerts occur THEN the system SHALL implement deduplication using configurable keys
5. WHEN alerts are generated THEN the system SHALL store them in the database with delivery tracking information

### Requirement 5

**User Story:** As a security operations team member, I want alerts delivered through multiple channels with reliability guarantees, so that critical threats are not missed due to delivery failures.

#### Acceptance Criteria

1. WHEN delivering alerts THEN the system SHALL support multiple sinks including stdout, syslog, webhook, email, and file output
2. WHEN alert delivery fails THEN the system SHALL implement circuit breaker pattern with configurable failure thresholds
3. WHEN delivery fails repeatedly THEN the system SHALL retry with exponential backoff up to 3 attempts with maximum 60-second delay
4. WHEN alerts cannot be delivered THEN the system SHALL store them in a dead letter queue for later processing
5. WHEN delivering to multiple sinks THEN the system SHALL process them in parallel without blocking other deliveries

### Requirement 6

**User Story:** As a system administrator, I want the monitoring system to operate with minimal privileges and drop them after initialization, so that the security risk is minimized.

#### Acceptance Criteria

1. WHEN starting procmond THEN the system SHALL request only minimal required privileges for process enumeration
2. WHEN enhanced access is configured THEN the system SHALL optionally request platform-specific privileges (CAP_SYS_PTRACE on Linux, SeDebugPrivilege on Windows)
3. WHEN initialization completes THEN procmond SHALL immediately drop all elevated privileges
4. WHEN operating post-initialization THEN all components SHALL run with standard user privileges
5. WHEN privilege operations occur THEN the system SHALL log all privilege changes for audit trail

### Requirement 7

**User Story:** As a compliance officer, I want all security-relevant events recorded in a tamper-evident audit ledger, so that I can verify system integrity for forensic analysis.

#### Acceptance Criteria

1. WHEN security events occur THEN the system SHALL record them in an append-only audit ledger with monotonic sequence numbers
2. WHEN creating audit entries THEN the system SHALL include timestamp, actor, action, payload hash, previous hash, and entry hash
3. WHEN computing hashes THEN the system SHALL use BLAKE3 for fast, cryptographically secure hash computation
4. WHEN verifying integrity THEN the system SHALL provide chain verification function to detect tampering
5. WHEN audit events are recorded THEN the system SHALL maintain proper ordering and millisecond-precision timestamps

### Requirement 8

**User Story:** As an operator, I want a command-line interface to query historical data and manage the system, so that I can investigate incidents and maintain the monitoring system.

#### Acceptance Criteria

1. WHEN querying data THEN sentinelcli SHALL execute user-provided SQL queries with parameterization and prepared statements
2. WHEN displaying results THEN the system SHALL support JSON, human-readable table, and CSV output formats
3. WHEN managing rules THEN the CLI SHALL provide capabilities to list, validate, test, and import/export detection rules
4. WHEN checking system health THEN the CLI SHALL display component status with color-coded indicators
5. WHEN handling large datasets THEN the system SHALL support streaming and pagination for result sets

### Requirement 9

**User Story:** As a security architect, I want the system to operate offline without external dependencies, so that it can function in airgapped environments and during network outages.

#### Acceptance Criteria

1. WHEN network connectivity is unavailable THEN all core functionality SHALL continue operating normally
2. WHEN operating offline THEN process enumeration, detection rules, and database operations SHALL function without degradation
3. WHEN network fails during operation THEN alert delivery SHALL degrade gracefully with local sinks continuing to work
4. WHEN distributing to airgapped systems THEN the system SHALL support bundle-based configuration and rule distribution
5. WHEN importing bundles THEN the system SHALL validate and apply them atomically with conflict resolution

### Requirement 10

**User Story:** As a DevOps engineer, I want the system to provide comprehensive observability and health monitoring, so that I can maintain operational visibility and integrate with existing monitoring infrastructure.

#### Acceptance Criteria

1. WHEN logging events THEN the system SHALL use structured JSON format with consistent field naming and configurable log levels
2. WHEN exposing metrics THEN the system SHALL provide Prometheus-compatible metrics for collection rate, detection latency, and alert delivery
3. WHEN monitoring health THEN the system SHALL expose HTTP health endpoints with component-level status checks
4. WHEN tracking performance THEN the system SHALL embed performance metrics in log entries with correlation IDs
5. WHEN integrating with monitoring systems THEN the system SHALL respect NO_COLOR and TERM=dumb environment variables for console output

### Requirement 11

**User Story:** As a system architect, I want a reusable collector-core framework that enables multiple collection components, so that I can extend monitoring capabilities across different domains while maintaining shared infrastructure.

#### Acceptance Criteria

1. WHEN creating collection components THEN the system SHALL provide a universal EventSource trait that abstracts collection methodology from operational infrastructure
2. WHEN registering event sources THEN the collector-core SHALL support multiple concurrent sources with unified event processing and IPC communication
3. WHEN handling different event types THEN the system SHALL support extensible CollectionEvent enum covering process, network, filesystem, and performance domains
4. WHEN managing component lifecycle THEN the collector-core SHALL provide consistent start/stop, health checks, and graceful shutdown across all registered sources
5. WHEN configuring components THEN the system SHALL share common configuration loading, validation, and environment handling across all collection types

### Requirement 12

**User Story:** As a platform developer, I want the collector-core framework to enable future monitoring components like network, filesystem, and performance monitoring, so that I can build comprehensive behavioral analysis capabilities.

#### Acceptance Criteria

1. WHEN implementing new collection components THEN the system SHALL provide shared IPC server logic, configuration management, and logging infrastructure through collector-core
2. WHEN developing network monitoring THEN the system SHALL support NetworkEvent types with connection tracking, traffic analysis, and DNS monitoring capabilities
3. WHEN implementing filesystem monitoring THEN the system SHALL support FilesystemEvent types with file operations, access patterns, and bulk operation detection
4. WHEN adding performance monitoring THEN the system SHALL support PerformanceEvent types with resource utilization, system metrics, and anomaly detection
5. WHEN correlating multi-domain events THEN sentinelagent SHALL receive unified event streams from multiple collection components for behavioral analysis

### Requirement 13

**User Story:** As a system architect, I want the collector-core framework to enable shared infrastructure between OSS and enterprise features, so that both versions can leverage the same proven operational foundation while supporting different collection capabilities.

#### Acceptance Criteria

1. WHEN implementing enterprise features THEN the system SHALL use the same collector-core infrastructure for IPC, configuration, logging, and runtime management
2. WHEN deploying different versions THEN both OSS and enterprise implementations SHALL use identical operational interfaces and protocols
3. WHEN extending capabilities THEN the system SHALL support capability negotiation that indicates available monitoring features without exposing implementation details
4. WHEN managing licensing THEN the collector-core SHALL remain Apache-2.0 licensed while enabling proprietary EventSource implementations
5. WHEN operating in production THEN both OSS and enterprise versions SHALL provide identical operational characteristics and management interfaces
