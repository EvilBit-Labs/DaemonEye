# SentinelD Requirements Specification

## Abstract

This document defines the functional and non-functional requirements for SentinelD, addressing
critical architecture issues identified in the review process. Requirements are organized by
component with measurable acceptance criteria and verification strategies. Special emphasis is
placed on addressing database performance bottlenecks, SQL injection prevention, resource
management, and deployment consistency.

## How to Read This Document

This document provides measurable requirements for SentinelD implementation. See also:

- [Product Specification](product.md) - Mission, architecture overview, and value proposition
- [Technical Stack](tech.md) - Technologies, patterns, and design decisions
- [Project Structure](structure.md) - Workspace layout and module organization
- [Tasks & Milestones](tasks.md) - Development phases and priorities

**Verification Strategy**: Each requirement includes specific acceptance criteria and test methods
(Unit/Integration/Performance).

---

## Functional Requirements

### FR-1: Process Monitoring (procmond)

#### FR-1.1: Cross-Platform Process Enumeration

**Requirement**: procmond must enumerate all accessible system processes on Linux, macOS, and
Windows using a unified interface with platform-specific optimizations.

**Acceptance Criteria**:

- ✅ Successfully enumerate processes on each target platform
- ✅ Collect minimum metadata: PID, PPID, name, executable path, command line
- ✅ Collect enhanced metadata when privileges allow: memory usage, CPU %, start time
- ✅ Handle inaccessible processes gracefully (log but continue)
- ✅ Complete enumeration within performance targets (see NFR-1.1)

**Verification**:

- **Unit Tests**: Mock process enumerators for each platform
- **Integration Tests**: Real enumeration on GitHub Actions CI matrix
- **Performance Tests**: Criterion benchmarks with 1000+ process scenarios

#### FR-1.2: Executable Integrity Verification

**Requirement**: procmond must compute SHA-256 hashes of executable files to enable tamper
detection.

**Acceptance Criteria**:

- ✅ Compute SHA-256 hash for accessible executable files
- ✅ Store hash_algorithm field ('sha256') with hash value
- ✅ Handle cases where executable file is missing or inaccessible
- ✅ Skip hashing for system processes where file access is restricted
- ✅ Complete hashing without significantly impacting enumeration performance

**Verification**:

- **Unit Tests**: Hash computation with known test files
- **Integration Tests**: Verify hashes match expected values for system binaries
- **Performance Tests**: Measure hashing overhead on enumeration time

#### FR-1.3: Batched Database Writes with Backpressure

**Requirement**: procmond must write process data using batched transactions with configurable
backpressure policies to prevent resource exhaustion.

**Acceptance Criteria**:

- ✅ Batch process records (default: 1000 records per transaction)
- ✅ Implement configurable backpressure policies:
  - DropOldest: Evict oldest records when queue full
  - BlockWithTimeout: Block collection up to timeout
  - SpillToDisk: Overflow to temporary disk storage
  - AdaptiveThrottling: Reduce collection frequency under pressure
- ✅ Use SQLite WAL mode with optimized PRAGMA settings
- ✅ Achieve target write performance (see NFR-1.2)
- ✅ Maintain data integrity during backpressure scenarios

**Verification**:

- **Unit Tests**: Backpressure policy behavior with mock queues
- **Integration Tests**: SQLite transaction behavior and WAL checkpoint
- **Load Tests**: Sustained high-volume writing with queue overflow scenarios

#### FR-1.4: Privilege Management

**Requirement**: procmond must operate with minimal required privileges and drop them immediately
after initialization.

**Acceptance Criteria**:

- ✅ Start with minimal privileges by default
- ✅ Optionally request enhanced privileges when configured:
  - Linux: CAP_SYS_PTRACE for process memory access
  - Windows: SeDebugPrivilege for enhanced process information
  - macOS: Appropriate entitlements for process access
- ✅ Drop all elevated privileges immediately after successful initialization
- ✅ Continue operating with standard user privileges post-drop
- ✅ Log privilege operations for audit trail

**Verification**:

