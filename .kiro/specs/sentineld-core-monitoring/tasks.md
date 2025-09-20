# Implementation Plan

- [x] 1. Set up workspace structure and core library foundations - [#33](https://github.com/EvilBit-Labs/SentinelD/issues/33)

  - Create Cargo workspace with three binary crates (procmond, sentinelagent, sentinelcli) and shared library (sentinel-lib)
  - Configure workspace-level lints: `unsafe_code = "forbid"`, `warnings = "deny"`
  - Set up basic error handling with thiserror for libraries and anyhow for applications
  - Implement structured logging with tracing ecosystem and JSON formatting
  - _Requirements: 6.4, 10.1_

- [x] 2. Implement core data models and serialization - [#34](https://github.com/EvilBit-Labs/SentinelD/issues/34)

  - Create ProcessRecord struct with comprehensive metadata fields (PID, PPID, name, executable_path, etc.)
  - Implement Alert struct with severity levels, deduplication keys, and context information
  - Create DetectionRule struct with SQL query, metadata, and versioning support
  - Add serde serialization/deserialization for all data structures
  - Write unit tests for data model validation and serialization
  - _Requirements: 1.3, 4.1, 4.3_

- [ ] 3. Migrate to interprocess crate for IPC protocol

- [x] 3.1 Define protobuf schema for IPC messages - [#35](https://github.com/EvilBit-Labs/SentinelD/issues/35)

  - Create .proto files for DetectionTask, DetectionResult, ProcessFilter, and HashCheck message types
  - Add build.rs configuration for protobuf code generation
  - Generate Rust code from protobuf definitions and verify compilation
  - _Requirements: 3.1, 3.2_

- [x] 3.2 Implement legacy custom IPC server for procmond - [#36](https://github.com/EvilBit-Labs/SentinelD/issues/36)

  - Create IpcServer trait with async message handling methods
  - Implement Unix socket server for Linux/macOS with proper error handling
  - Add named pipe server implementation for Windows
  - Write unit tests for server connection handling and message parsing
  - _Requirements: 3.1, 3.2_

- [x] 3.3 Migrate to interprocess crate with unified local socket API

  - Add interprocess crate dependency with tokio feature for cross-platform local sockets
  - Implement interprocess-based IpcServer maintaining same trait interface
  - Create codec layer preserving protobuf framing with CRC32 validation
  - Ensure Unix domain socket permissions (0700 dir, 0600 socket) and Windows named pipe SDDL restrictions (revisit later)
  - _Requirements: 3.1, 3.2_

- [x] 3.4 Implement interprocess IpcClient for sentinelagent - [#37](https://github.com/EvilBit-Labs/SentinelD/issues/37)

  - Create IpcClient using `interprocess` LocalSocketStream with connection management
  - Add automatic reconnection logic with exponential backoff using existing patterns
  - Implement connection lifecycle management (connect, disconnect, health checks)
  - Implement message timeout, size limits, and error handling implementation, maximizing cross-platform portability and reusability
  - Write unit tests for client connection scenarios and cross-platform compatibility
  - _Requirements: 3.1, 3.2_

- [ ] 3.5 Comprehensive testing and validation for interprocess communication - [#38](https://github.com/EvilBit-Labs/SentinelD/issues/38)

  - Create integration tests for `interprocess` transport behavior
  - Add cross-platform tests (Linux, macOS, Windows) for local socket functionality
  - Implement integration tests for cross-component interaction with `interprocess`, specifically testing task distribution and result collection workflows
  - Implement integration tests for error handling and recovery scenarios on both client and server sides for communication reliability
  - Implement property-based tests for codec robustness with malformed inputs, ensuring maximum resilience and security
  - Add performance benchmarks ensuring no regression in message throughput or latency
  - Create security validation tests for socket permissions and connection limits
  - _Requirements: 3.1, 3.2_

- [ ] 4. Implement collector-core framework for extensible monitoring architecture

- [ ] 4.1 Create collector-core library foundation - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Create new collector-core workspace member crate with public API exports
  - Define universal EventSource trait with start/stop lifecycle and capability reporting
  - Implement Collector struct for runtime management and event source registration
  - Create extensible CollectionEvent enum supporting process, network, filesystem, and performance domains
  - Add SourceCaps bitflags for capability negotiation (PROCESS, NETWORK, FILESYSTEM, PERFORMANCE, REALTIME, KERNEL_LEVEL, SYSTEM_WIDE)
  - Write unit tests for EventSource trait and Collector registration
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 4.2 Integrate existing IPC infrastructure into collector-core - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Move existing IPC server logic from procmond to collector-core runtime
  - Integrate existing InterprocessServer and IpcCodec with collector-core event handling
  - Preserve existing protobuf protocol and CRC32 framing from docs/src/technical/ipc-implementation.md
  - Add capability negotiation to existing IPC protocol with CollectionCapabilities message
  - Extend existing DetectionTask and DetectionResult messages for multi-domain support
  - Write integration tests for IPC compatibility with existing sentinelagent client
  - _Requirements: 11.1, 11.2, 12.3_

- [ ] 4.3 Add shared infrastructure components to collector-core - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Integrate existing config::ConfigLoader for hierarchical configuration management
  - Move existing telemetry::TelemetryCollector integration to collector-core runtime
  - Add existing structured logging and health check infrastructure
  - Implement event batching and backpressure handling for multiple event sources
  - Create graceful shutdown coordination for all registered event sources
  - Write unit tests for shared infrastructure integration and lifecycle management
  - _Requirements: 11.1, 11.2, 13.1, 13.2, 13.5_

- [ ] 4.4 Extend protobuf definitions for multi-domain support - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Add CollectionCapabilities message for capability negotiation between collector-core and sentinelagent
  - Extend existing TaskType enum with future network, filesystem, and performance task types
  - Add optional filter fields to DetectionTask for future multi-domain filtering
  - Extend DetectionResult with optional fields for future multi-domain event records
  - Maintain backward compatibility with existing protobuf message structure
  - Write unit tests for protobuf serialization and backward compatibility
  - _Requirements: 12.1, 12.2, 12.3_

- [ ] 5. Implement cross-platform process enumeration in procmond - [#39](https://github.com/EvilBit-Labs/SentinelD/issues/39)

  - Create ProcessCollector trait with platform-specific implementations using sysinfo crate
  - Implement process metadata extraction (PID, PPID, name, executable path, command line)
  - Add enhanced metadata collection when privileges allow (memory usage, CPU%, start time) - procmond should run as root or with CAP_SYS_PTRACE but we need to fail gracefully if we don't have the privileges
  - Handle inaccessible processes gracefully with proper error logging
  - Write unit tests with mock process data and integration tests on real systems
  - _Requirements: 1.1, 1.5_

- [ ] 6. Implement executable integrity verification with SHA-256 hashing - [#40](https://github.com/EvilBit-Labs/SentinelD/issues/40)

  - Create HashComputer trait for cryptographic hashing of executable files
  - Implement SHA-256 hash computation for accessible executable files
  - Handle missing or inaccessible executable files without failing enumeration
  - Store hash_algorithm field ('sha256') with computed hash values
  - Write performance tests to ensure hashing doesn't impact enumeration speed
  - _Requirements: 2.1, 2.2, 2.4_

- [ ] 7. Create ProcessEventSource and refactor procmond to use collector-core

- [ ] 7.1 Create ProcessEventSource wrapping existing ProcessMessageHandler - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Implement ProcessEventSource that wraps existing ProcessMessageHandler from procmond/src/lib.rs
  - Reuse existing enumerate_processes() and convert_process_to_record() logic
  - Integrate with existing database::DatabaseManager and storage layer
  - Preserve existing sysinfo-based process enumeration and hash computation
  - Add CollectionEvent::Process generation from existing ProtoProcessRecord
  - Write unit tests for ProcessEventSource integration with existing components
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 7.2 Refactor procmond to use collector-core framework - [#76](https://github.com/EvilBit-Labs/SentinelD/issues/76)

  - Refactor procmond main.rs to use collector-core Collector instead of direct IPC server
  - Preserve existing CLI parsing, configuration loading, and database initialization
  - Register ProcessEventSource with collector-core runtime
  - Maintain identical behavior and IPC protocol compatibility
  - Ensure existing telemetry and logging integration continues to work
  - Write integration tests to verify identical behavior with existing sentinelagent
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 8. Implement privilege management and security boundaries - [#41](https://github.com/EvilBit-Labs/SentinelD/issues/41)

  - Create PrivilegeManager for platform-specific privilege handling
  - Implement optional enhanced privilege requests (CAP_SYS_PTRACE, SeDebugPrivilege, macOS entitlements)
  - Add immediate privilege dropping after initialization with audit logging
  - Ensure procmond operates with standard user privileges post-initialization
  - Write security tests to verify privilege boundaries and proper dropping
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9. Create tamper-evident audit logging system - [#42](https://github.com/EvilBit-Labs/SentinelD/issues/42)

  - Implement AuditChain with BLAKE3 hashing for cryptographic integrity
  - Create append-only audit ledger with monotonic sequence numbers
  - Add audit entry structure with timestamp, actor, action, payload hash, previous hash, entry hash
  - Implement chain verification function to detect tampering attempts
  - Write cryptographic tests for hash chain integrity and optional Ed25519 signatures
  - _Requirements: 7.1, 7.2, 7.4, 7.5_

- [ ] 10. Implement redb database layer for sentinelagent

- [ ] 10.1 Set up redb database foundation - [#43](https://github.com/EvilBit-Labs/SentinelD/issues/43)

  - Add redb dependency and create database initialization code
  - Configure optimal redb settings for concurrent access and performance
  - Create database connection management with proper error handling
  - Write unit tests for database creation and basic operations
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 10.2 Define database schema and table structures - [#44](https://github.com/EvilBit-Labs/SentinelD/issues/44)

  - Create table definitions for processes, scans, detection_rules, alerts, and alert_deliveries
  - Implement data serialization/deserialization for complex types
  - Add proper indexing strategy for query performance
  - Write unit tests for schema creation and data type handling
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 10.3 Implement transaction handling and error recovery - [#45](https://github.com/EvilBit-Labs/SentinelD/issues/45)

  - Add transaction management for atomic operations
  - Implement proper error recovery and rollback mechanisms
  - Create connection pooling and lifecycle management
  - Write integration tests for transaction scenarios and error conditions
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 10.4 Add database migration system - [#46](https://github.com/EvilBit-Labs/SentinelD/issues/46)

  - Create schema versioning and migration framework
  - Implement upgrade/downgrade migration scripts
  - Add migration validation and rollback capabilities
  - Write integration tests for migration scenarios and version compatibility
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 11. Implement SQL-based detection engine with SQL-to-IPC translation pipeline

- [ ] 11.1 Create SQL validator and collection requirements extractor - [#47](https://github.com/EvilBit-Labs/SentinelD/issues/47)

  - Implement SqlValidator using sqlparser-rs for AST parsing and validation
  - Create CollectionRequirementsExtractor to analyze SQL AST and extract collection needs
  - Add SQL-to-IPC translation logic that converts complex SQL rules into simple protobuf collection tasks
  - Create whitelist of allowed SQL constructs (SELECT statements, approved functions from docs/src/technical/query-pipeline.md)
  - Write unit tests for SQL validation and collection requirements extraction
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 11.2 Implement detection engine with two-phase execution - [#48](https://github.com/EvilBit-Labs/SentinelD/issues/48)

  - Create DetectionEngine struct implementing the two-phase query pipeline from docs/src/technical/query-pipeline.md
  - Add Phase 1: SQL-to-IPC translation that generates protobuf tasks for collector-core components
  - Add Phase 2: SQL rule execution against collected data in redb event store
  - Implement overcollection strategy where collector-core may collect broader data than SQL requires
  - Write unit tests for both phases of the detection pipeline
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 11.3 Add query execution with resource limits and sandboxing - [#49](https://github.com/EvilBit-Labs/SentinelD/issues/49)

  - Implement query timeouts (30 seconds) and memory limits for SQL rule execution
  - Add sandboxed execution environment with read-only database connections
  - Create resource cleanup and cancellation mechanisms for both translation and execution phases
  - Ensure privilege separation where SQL execution never directly touches live processes
  - Write integration tests for resource limit enforcement and timeout scenarios
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 11.4 Create comprehensive security testing for SQL-to-IPC pipeline - [#50](https://github.com/EvilBit-Labs/SentinelD/issues/50)

  - Implement OWASP SQL injection test vector validation for both AST parsing and collection extraction
  - Add fuzzing tests for malformed SQL input in both translation and execution phases
  - Create penetration testing scenarios for SQL validation bypass attempts
  - Write security audit tests for rule execution sandboxing and IPC task generation
  - Test that complex SQL rules are properly translated to simple, secure protobuf tasks
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 12. Create alert generation and management system - [#51](https://github.com/EvilBit-Labs/SentinelD/issues/51)

  - Implement AlertManager for generating structured alerts from detection results
  - Add alert deduplication using configurable keys and time windows
  - Create alert severity classification (low, medium, high, critical)
  - Include affected process details and rule execution metadata in alerts
  - Write unit tests for alert generation logic and deduplication behavior
  - _Requirements: 4.1, 4.2, 4.3, 4.4_

- [ ] 13. Implement multi-channel alert delivery system

- [ ] 13.1 Create AlertSink trait and basic sinks - [#52](https://github.com/EvilBit-Labs/SentinelD/issues/52)

  - Define AlertSink trait with async delivery methods
  - Implement StdoutSink and FileSink for basic alert output
  - Add structured alert formatting (JSON) and human-readable text
  - Write unit tests for basic sink implementations and formatting
  - _Requirements: 5.1, 5.2_

- [ ] 13.2 Implement network-based alert sinks - [#53](https://github.com/EvilBit-Labs/SentinelD/issues/53)

  - Create WebhookSink with HTTP POST delivery and authentication
  - Add SyslogSink for Unix syslog integration
  - Implement EmailSink with SMTP delivery and templates
  - Write integration tests for network sinks with mock endpoints
  - _Requirements: 5.1, 5.2_

- [ ] 13.3 Add parallel delivery and tracking - [#54](https://github.com/EvilBit-Labs/SentinelD/issues/54)

  - Implement parallel alert delivery to multiple sinks without blocking
  - Add delivery tracking with success/failure status recording
  - Create delivery attempt logging and metrics collection
  - Write integration tests for concurrent delivery scenarios
  - _Requirements: 5.1, 5.2_

- [ ] 14. Add alert delivery reliability with circuit breakers and retries - [#55](https://github.com/EvilBit-Labs/SentinelD/issues/55)

  - Implement circuit breaker pattern for each alert sink with configurable failure thresholds
  - Add exponential backoff retry logic with jitter and maximum retry limits
  - Create dead letter queue for permanently failed alert deliveries
  - Implement delivery success rate tracking and metrics collection
  - Write chaos testing scenarios for network failures and endpoint unavailability
  - _Requirements: 5.2, 5.3, 5.4, 5.5_

- [ ] 15. Implement sentinelcli query and management interface - [#56](https://github.com/EvilBit-Labs/SentinelD/issues/56)

  - Create QueryExecutor for safe SQL query execution with parameterization
  - Add multiple output formats (JSON, human-readable table, CSV) with terminal capability detection
  - Implement streaming and pagination for large result sets
  - Add color support with NO_COLOR and TERM=dumb environment variable handling
  - Write CLI integration tests using insta for snapshot testing
  - _Requirements: 8.1, 8.2, 10.5_

- [ ] 16. Add rule management capabilities to sentinelcli - [#57](https://github.com/EvilBit-Labs/SentinelD/issues/57)

  - Implement rule listing, validation, and testing operations
  - Add rule import/export functionality for bundle-based distribution
  - Create rule enable/disable operations with configuration validation
  - Display rule execution statistics and alert history
  - Write integration tests for complete rule management workflows
  - _Requirements: 8.2, 8.3_

- [ ] 17. Implement system health monitoring and diagnostics - [#58](https://github.com/EvilBit-Labs/SentinelD/issues/58)

  - Create HealthChecker for component status verification (database, alert sinks, collector-core components via interprocess crate)
  - Add system health overview with color-coded status indicators
  - Implement performance metrics reporting and resource usage tracking
  - Create configuration validation with detailed error messages and troubleshooting guidance
  - Write health check tests with simulated component failures
  - _Requirements: 8.4, 8.5, 10.2, 10.3_

- [ ] 18. Add offline-first operation and bundle support - [#59](https://github.com/EvilBit-Labs/SentinelD/issues/59)

  - Ensure all core functionality operates without network connectivity
  - Implement graceful degradation for alert delivery when network is unavailable
  - Create bundle-based configuration and rule distribution system
  - Add bundle validation, conflict resolution, and atomic application
  - Write integration tests for airgapped environment operation
  - _Requirements: 9.1, 9.2, 9.4, 9.5_

- [ ] 19. Implement comprehensive observability and metrics - [#60](https://github.com/EvilBit-Labs/SentinelD/issues/60)

  - Add Prometheus-compatible metrics export for collection rate, detection latency, alert delivery
  - Create structured logging with correlation IDs and performance metrics
  - Implement HTTP health endpoints (localhost-only) for external monitoring
  - Add resource utilization metrics (CPU, memory, disk usage) and error rate tracking
  - Write metrics accuracy tests and Prometheus scraping compatibility verification
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [ ] 20. Create comprehensive test suite and quality assurance

- [ ] 20.1 Implement unit test coverage - [#61](https://github.com/EvilBit-Labs/SentinelD/issues/61)

  - Add unit tests for all core functionality targeting >85% code coverage
  - Set up llvm-cov for code coverage measurement and reporting
  - Create test utilities and mock objects for isolated testing
  - Write unit tests for error handling and edge cases
  - _Requirements: All requirements verification_

- [ ] 20.2 Add integration and CLI testing - [#61](https://github.com/EvilBit-Labs/SentinelD/issues/61)

  - Implement integration tests with insta for CLI snapshot testing
  - Add cross-component interaction tests for interprocess-based IPC communication
  - Create end-to-end workflow tests for complete monitoring scenarios
  - Write snapshot tests with insta for CLI output validation
  - _Requirements: All requirements verification_

- [ ] 20.3 Create performance and property-based testing - [#61](https://github.com/EvilBit-Labs/SentinelD/issues/61)

  - Add performance tests with criterion for regression detection
  - Implement property-based tests with proptest for edge case discovery
  - Create load testing scenarios for high-volume process monitoring
  - Write benchmark tests for critical performance paths
  - _Requirements: All requirements verification_

- [ ] 20.4 Set up CI matrix and quality gates - [#61](https://github.com/EvilBit-Labs/SentinelD/issues/61)

  - Configure CI matrix testing across Linux, macOS, Windows
  - Add multiple Rust version testing (stable, beta, MSRV)
  - Set up quality gates with clippy, rustfmt, and security auditing
  - Create automated test reporting and coverage tracking
  - _Requirements: All requirements verification_

- [ ] 21. Add advanced security testing and validation - [#62](https://github.com/EvilBit-Labs/SentinelD/issues/62)

  - Implement comprehensive SQL injection prevention testing with malicious input vectors
  - Add privilege boundary verification tests for all components
  - Create input validation fuzzing with cargo-fuzz for security-critical components
  - Add memory safety verification with Miri and AddressSanitizer where available
  - Write penetration testing scenarios for interprocess IPC protocol and component boundaries
  - _Requirements: 3.5, 6.4, 6.5_

- [ ] 22. Integrate components and implement end-to-end workflows

- [x] 22.1 Wire IPC communication between components - [#63](https://github.com/EvilBit-Labs/SentinelD/issues/63)

  - COMPLETED: Connected collector-core IPC server with sentinelagent IPC client via interprocess crate
  - COMPLETED: Implemented task distribution and result collection workflows with protobuf + CRC32 framing
  - COMPLETED: Added proper error handling and connection management
  - COMPLETED: Created integration tests for cross-platform IPC communication reliability
  - _Requirements: All requirements integration_

- [ ] 22.2 Implement rule translation and execution pipeline - [#63](https://github.com/EvilBit-Labs/SentinelD/issues/63)

  - Create rule translation from SQL to simple protobuf tasks for collector-core components
  - Integrate detection rule execution with interprocess IPC task distribution
  - Add result aggregation and alert generation from detection outcomes
  - Write integration tests for complete detection pipeline
  - _Requirements: All requirements integration_

- [ ] 22.3 Connect alert generation to delivery pipeline - [#63](https://github.com/EvilBit-Labs/SentinelD/issues/63)

  - Wire alert generation from detection results to multi-channel delivery
  - Implement alert deduplication and priority handling
  - Add delivery status tracking and retry coordination
  - Write end-to-end tests for alert generation and delivery
  - _Requirements: All requirements integration_

- [ ] 22.4 Add unified configuration and service management - [#63](https://github.com/EvilBit-Labs/SentinelD/issues/63)

  - Implement configuration management across all three components
  - Add sentinelagent process lifecycle management for collector-core components
  - Create unified logging and health reporting
  - Write integration tests for service startup, shutdown, and configuration changes
  - _Requirements: All requirements integration_