- **Unit Tests**: Privilege drop simulation and verification
- **Integration Tests**: Actual privilege verification on each platform
- **Security Tests**: Attempt privileged operations post-drop (should fail)

### FR-2: Detection Engine (sentinelagent)

#### FR-2.1: SQL Rule Validation and Execution

**Requirement**: sentinelagent must execute SQL-based detection rules with comprehensive security
validation to prevent injection attacks.

**Acceptance Criteria**:

- ✅ Parse SQL queries using sqlparser-rs with SQLite dialect
- ✅ Enforce whitelist of allowed SQL constructs:
  - **Allowed**: SELECT statements, WHERE/GROUP BY/HAVING/ORDER BY/LIMIT clauses
  - **Allowed Functions**: COUNT, SUM, AVG, MIN, MAX, LENGTH, SUBSTR, datetime functions
  - **Forbidden**: INSERT/UPDATE/DELETE, CREATE/DROP/ALTER, PRAGMA, ATTACH/DETACH
- ✅ Execute all queries using prepared statements with positional parameters
- ✅ Use read-only database connections for rule execution
- ✅ Apply query timeouts (default: 30 seconds) and memory limits
- ✅ Log all rule execution attempts with performance metrics

**Verification**:

- **Unit Tests**: AST validation with allowed/forbidden query samples
- **Security Tests**: Injection attempt blocking (100% block rate required)
- **Fuzzing Tests**: Malformed SQL input with cargo-fuzz
- **Integration Tests**: Rule execution against sample process data

#### FR-2.2: Sandboxed Detection Execution

**Requirement**: Detection rules must execute in a sandboxed environment with resource limits and
isolation.

**Acceptance Criteria**:

- ✅ Execute detection rules in separate process (optional: sentinel-detexec)
- ✅ Apply OS-specific sandboxing:
  - Linux: seccomp-bpf profiles, user namespaces, rlimits
  - Windows: Job objects with restricted tokens
  - macOS: Resource limits and sandbox profiles
- ✅ Enforce resource limits per execution:
  - Memory limit: 64MB default
  - CPU time limit: 30 seconds default
  - File descriptor limits: minimal required set
- ✅ Support cancellation and cleanup of long-running rules
- ✅ Provide fallback execution when sandboxing unavailable

**Verification**:

- **Unit Tests**: Resource limit enforcement with mock executors
- **Integration Tests**: Actual sandbox behavior on each platform
- **Load Tests**: Concurrent rule execution under resource pressure

#### FR-2.3: Alert Generation and Management

**Requirement**: sentinelagent must generate structured alerts from detection results with
deduplication and context.

**Acceptance Criteria**:

- ✅ Generate alerts with required fields:
  - Timestamp, severity, rule_id, title, description
  - Affected process details (PID, name, executable path)
  - Rule execution metadata (execution time, query results)
- ✅ Support four severity levels: low, medium, high, critical
- ✅ Implement alert deduplication using configurable keys
- ✅ Store alerts in SQLite with delivery tracking
- ✅ Provide alert context for forensic analysis

**Verification**:

- **Unit Tests**: Alert generation from detection results
- **Integration Tests**: End-to-end detection and alert pipeline
- **Schema Tests**: Alert database storage and retrieval

#### FR-2.4: Rule Management and Hot-Reloading

**Requirement**: sentinelagent must support loading, validating, and hot-reloading detection rules
from multiple sources.

**Acceptance Criteria**:

- ✅ Load rules from multiple sources:
  - Built-in rule library (embedded in binary)
  - File-based rules (.sql files with .yaml metadata)
  - Database-stored user rules
- ✅ Validate rule syntax and metadata on load
- ✅ Support rule versioning and metadata tracking
- ✅ Enable/disable individual rules via configuration
- ✅ Hot-reload rules without service restart (SIGHUP signal)
- ✅ Maintain rule execution history and performance metrics

**Verification**:

- **Unit Tests**: Rule loading from various sources
- **Integration Tests**: Hot-reload functionality with signal handling
- **Performance Tests**: Rule loading performance with large rule sets

### FR-3: Alert Delivery (sentinelagent)

#### FR-3.1: Multi-Channel Alert Delivery

**Requirement**: sentinelagent must deliver alerts through multiple channels with reliability
guarantees and delivery tracking.

**Acceptance Criteria**:

- ✅ Support alert sinks: stdout, syslog, webhook, email, file
- ✅ Deliver alerts in structured formats (JSON) and human-readable text
- ✅ Implement parallel delivery to multiple sinks simultaneously
- ✅ Track delivery attempts, successes, and failures in database
- ✅ Support sink-specific configuration (timeouts, formats, filters)

**Verification**:

- **Unit Tests**: Individual sink delivery with mocks
- **Integration Tests**: Real delivery to test endpoints
- **Load Tests**: High-volume alert delivery with multiple sinks

#### FR-3.2: Delivery Reliability and Circuit Breaking

**Requirement**: Alert delivery must be resilient to failures with circuit breakers, retries, and
dead letter queues.

**Acceptance Criteria**:

- ✅ Implement circuit breaker pattern for each alert sink:
  - Open circuit after 5 consecutive failures (configurable)
  - Half-open circuit after 30 seconds (configurable)
  - Close circuit after 2 consecutive successes
- ✅ Retry failed deliveries with exponential backoff:
  - Initial delay: 1 second, max delay: 60 seconds
  - Maximum retry attempts: 3 (configurable)
  - Jitter to prevent thundering herd
- ✅ Store permanently failed alerts in dead letter queue
- ✅ Provide dead letter queue management via CLI
- ✅ Maintain delivery success rate metrics (target: >99%)

**Verification**:

- **Unit Tests**: Circuit breaker state transitions
- **Integration Tests**: Retry behavior with simulated failures
- **Chaos Tests**: Network partition and endpoint failure scenarios

### FR-4: Command-Line Interface (sentinelcli)

#### FR-4.1: Data Querying and Export

**Requirement**: sentinelcli must provide flexible SQL-based querying of historical process data
with multiple output formats.

**Acceptance Criteria**:

- ✅ Execute user-provided SQL queries against process database
- ✅ Support query parameterization and prepared statements
- ✅ Output formats: JSON, human-readable table, CSV
- ✅ Handle large result sets with streaming and pagination
- ✅ Respect terminal capabilities (color, width detection)
- ✅ Support NO_COLOR and TERM=dumb environment variables

**Verification**:

- **Unit Tests**: Query parsing and parameter binding
- **Integration Tests**: CLI invocation with assert_cmd
- **Output Tests**: Snapshot testing with insta for consistent formatting

#### FR-4.2: Rule Management Operations

**Requirement**: sentinelcli must provide comprehensive rule management capabilities including
validation, testing, and import/export.

**Acceptance Criteria**:

- ✅ List all available rules with status (enabled/disabled)
- ✅ Validate rule syntax and metadata without execution
- ✅ Test rules against historical data with performance metrics
- ✅ Import/export rule packages for distribution
- ✅ Enable/disable rules with configuration validation
- ✅ Display rule execution statistics and alert history

**Verification**:

- **Unit Tests**: Rule management operations with mock data
- **Integration Tests**: Full CLI workflow with temporary database
- **Performance Tests**: Rule testing against large datasets

#### FR-4.3: System Health and Diagnostics

**Requirement**: sentinelcli must provide comprehensive system diagnostics and health monitoring
capabilities.

**Acceptance Criteria**:

- ✅ Display system health overview with color-coded status
- ✅ Check component connectivity (database, alert sinks)
- ✅ Report performance metrics and resource usage
- ✅ Validate configuration files with detailed error messages
- ✅ Test alert delivery with configurable test messages
- ✅ Provide troubleshooting guidance for common issues

**Verification**:

- **Unit Tests**: Health check logic with simulated components
- **Integration Tests**: Full health check against running system
- **Error Tests**: Configuration validation with invalid inputs

### FR-5: Tamper-Evident Audit Logging

#### FR-5.1: Cryptographic Audit Chain

**Requirement**: All components must maintain a tamper-evident audit ledger using cryptographic
chaining for compliance and forensics.

**Acceptance Criteria**:

- ✅ Append-only audit ledger table with monotonic sequence numbers
- ✅ Each entry contains: timestamp, actor, action, payload hash, previous hash, entry hash
- ✅ Use BLAKE3 for hash computation (fast, cryptographically secure)
- ✅ Optional Ed25519 digital signatures for entries
- ✅ Chain verification function to detect tampering
- ✅ Export audit chain segments for external verification

**Verification**:

- **Unit Tests**: Hash chain construction and verification
- **Crypto Tests**: Signature generation and validation
- **Tampering Tests**: Deliberate modification detection

#### FR-5.2: Audit Event Recording

**Requirement**: Components must record security-relevant events in the audit ledger with structured
context.

**Acceptance Criteria**:

- ✅ Record events: process_scan_start, process_scan_complete, rule_execute, alert_generate,
  config_change
- ✅ Include structured event data (JSON format)
- ✅ Associate events with specific component (procmond, sentinelagent, sentinelcli)
- ✅ Maintain event ordering and timestamps (millisecond precision)
- ✅ Survive component restarts and failures

**Verification**:

- **Unit Tests**: Event recording with structured data
- **Integration Tests**: Multi-component audit event sequencing
- **Durability Tests**: Event persistence through restart scenarios

### FR-6: Offline-First Operation

#### FR-6.1: No External Dependencies

**Requirement**: All core functionality must operate without internet connectivity or external
services.

**Acceptance Criteria**:

- ✅ Process enumeration works offline
- ✅ Detection rule execution works offline
- ✅ Database operations work offline (embedded SQLite)
- ✅ Configuration loading works offline
- ✅ Alert delivery degrades gracefully (local sinks continue working)
- ✅ All diagnostics and health checks work offline

**Verification**:

- **Integration Tests**: Full system operation with network disabled
- **Container Tests**: Airgapped container environment testing
- **Fallback Tests**: Network failure during operation

#### FR-6.2: Bundle-Based Distribution

**Requirement**: Configuration, rules, and data must support bundle-based distribution for airgapped
environments.

**Acceptance Criteria**:

- ✅ Export complete configuration as self-contained bundle
- ✅ Export rule sets as versioned packages
- ✅ Export historical data in portable formats (JSON, SQLite)
- ✅ Import bundles with validation and conflict resolution
- ✅ Support bundle signing and verification
- ✅ Atomic bundle application (all-or-nothing)

**Verification**:

- **Unit Tests**: Bundle creation and validation
- **Integration Tests**: Bundle import/export workflows
- **Migration Tests**: Bundle application to different systems

---

## Non-Functional Requirements

### NFR-1: Performance Requirements

#### NFR-1.1: Process Enumeration Performance

**Requirement**: Process enumeration must meet strict performance targets under normal and stressed
conditions.

**Targets**:

- **Baseline**: Complete enumeration of 1,000 processes in \<2 seconds
- **Scale**: Complete enumeration of 10,000 processes in \<5 seconds
- **CPU Impact**: \<5% sustained CPU usage during continuous monitoring
- **Memory Impact**: \<100MB resident memory under normal operation

**Acceptance Criteria**:

- ✅ Meet performance targets on GitHub Actions CI runners
- ✅ Demonstrate linear or sub-linear scaling with process count
- ✅ Maintain performance under concurrent rule execution
- ✅ Performance regression detection with 10% tolerance

**Verification**:

- **Benchmark Tests**: Criterion-based performance testing
- **Load Tests**: Synthetic high-process-count scenarios
- **Regression Tests**: Baseline comparison in CI

#### NFR-1.2: Database Write Performance

**Requirement**: Database write operations must achieve high throughput while maintaining data
integrity.

**Targets**:

- **Write Rate**: >1,000 process records per second sustained
- **Batch Efficiency**: >5,000 records per second in batch mode
- **Transaction Latency**: \<100ms for 1,000-record batches
- **Concurrent Read**: No blocking of read operations during writes

**Acceptance Criteria**:

- ✅ Meet write rate targets with SQLite WAL mode
- ✅ Maintain performance with concurrent detection queries
- ✅ Handle backpressure scenarios without data loss
- ✅ Demonstrate fsync efficiency with coalescing

**Critical Issue Mitigation**: Addresses "Database Write Bottleneck" from architecture review.

**Verification**:

- **Performance Tests**: Sustained write load testing
- **Concurrency Tests**: Read/write operation interference
- **Durability Tests**: WAL checkpoint and recovery behavior

#### NFR-1.3: Detection Rule Performance

**Requirement**: Detection rule execution must complete within acceptable latency bounds.

**Targets**:

- **Rule Latency**: \<100ms per rule execution (median)
- **Concurrent Rules**: Support 20+ concurrent rule executions
- **Query Timeout**: Hard limit at 30 seconds per rule
- **Memory per Rule**: \<64MB memory usage per sandboxed execution

**Acceptance Criteria**:

- ✅ Meet latency targets with realistic process datasets
- ✅ Handle complex analytical queries within timeout
- ✅ Sandbox overhead \<20% performance penalty
- ✅ Resource cleanup after rule completion

**Verification**:

- **Benchmark Tests**: Rule execution timing with various query types
- **Resource Tests**: Memory and CPU usage measurement
- **Sandbox Tests**: Isolation overhead measurement

#### NFR-1.4: Alert Delivery Performance

**Requirement**: Alert delivery must be responsive and efficient across multiple channels.

**Targets**:

- **Delivery Latency**: \<1 second from detection to sink delivery
- **Parallel Delivery**: Support 100+ concurrent alert deliveries
- **Circuit Recovery**: \<30 seconds to detect and recover from sink failures
- **Success Rate**: >99% delivery success under normal conditions

**Acceptance Criteria**:

- ✅ Meet latency targets for all supported sink types
- ✅ Scale to handle alert bursts without queue overflow
- ✅ Circuit breaker reaction within tolerance
- ✅ Maintain success rate under realistic failure scenarios

**Verification**:

- **Latency Tests**: End-to-end delivery timing
- **Burst Tests**: High-volume alert generation scenarios
- **Reliability Tests**: Sink failure and recovery simulation

### NFR-2: Security Requirements

#### NFR-2.1: SQL Injection Prevention

**Requirement**: The system must be completely immune to SQL injection attacks through comprehensive
input validation.

**Acceptance Criteria**:

- ✅ Block 100% of OWASP SQL injection test vectors
- ✅ AST validation rejects all non-SELECT statements
- ✅ Prepared statements used exclusively for user-provided queries
- ✅ Read-only database connections for detection rules
- ✅ Audit log all rejected query attempts
- ✅ Sandbox execution prevents privilege escalation

**Critical Issue Mitigation**: Addresses "SQL Injection Vulnerability" from architecture review.

**Verification**:

- **Security Tests**: Comprehensive injection attempt battery
- **Fuzzing Tests**: Malformed SQL input with high iteration count
- **Penetration Tests**: External security assessment (recommended)

#### NFR-2.2: Privilege Separation

**Requirement**: System must maintain strict privilege boundaries between components.

**Acceptance Criteria**:

- ✅ procmond operates with minimal required privileges only
- ✅ sentinelagent operates in user space (no elevated privileges)
- ✅ sentinelcli operates in user space (no elevated privileges)
- ✅ Network access limited to sentinelagent outbound connections only
- ✅ Database access follows principle of least privilege
- ✅ Inter-process communication uses secure, local-only channels

**Verification**:

- **Security Tests**: Privilege verification on each platform
- **Integration Tests**: Component interaction boundary testing
- **Network Tests**: Verify no unexpected network access

#### NFR-2.3: Resource Security

**Requirement**: System must prevent resource exhaustion attacks and gracefully handle resource
pressure.

**Acceptance Criteria**:

- ✅ Bounded memory usage with configurable limits
- ✅ Bounded queue sizes with overflow policies
- ✅ CPU usage limits with cooperative scheduling
- ✅ File descriptor limits with resource cleanup
- ✅ Query timeouts prevent runaway detection rules
- ✅ Circuit breakers prevent cascade failures

**Critical Issue Mitigation**: Addresses "Unbounded Resource Growth" from architecture review.

**Verification**:

- **Load Tests**: Resource exhaustion attempt scenarios
- **Stress Tests**: Sustained high resource usage
- **Limit Tests**: Boundary condition verification

#### NFR-2.4: Input Validation

**Requirement**: All input must be validated at system boundaries to prevent exploitation.

**Acceptance Criteria**:

- ✅ Configuration file validation with detailed error messages
- ✅ SQL query AST validation with comprehensive whitelist
- ✅ File path validation prevents directory traversal
- ✅ Network input validation for webhook and email configuration
- ✅ Process data sanitization prevents log injection
- ✅ Binary data validation for executable hashing

**Verification**:

- **Security Tests**: Malicious input validation across all boundaries
- **Fuzzing Tests**: Random input generation with edge cases
- **Sanitization Tests**: Log injection and data contamination prevention

### NFR-3: Reliability Requirements

#### NFR-3.1: Data Integrity

**Requirement**: System must ensure data integrity under all operating conditions including failures
and restarts.

**Acceptance Criteria**:

- ✅ SQLite WAL mode ensures crash-safe database operations
- ✅ Idempotent writes prevent data duplication on retry
- ✅ Audit ledger cryptographic verification detects tampering
- ✅ Configuration validation prevents invalid system states
- ✅ Atomic operations for critical state changes
- ✅ Graceful degradation during storage failures

**Verification**:

- **Durability Tests**: Crash simulation during database operations
- **Integrity Tests**: Audit chain verification after restarts
- **Recovery Tests**: System recovery from various failure modes

#### NFR-3.2: High Availability

**Requirement**: System must maintain operational availability under adverse conditions.

**Targets**:

- **Uptime**: >99.9% availability during normal operation
- **Recovery Time**: \<30 seconds restart after failure
- **Graceful Shutdown**: Complete shutdown within 60 seconds
- **Data Loss**: Zero data loss for committed transactions

**Acceptance Criteria**:

- ✅ Automatic restart capabilities with systemd/launchd/Windows Service
- ✅ Graceful shutdown handling with signal coordination
- ✅ Resource cleanup during abnormal termination
- ✅ State recovery from persistent storage

**Verification**:

- **Availability Tests**: Extended runtime monitoring
- **Restart Tests**: Automated restart scenario validation
- **Signal Tests**: Graceful shutdown signal handling

#### NFR-3.3: Error Handling and Recovery

**Requirement**: System must handle errors gracefully and provide recovery mechanisms.

**Acceptance Criteria**:

- ✅ Structured error types with context information
- ✅ Error logging with appropriate detail levels
- ✅ Automatic retry for transient failures
- ✅ Circuit breakers for persistent failures
- ✅ Fallback behaviors when primary paths fail
- ✅ Error propagation without system-wide failures

**Verification**:

- **Error Tests**: Systematic error injection scenarios
- **Recovery Tests**: Automatic recovery validation
- **Logging Tests**: Error message clarity and usefulness

### NFR-4: Portability Requirements

#### NFR-4.1: Cross-Platform Support

**Requirement**: System must provide consistent functionality across Linux, macOS, and Windows.

**Acceptance Criteria**:

- ✅ Identical core functionality on all platforms
- ✅ Platform-specific optimizations where beneficial
- ✅ Graceful degradation when platform features unavailable
- ✅ Consistent configuration and CLI behavior
- ✅ Same performance characteristics (within 20% variance)
- ✅ Feature parity documentation and testing

**Verification**:

- **Matrix Tests**: GitHub Actions CI testing on all platforms
- **Feature Tests**: Platform-specific capability verification
- **Performance Tests**: Cross-platform benchmark comparison

#### NFR-4.2: Deployment Flexibility

**Requirement**: System must support various deployment models and environments.

**Acceptance Criteria**:

- ✅ Static binary deployment (no external dependencies)
- ✅ System service integration (systemd, launchd, Windows Service)
- ✅ Container deployment with proper privilege handling
- ✅ Package manager distribution (APT, Homebrew, Chocolatey planned)
- ✅ Configuration via files, environment variables, and CLI arguments
- ✅ Offline installation and operation support

**Critical Issue Mitigation**: Addresses "Container Deployment Contradiction" from architecture
review.

**Verification**:

- **Deployment Tests**: Installation scenarios on clean systems
- **Container Tests**: Docker deployment with various configurations
- **Configuration Tests**: All configuration methods and precedence

### NFR-5: Observability Requirements

#### NFR-5.1: Structured Logging

**Requirement**: System must provide comprehensive structured logging for operational visibility.

**Acceptance Criteria**:

- ✅ JSON-formatted logs with consistent field naming
- ✅ Configurable log levels with runtime adjustment
- ✅ Request tracing with correlation IDs
- ✅ Performance metrics embedded in log entries
- ✅ Respect NO_COLOR and TERM=dumb for console output
- ✅ Log rotation and size management integration

**Verification**:

- **Logging Tests**: Log format validation and parsing
- **Level Tests**: Dynamic log level adjustment
- **Output Tests**: Terminal compatibility across environments

#### NFR-5.2: Performance Metrics

**Requirement**: System must expose performance metrics for monitoring and alerting.

**Acceptance Criteria**:

- ✅ Prometheus-compatible metrics export (optional feature)
- ✅ Key performance indicators: collection rate, detection latency, alert delivery
- ✅ Resource utilization metrics: CPU, memory, disk usage
- ✅ Error rate metrics: failed operations, circuit breaker states
- ✅ Business metrics: process counts, alert volumes, rule performance
- ✅ Historical metric trends for capacity planning

**Verification**:

- **Metrics Tests**: Metric accuracy and consistency
- **Export Tests**: Prometheus scraping compatibility
- **Performance Tests**: Metric collection overhead measurement

#### NFR-5.3: Health Monitoring

**Requirement**: System must provide health endpoints for external monitoring systems.

**Acceptance Criteria**:

- ✅ HTTP health endpoint (localhost-only binding)
- ✅ Component-level health checks (database, alert sinks)
- ✅ Dependency health verification (configuration, rule files)
- ✅ Performance-based health indicators (latency thresholds)
- ✅ Health history for trend analysis
- ✅ Custom health check plugin interface

**Verification**:

- **Health Tests**: Endpoint response validation
- **Integration Tests**: External monitoring system compatibility
- **Failure Tests**: Health check behavior during component failures

### NFR-6: Quality Requirements

#### NFR-6.1: Code Quality Standards

**Requirement**: Codebase must maintain high quality standards with automated enforcement.

**Acceptance Criteria**:

- ✅ Zero warnings with `cargo clippy -- -D warnings`
- ✅ Consistent formatting with `cargo fmt`
- ✅ Memory safety guaranteed with `unsafe_code = "forbid"`
- ✅ Documentation coverage >90% for public APIs
- ✅ Test coverage >85% with detailed reporting
- ✅ Dependency vulnerability scanning with `cargo audit`

**Verification**:

- **CI Tests**: Automated quality gate enforcement
- **Coverage Tests**: Code coverage measurement and reporting
- **Security Tests**: Dependency vulnerability assessment

#### NFR-6.2: Testing Requirements

**Requirement**: Comprehensive testing strategy must validate all functionality and performance
characteristics.

**Acceptance Criteria**:

- ✅ Unit tests for all core functionality (>85% coverage)
- ✅ Integration tests with assert_cmd for CLI testing
- ✅ Performance tests with criterion for regression detection
- ✅ Snapshot tests with insta for output validation
- ✅ Property-based tests with proptest for edge cases
- ✅ Cross-platform test execution in CI

**Verification**:

- **Test Results**: CI test execution and reporting
- **Coverage Reports**: Detailed test coverage analysis
- **Performance Reports**: Benchmark results and trend analysis

---

## Critical Architecture Issue Mitigation

### Database Write Bottlenecks

**Problem**: SQLite single-writer limitation with unbounded queue growth leads to system failure
under load.

**Solution Requirements**:

- ✅ Implement bounded write queues with configurable capacity
- ✅ Provide multiple backpressure policies (DropOldest, BlockWithTimeout, SpillToDisk)
- ✅ Use WAL mode with optimized PRAGMA settings
- ✅ Separate event store and audit ledger for different durability requirements
- ✅ Implement fsync coalescing and checkpoint management
- ✅ Performance gates in testing to prevent regression

**Verification**: Load testing with >1000 records/second sustained write rate.

### SQL Injection Prevention

**Problem**: User-provided detection rules executed directly against SQLite without validation.

**Solution Requirements**:

- ✅ Parse all SQL with AST validation using sqlparser-rs
- ✅ Enforce strict whitelist of allowed SQL constructs
- ✅ Use prepared statements exclusively for user queries
- ✅ Read-only database connections for detection execution
- ✅ Optional sandboxed execution in separate process
- ✅ Comprehensive security testing with injection attempts

**Verification**: 100% blocking of OWASP SQL injection test vectors.

### Resource Management

**Problem**: Unbounded resource growth leads to memory exhaustion and system instability.

**Solution Requirements**:

- ✅ Bounded channels with configurable capacity limits
- ✅ Memory budget enforcement with cooperative scheduling
- ✅ Task pool limits with backpressure handling
- ✅ Query timeouts and cancellation support
- ✅ Circuit breakers for external dependencies
- ✅ Resource cleanup and graceful shutdown

**Verification**: Load testing with sustained high resource usage scenarios.

### Deployment Strategy Consistency

**Problem**: Contradictory documentation about container deployment creates confusion.

**Solution Requirements**:

- ✅ Clear documentation of supported deployment models
- ✅ Explicit container requirements (privileged access for host monitoring)
- ✅ Service integration templates (systemd, launchd, Windows Service)
- ✅ Configuration management documentation with examples
- ✅ Migration guide from conflicting previous documentation
- ✅ Deployment testing in CI for all documented scenarios

**Verification**: Successful deployment using documented procedures.

---

## Acceptance and Verification Strategy

### Requirement Verification Levels

**Unit Testing (70% of verification effort)**:

- Individual function and module testing
- Mock-based isolation testing
- Property-based testing for edge cases
- Snapshot testing for output validation

**Integration Testing (25% of verification effort)**:

- Component interaction testing
- CLI integration with assert_cmd
- Database integration with temporary instances
- Cross-platform compatibility testing

**System Testing (5% of verification effort)**:

- End-to-end workflow validation
- Performance and load testing
- Security and penetration testing
- Deployment scenario validation

### Continuous Verification

**Development Phase**:

- Pre-commit hooks for code quality
- Automated testing on every commit
- Performance regression detection
- Security scan integration

**Release Phase**:

- Comprehensive test suite execution
- Cross-platform compatibility verification
- Performance benchmark validation
- Security assessment review

### Traceability Matrix

Each requirement maps to specific test cases and verification methods. This traceability ensures
comprehensive validation and enables impact analysis for changes.

**Example Traceability**:

- FR-1.1 (Process Enumeration) → `unit_tests::enumeration`, `integration_tests::platform_compat`,
  `perf_tests::enum_benchmark`
- NFR-1.2 (Database Performance) → `perf_tests::write_throughput`, `load_tests::sustained_writes`,
  `regression_tests::db_perf`

---

## Conclusion

This requirements specification addresses all critical architecture issues identified in the review
process while establishing measurable acceptance criteria for functional and non-functional
requirements. The emphasis on performance, security, and reliability ensures that SentinelD will be
suitable for production security operations environments.

Key requirement themes include:

1. **Security-First**: Comprehensive SQL injection prevention, privilege separation, and input
   validation
2. **Performance-Conscious**: Specific targets for throughput, latency, and resource usage
3. **Reliability-Focused**: Data integrity, high availability, and graceful error handling
4. **Operationally-Aware**: Cross-platform consistency, deployment flexibility, and observability
5. **Quality-Driven**: Automated enforcement of code quality and comprehensive testing

The verification strategy ensures all requirements are measurable and testable, providing confidence
in the system's fitness for security-critical production environments.
