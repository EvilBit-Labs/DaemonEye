# Implementation Plan

- [x] 1. Set up workspace structure and core library foundations - [#33](https://github.com/EvilBit-Labs/DaemonEye/issues/33)

  - Create Cargo workspace with three binary crates (procmond, daemoneye-agent, daemoneye-cli) and shared library (daemoneye-lib)
  - Configure workspace-level lints: `unsafe_code = "forbid"`, `warnings = "deny"`
  - Set up basic error handling with thiserror for libraries and anyhow for applications
  - Implement structured logging with tracing ecosystem and JSON formatting
  - _Requirements: 6.4, 10.1_

- [x] 2. Implement core data models and serialization - [#34](https://github.com/EvilBit-Labs/DaemonEye/issues/34)

  - Create ProcessRecord struct with comprehensive metadata fields (PID, PPID, name, executable_path, etc.)
  - Implement Alert struct with severity levels, deduplication keys, and context information
  - Create DetectionRule struct with SQL query, metadata, and versioning support
  - Add serde serialization/deserialization for all data structures
  - Write unit tests for data model validation and serialization
  - _Requirements: 1.3, 4.1, 4.3_

- [x] 3. Implement IPC client infrastructure for daemoneye-agent

- [x] 3.1 Define protobuf schema for IPC messages - [#35](https://github.com/EvilBit-Labs/DaemonEye/issues/35)

  - Create .proto files for DetectionTask, DetectionResult, ProcessFilter, and HashCheck message types
  - Add build.rs configuration for protobuf code generation
  - Generate Rust code from protobuf definitions and verify compilation
  - _Requirements: 3.1, 3.2_

- [x] 3.2 Implement IPC client infrastructure with interprocess crate - [#36](https://github.com/EvilBit-Labs/DaemonEye/issues/36)

  - Add interprocess crate dependency with tokio feature for cross-platform local sockets
  - Create IpcCodec with protobuf framing and CRC32 validation
  - Implement IpcClientManager with automatic reconnection logic and exponential backoff
  - Add connection lifecycle management (connect, disconnect, health checks)
  - Implement message timeout, size limits, and comprehensive error handling
  - Integrate with daemoneye-agent main loop for task distribution and result collection
  - Write comprehensive unit tests for client connection scenarios and cross-platform compatibility
  - _Requirements: 3.1, 3.2_

- [x] 3.3 Add advanced IPC client features - [#37](https://github.com/EvilBit-Labs/DaemonEye/issues/37)

  - Add connection pooling for multiple collector-core components
  - Implement load balancing and failover between multiple collectors
  - Add metrics collection for IPC client performance monitoring
  - Enhance error recovery with circuit breaker patterns
  - _Requirements: 3.1, 3.2_

- [x] 3.4 Add capability negotiation to IPC client - [#38](https://github.com/EvilBit-Labs/DaemonEye/issues/38)

  - Implement capability discovery and negotiation with collector-core components
  - Add support for multi-domain task routing based on collector capabilities
  - Integrate with detection engine for SQL-to-IPC task translation
  - Handle capability changes and component reconnection scenarios
  - Write integration tests for capability negotiation workflows
  - _Requirements: 3.1, 3.2, 11.1, 11.2_

- [x] 3.5 Add comprehensive IPC client testing and validation - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Create integration tests for daemoneye-agent IPC client connecting to collector-core servers
  - Add cross-platform tests (Linux, macOS, Windows) for local socket functionality
  - Implement integration tests for task distribution from daemoneye-agent to collector components
  - Add property-based tests for codec robustness with malformed inputs
  - Create performance benchmarks ensuring no regression in message throughput or latency
  - Write security validation tests for connection authentication and message integrity
  - _Requirements: 3.1, 3.2_

- [x] 4. Implement collector-core framework for extensible monitoring architecture

- [x] 4.1 Create collector-core library foundation - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Create new collector-core workspace member crate with public API exports
  - Define universal EventSource trait with start/stop lifecycle and capability reporting
  - Implement Collector struct for runtime management and event source registration
  - Create extensible CollectionEvent enum supporting process, network, filesystem, and performance domains
  - Add SourceCaps bitflags for capability negotiation (PROCESS, NETWORK, FILESYSTEM, PERFORMANCE, REALTIME, KERNEL_LEVEL, SYSTEM_WIDE)
  - Write unit tests for EventSource trait and Collector registration
  - _Requirements: 11.1, 11.2, 11.5_

- [x] 4.2 Implement IPC server integration in collector-core - [#77](https://github.com/EvilBit-Labs/DaemonEye/issues/77)

  - Implement CollectorIpcServer with full functionality
  - Integrate InterprocessServer and IpcCodec with collector-core event handling
  - Add capability advertisement to IPC protocol with CollectionCapabilities message
  - Handle incoming DetectionTask messages from daemoneye-agent and route to appropriate EventSources
  - Send DetectionResult messages back to daemoneye-agent with collected data
  - Add task validation against collector capabilities
  - Write comprehensive unit tests for IPC server functionality
  - _Requirements: 11.1, 11.2, 12.3_

- [x] 4.3 Add shared infrastructure components to collector-core - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Integrate existing config::ConfigLoader for hierarchical configuration management
  - Move existing telemetry::TelemetryCollector integration to collector-core runtime
  - Add existing structured logging and health check infrastructure
  - Implement event batching and backpressure handling for multiple event sources
  - Create graceful shutdown coordination for all registered event sources
  - Write unit tests for shared infrastructure integration and lifecycle management
  - _Requirements: 11.1, 11.2, 13.1, 13.2, 13.5_

- [x] 4.4 Extend protobuf definitions for multi-domain support - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Add CollectionCapabilities message for capability negotiation between collector-core and daemoneye-agent
  - Extend existing TaskType enum with future network, filesystem, and performance task types
  - Add optional filter fields to DetectionTask for future multi-domain filtering
  - Extend DetectionResult with optional fields for future multi-domain event records
  - Maintain backward compatibility with existing protobuf message structure
  - Write unit tests for protobuf serialization and backward compatibility
  - _Requirements: 12.1, 12.2, 12.3_

- [x] 4.5 Complete collector-core testing suite - [#78](https://github.com/EvilBit-Labs/DaemonEye/issues/78)

  - Write integration tests for multiple concurrent EventSource registration and lifecycle management
  - Add criterion benchmarks for event batching, backpressure handling, and graceful shutdown coordination
  - Create property-based tests for CollectionEvent serialization and capability negotiation
  - Implement chaos testing for EventSource failure scenarios and recovery behavior
  - Add performance benchmarks for collector-core runtime overhead and event throughput
  - Write security tests for EventSource isolation and capability enforcement
  - Create compatibility tests ensuring collector-core works with existing procmond and daemoneye-agent
  - _Requirements: 11.1, 11.2, 12.1, 12.2, 13.1, 13.2, 13.5_

- [ ] 5. Implement cross-platform process enumeration in procmond

- [x] 5.1a Create ProcessMessageHandler with sysinfo-based process enumeration - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Implement ProcessMessageHandler with basic sysinfo-based process enumeration
  - Add process metadata extraction (PID, PPID, name, executable path, command line, CPU, memory)
  - Create convert_process_to_record method for ProtoProcessRecord conversion
  - Add basic unit tests for ProcessMessageHandler functionality
  - _Requirements: 1.1, 1.5_

- [x] 5.1b Complete ProcessEventSource integration to collector-core standards - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Refactor ProcessEventSource to fully comply with collector-core framework standards
  - Implement proper async_trait EventSource with start/stop lifecycle and capability reporting
  - Add proper shutdown signal handling with cooperative cancellation and timeout enforcement
  - Implement event batching and backpressure handling for multiple event sources
  - Add comprehensive error handling with graceful degradation and structured logging
  - Create proper health check implementation with detailed diagnostics
  - Write comprehensive unit tests for EventSource trait implementation and lifecycle management
  - Add integration tests for ProcessEventSource with collector-core runtime
  - _Requirements: 1.1, 1.5, 11.1, 11.2, 11.5_

- [x] 5.2 Create ProcessCollector trait and refactor for extensibility - [#80](https://github.com/EvilBit-Labs/DaemonEye/issues/80)

  - Define ProcessCollector trait with platform-agnostic interface for process enumeration
  - Refactor ProcessMessageHandler to use ProcessCollector trait
  - Create SysinfoProcessCollector as default cross-platform implementation
  - Add proper error handling for inaccessible processes with structured logging
  - Write comprehensive unit tests with mock process data
  - _Requirements: 1.1, 1.5_

- [x] 5.3 Implement Linux-specific optimizations - [#81](https://github.com/EvilBit-Labs/DaemonEye/issues/81)

  - Create LinuxProcessCollector with direct /proc filesystem access for enhanced performance
  - Add CAP_SYS_PTRACE capability detection and privilege management
  - Implement enhanced metadata collection (memory maps, file descriptors, network connections)
  - Add support for process namespaces and container detection
  - Handle permission denied gracefully for restricted processes
  - Write Linux-specific integration tests
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [x] 5.4 Implement macOS-specific optimizations using third-party crates - [#82](https://github.com/EvilBit-Labs/DaemonEye/issues/82)

  - Refactor macOSProcessCollector to use well-maintained third-party crates instead of direct libc
  - Replace libproc/sysctl with enhanced sysinfo + security-framework + core-foundation + mac-sys-info (procfs is Linux-only)
  - Implement proper entitlements detection using security-framework crate for accurate code signing and bundle info
  - Add System Integrity Protection (SIP) awareness using mac-sys-info for system information
  - Handle sandboxed process restrictions gracefully with security-framework entitlements parsing
  - Maintain all existing capabilities while improving safety and accuracy
  - Write comprehensive integration tests for third-party crate approach
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 5.5 Implement Windows-specific optimizations using third-party crates - [#83](https://github.com/EvilBit-Labs/DaemonEye/issues/83)

  - Create WindowsProcessCollector using well-maintained third-party crates instead of direct Windows API calls
  - Use enhanced sysinfo + windows-rs + winapi-safe + psutil-rs for comprehensive Windows process monitoring
  - Implement Windows-specific security features: SeDebugPrivilege detection, process tokens, security contexts
  - Add support for Windows process attributes: protected processes, system processes, UAC elevation status
  - Implement Windows service detection and management using windows-service crate
  - Add Windows-specific metadata: process integrity levels, session isolation, virtualization status
  - Handle Windows Defender and antivirus process restrictions gracefully
  - Add support for Windows containers (Hyper-V containers, Windows Server containers)
  - Implement Windows-specific performance counters using perfmon-rs or similar
  - Write comprehensive Windows-specific integration tests covering all Windows versions
  - Maintain all existing capabilities while improving safety and accuracy with third-party crates
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 5.6 Add comprehensive cross-platform testing - [#84](https://github.com/EvilBit-Labs/DaemonEye/issues/84)

  - Create cross-platform integration tests for all ProcessCollector implementations
  - Add performance benchmarks comparing platform-specific vs sysinfo implementations
  - Implement privilege escalation/dropping tests for all platforms
  - Add criterion benchmarks with high process counts (10,000+ processes)
  - Create compatibility tests for different OS versions and configurations
  - Write property-based tests for process enumeration edge cases
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 6. Implement executable integrity verification with SHA-256 hashing - [#40](https://github.com/EvilBit-Labs/DaemonEye/issues/40)

  - Create HashComputer trait for cryptographic hashing of executable files
  - Implement SHA-256 hash computation for accessible executable files in ProcessMessageHandler
  - Handle missing or inaccessible executable files without failing enumeration
  - Store hash_algorithm field ('sha256') with computed hash values in ProtoProcessRecord
  - Write criterion benchmarks to ensure hashing doesn't impact enumeration speed
  - _Requirements: 2.1, 2.2, 2.4_

- [ ] 7. Create ProcessEventSource and refactor procmond to use collector-core

- [x] 7.1 Create ProcessEventSource wrapping existing ProcessMessageHandler - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Implement ProcessEventSource that wraps existing ProcessMessageHandler from procmond/src/lib.rs
  - Reuse existing enumerate_processes() and convert_process_to_record() logic
  - Integrate with existing database::DatabaseManager and storage layer
  - Preserve existing sysinfo-based process enumeration and hash computation
  - Add CollectionEvent::Process generation from existing ProtoProcessRecord
  - Write unit tests for ProcessEventSource integration with existing components
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 7.2 Refactor procmond to use collector-core framework - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Refactor procmond main.rs to use collector-core Collector instead of direct IPC server
  - Preserve existing CLI parsing, configuration loading, and database initialization
  - Register ProcessEventSource with collector-core runtime
  - Maintain identical behavior and IPC protocol compatibility
  - Ensure existing telemetry and logging integration continues to work
  - Write integration tests to verify identical behavior with existing daemoneye-agent
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 8. Implement privilege management and security boundaries - [#41](https://github.com/EvilBit-Labs/DaemonEye/issues/41)

  - Create PrivilegeManager for platform-specific privilege handling
  - Implement optional enhanced privilege requests (CAP_SYS_PTRACE, SeDebugPrivilege, macOS entitlements)
  - Add immediate privilege dropping after initialization with audit logging
  - Ensure procmond operates with standard user privileges post-initialization
  - Write security tests to verify privilege boundaries and proper dropping
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9. Implement daemon/service functionality for daemoneye-agent with process lifecycle management

- [ ] 9.1 Add daemon/service dependencies and cross-platform service infrastructure

  - Add `daemonize` crate dependency for Unix-like systems (Linux, macOS) daemon functionality
  - Add `windows-service` crate dependency for Windows service integration
  - Create `ServiceManager` trait for cross-platform service lifecycle management
  - Implement platform-specific service managers: `UnixServiceManager` and `WindowsServiceManager`
  - Add service configuration structure with startup options, working directory, and user context
  - Write unit tests for service manager trait implementations and configuration validation
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.2 Implement Unix daemon functionality with proper privilege management

  - Create `UnixDaemon` implementation using daemonize crate for Linux and macOS
  - Add proper daemon initialization: fork, setsid, chdir, umask, file descriptor management
  - Implement PID file management with lock file creation and cleanup
  - Add signal handling for SIGTERM, SIGINT, SIGHUP (config reload), and SIGUSR1 (status)
  - Create privilege dropping after daemon initialization with audit logging
  - Implement proper daemon shutdown with graceful collector process termination
  - Write integration tests for daemon lifecycle and signal handling
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.3 Implement Windows service functionality with SCM integration

  - Create `WindowsService` implementation using windows-service crate
  - Add Windows Service Control Manager (SCM) integration with proper service registration
  - Implement service control handlers for start, stop, pause, continue, and shutdown
  - Add Windows event log integration for service status and error reporting
  - Create service installer/uninstaller functionality with proper registry entries
  - Implement service recovery options and failure handling policies
  - Write integration tests for Windows service lifecycle and SCM interaction
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.4 Add collector process lifecycle management and supervision

  - Create `ProcessSupervisor` for managing collector process lifecycles (procmond, specialty collectors)
  - Implement collector process spawning with proper environment setup and privilege management
  - Add collector process health monitoring with heartbeat checks and restart policies
  - Create collector process graceful shutdown coordination with timeout enforcement
  - Implement collector process crash detection and automatic restart with backoff policies
  - Add collector process resource monitoring and limit enforcement
  - Write integration tests for process supervision scenarios and failure recovery
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 9.5 Integrate service functionality with existing daemoneye-agent architecture

  - Modify daemoneye-agent main.rs to support both interactive and service/daemon modes
  - Add CLI flags for service operations: --install, --uninstall, --start, --stop, --status
  - Integrate service manager with existing configuration system and database initialization
  - Preserve existing IPC client functionality and detection engine integration
  - Add service-specific logging configuration with file rotation and retention policies
  - Create service configuration templates for systemd, launchd, and Windows services
  - Write integration tests ensuring service mode maintains identical functionality to interactive mode
  - _Requirements: 6.1, 6.2, 6.4, 6.5, 11.1, 11.2_

- [ ] 9.6 Add service monitoring and health reporting capabilities

  - Implement service health endpoints for external monitoring systems
  - Add service status reporting with component health aggregation
  - Create service metrics export for monitoring collector process status and performance
  - Implement service configuration validation and startup diagnostics
  - Add service log aggregation from all managed collector processes
  - Create service recovery actions for common failure scenarios
  - Write monitoring integration tests with simulated component failures and recovery validation
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [ ] 9.7 Create service deployment and configuration management

  - Create systemd service unit files for Linux with proper dependencies and security settings
  - Add launchd plist files for macOS with appropriate permissions and startup configuration
  - Create Windows service installer with proper registry entries and security descriptors
  - Implement service configuration validation and deployment verification
  - Add service update and migration procedures for configuration changes
  - Create service deployment documentation with platform-specific installation guides
  - Write deployment automation scripts for common service management scenarios
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.8 Add comprehensive service testing and validation

  - Create integration tests for service installation, startup, and shutdown on all platforms
  - Add service failure scenario testing with collector process crashes and recovery
  - Implement service upgrade testing with configuration migration and data preservation
  - Create service security testing for privilege boundaries and access controls
  - Add service performance testing under high load with multiple collector processes
  - Write service compatibility tests across different OS versions and configurations
  - Create service chaos testing for various failure modes and recovery validation
  - _Requirements: All requirements verification_

- [ ] 10. Create tamper-evident audit logging system - [#42](https://github.com/EvilBit-Labs/DaemonEye/issues/42)

  - Implement AuditChain with BLAKE3 hashing for cryptographic integrity using rs_merkle crate
  - Create append-only audit ledger with monotonic sequence numbers in separate redb database
  - Add audit entry structure with timestamp, actor, action, payload hash, previous hash, entry hash
  - Implement chain verification function to detect tampering attempts
  - Add periodic checkpoints with optional Ed25519 signatures for external verification
  - Write cryptographic tests for hash chain integrity and inclusion proofs
  - _Requirements: 7.1, 7.2, 7.4, 7.5_

- [ ] 11. Implement redb database layer for daemoneye-agent

- [x] 11.1 Create basic DatabaseManager structure and error handling - [#43](https://github.com/EvilBit-Labs/DaemonEye/issues/43)

  - Add redb dependency and create basic DatabaseManager structure
  - Create database initialization with table definitions using `Vec<u8>` placeholders
  - Add comprehensive error handling with platform-agnostic error messages
  - Write basic unit tests for database creation and error scenarios
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 11.2 Implement actual redb database operations - [#44](https://github.com/EvilBit-Labs/DaemonEye/issues/44)

  - Configure optimal redb settings for concurrent access and performance
  - Replace all placeholder TODO implementations with actual redb operations
  - Implement proper data serialization/deserialization for ProcessRecord, DetectionRule, Alert types
  - Add proper indexing strategy for query performance
  - Implement transaction handling for atomic operations
  - Write comprehensive integration tests for all database operations
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 11.3 Add database migration system and advanced features - [#45](https://github.com/EvilBit-Labs/DaemonEye/issues/45)

  - Add database migration system with schema versioning
  - Implement connection pooling and lifecycle management
  - Add proper error recovery and rollback mechanisms
  - Create data cleanup and retention policy implementation
  - Write integration tests for migration scenarios and version compatibility
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 12. Integrate SQL-to-IPC detection engine (see dedicated spec: sql-to-ipc-detection-engine)

- [x] 12.1 Create basic DetectionEngine structure and rule management - [#47](https://github.com/EvilBit-Labs/DaemonEye/issues/47)

  - Create DetectionEngine struct with rule loading and execution capabilities
  - Implement basic SQL validation using sqlparser-rs for AST parsing
  - Create rule management functions (load, remove, enable/disable)
  - Write basic unit tests for detection engine structure and rule management
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 12.2 Replace existing detection engine with SQL-to-IPC implementation

  - **Note**: This task is now covered by the dedicated sql-to-ipc-detection-engine spec
  - Integrate SQL-to-IPC engine as replacement for placeholder detection logic
  - Implement SQL parsing, pushdown planning, and two-layer execution architecture
  - Add schema registry integration for collector capability negotiation
  - Create reactive pipeline orchestrator for cascading analysis and auto-correlation
  - Integrate specialty collectors (YARA, PE analysis, network analysis) with SQL queries
  - **Reference**: See #[[file:.kiro/specs/sql-to-ipc-detection-engine/tasks.md]] for complete implementation plan
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 12.3 Integrate SQL-to-IPC engine with existing daemoneye-agent infrastructure

  - Create integration wrapper that implements existing DetectionEngine trait
  - Replace placeholder rule execution with SQL-to-IPC query planning and execution
  - Integrate with existing IPC client infrastructure for collector communication
  - Maintain backward compatibility with existing rule file formats and configuration
  - Add SQL-to-IPC engine configuration to existing daemoneye-agent config system
  - Write integration tests comparing old vs new detection engine behavior
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 12.4 Extend collector-core framework for SQL-to-IPC task handling

  - Modify EventSource trait to accept DetectionTask with supplemental rule data
  - Add capability advertisement methods for schema registry integration
  - Update existing ProcessEventSource to handle SQL-to-IPC generated tasks
  - Integrate schema registry with collector-core startup and capability advertisement
  - Create task validation and execution logic in collector-core runtime
  - Write integration tests for schema registry with existing procmond and collector-core
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 12.5 Validate SQL-to-IPC integration with comprehensive testing

  - Create end-to-end integration tests for SQL rules with multi-collector queries
  - Add performance validation comparing SQL-to-IPC vs placeholder detection
  - Implement security testing for SQL parsing and pushdown optimization
  - Create integration tests for reactive pipeline orchestration and auto-correlation
  - Write compatibility tests ensuring existing detection workflows continue to function
  - Add performance benchmarks for SQL parsing, planning, and execution phases
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [x] 13. Create alert generation and management system - [#51](https://github.com/EvilBit-Labs/DaemonEye/issues/51)

  - Implement AlertManager for generating structured alerts from detection results
  - Add alert deduplication using configurable keys and time windows
  - Create alert severity classification (low, medium, high, critical)
  - Include affected process details and rule execution metadata in alerts
  - Write unit tests for alert generation logic and deduplication behavior
  - _Requirements: 4.1, 4.2, 4.3, 4.4_

- [ ] 14. Implement multi-channel alert delivery system

- [x] 14.1 Create AlertSink trait and basic sinks - [#52](https://github.com/EvilBit-Labs/DaemonEye/issues/52)

  - Define AlertSink trait with async delivery methods
  - Implement StdoutSink and FileSink for basic alert output
  - Add structured alert formatting (JSON, YAML, CSV) and human-readable text
  - Create AlertSinkFactory for creating sinks from configuration
  - Write comprehensive unit tests for basic sink implementations and formatting
  - _Requirements: 5.1, 5.2_

- [ ] 14.2 Implement network-based alert sinks - [#53](https://github.com/EvilBit-Labs/DaemonEye/issues/53)

  - Create WebhookSink with HTTP POST delivery and authentication
  - Add SyslogSink for Unix syslog integration
  - Implement EmailSink with SMTP delivery and templates
  - Write integration tests for network sinks with mock endpoints
  - _Requirements: 5.1, 5.2_

- [x] 14.3 Add parallel delivery and tracking - [#54](https://github.com/EvilBit-Labs/DaemonEye/issues/54)

  - Implement parallel alert delivery to multiple sinks without blocking
  - Add delivery tracking with success/failure status recording
  - Create delivery attempt logging and metrics collection
  - Add health monitoring for all alert sinks
  - Write integration tests for concurrent delivery scenarios
  - _Requirements: 5.1, 5.2_

- [ ] 15. Add alert delivery reliability with circuit breakers and retries - [#55](https://github.com/EvilBit-Labs/DaemonEye/issues/55)

  - Implement circuit breaker pattern for each alert sink with configurable failure thresholds
  - Add exponential backoff retry logic with jitter and maximum retry limits
  - Create dead letter queue for permanently failed alert deliveries
  - Implement delivery success rate tracking and metrics collection
  - Write chaos testing scenarios for network failures and endpoint unavailability
  - _Requirements: 5.2, 5.3, 5.4, 5.5_

- [ ] 15.1 Complete collector-core IPC server integration - [#78](https://github.com/EvilBit-Labs/DaemonEye/issues/78)

  - Implement CollectorIpcServer in collector-core/src/ipc.rs with full functionality
  - Add capability negotiation and task routing between collector-core and daemoneye-agent
  - Integrate IPC server with collector runtime for event source management
  - Add proper error handling and connection management
  - Write integration tests for IPC server functionality
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 15.2 Complete IPC client implementation in daemoneye-agent - [#77](https://github.com/EvilBit-Labs/DaemonEye/issues/77)

  - Complete IpcClientManager implementation with proper error handling and reconnection logic
  - Add capability negotiation between daemoneye-agent and collector-core components
  - Implement task distribution and result collection workflows
  - Add comprehensive integration tests for IPC client functionality
  - Ensure compatibility with existing procmond ProcessMessageHandler
  - _Requirements: 3.1, 3.2, 11.1, 11.2_

- [ ] 16. Implement daemoneye-cli basic infrastructure - [#56](https://github.com/EvilBit-Labs/DaemonEye/issues/56)

- [x] 16.1 Create basic CLI structure and database stats - [#56](https://github.com/EvilBit-Labs/DaemonEye/issues/56)

  - Implement clap-based CLI with basic options (--database, --format, --help, --version)
  - Add basic database statistics display functionality
  - Implement multiple output formats (JSON, human-readable)
  - Add basic telemetry integration and health checking
  - _Requirements: 8.1, 8.2, 10.5_

- [ ] 16.2 Add comprehensive CLI testing and error handling - [#79](https://github.com/EvilBit-Labs/DaemonEye/issues/79)

  - Write comprehensive CLI tests using insta for snapshot testing
  - Add proper error handling with helpful error messages
  - Implement configuration file support and validation
  - Add shell completion support for bash, zsh, fish, and PowerShell
  - _Requirements: 8.1, 8.2, 10.5_

- [ ] 16.3 Extend daemoneye-cli with advanced query capabilities - [#80](https://github.com/EvilBit-Labs/DaemonEye/issues/80)

  - Create IPC client to communicate with daemoneye-agent using existing interprocess crate infrastructure
  - Add query command with subcommands: interactive shell, single query execution, history, explain, validate
  - Add streaming and pagination support for large result sets via IPC protocol
  - Create interactive query shell with syntax highlighting, auto-completion, and command history
  - Implement query parameter binding and prepared statement support through daemoneye-agent
  - Add color support with NO_COLOR and TERM=dumb environment variable handling
  - Create query request/response protobuf messages extending existing IPC protocol
  - Write CLI integration tests using insta for snapshot testing of all output formats
  - _Requirements: 8.1, 8.2, 10.5_

- [ ] 17. Implement daemoneye-cli management commands (rules, alerts, health, data, config, service) - [#57](https://github.com/EvilBit-Labs/DaemonEye/issues/57)

  - Add rules command with subcommands: list, show, validate, test, enable/disable, create, edit, delete, import/export, stats, pack management
  - Implement alerts command with subcommands: list, show, acknowledge, close, reopen, stats, export with filtering by severity/rule/status
  - Create health command with subcommands: overall status, component-specific checks, metrics, config validation, connectivity testing, logs, diagnostics, repair
  - Add data command with subcommands: export (processes/audit), stats, vacuum, integrity-check, cleanup with retention policies
  - Implement config command with subcommands: show, validate, set, reset with hierarchical configuration management
  - Create service command with subcommands: status, start/stop/restart, logs with component filtering and follow mode
  - Add shell completion support for bash, zsh, fish, and PowerShell
  - Implement configuration file support with YAML format and user/system config hierarchy
  - Add comprehensive error handling with helpful suggestions and troubleshooting guidance
  - Write integration tests for all command workflows and error scenarios using insta snapshots
  - _Requirements: 8.2, 8.3, 8.4, 8.5_

- [ ] 18. Implement system health monitoring and diagnostics - [#58](https://github.com/EvilBit-Labs/DaemonEye/issues/58)

  - Create HealthChecker for component status verification (database, alert sinks, collector-core components via interprocess crate)
  - Add system health overview with color-coded status indicators
  - Implement performance metrics reporting and resource usage tracking
  - Create configuration validation with detailed error messages and troubleshooting guidance
  - Write health check tests with simulated component failures
  - _Requirements: 8.4, 8.5, 10.2, 10.3_

- [ ] 19. Add offline-first operation and bundle support - [#59](https://github.com/EvilBit-Labs/DaemonEye/issues/59)

  - Ensure all core functionality operates without network connectivity
  - Implement graceful degradation for alert delivery when network is unavailable
  - Create bundle-based configuration and rule distribution system
  - Add bundle validation, conflict resolution, and atomic application
  - Write integration tests for airgapped environment operation
  - _Requirements: 9.1, 9.2, 9.4, 9.5_

- [ ] 20. Implement comprehensive observability and metrics - [#60](https://github.com/EvilBit-Labs/DaemonEye/issues/60)

  - Add Prometheus-compatible metrics export for collection rate, detection latency, alert delivery
  - Create structured logging with correlation IDs and performance metrics
  - Implement HTTP health endpoints (localhost-only) for external monitoring
  - Add resource utilization metrics (CPU, memory, disk usage) and error rate tracking
  - Write metrics accuracy tests and Prometheus scraping compatibility verification
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [ ] 21. Create comprehensive test suite and quality assurance

- [ ] 21.1 Implement unit test coverage - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Add unit tests for all core functionality targeting >85% code coverage
  - Set up llvm-cov for code coverage measurement and reporting
  - Create test utilities and mock objects for isolated testing
  - Write unit tests for error handling and edge cases
  - _Requirements: All requirements verification_

- [ ] 21.2 Add integration and CLI testing - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Implement integration tests with insta for CLI snapshot testing
  - Add cross-component interaction tests for interprocess-based IPC communication
  - Create end-to-end workflow tests for complete monitoring scenarios
  - Write snapshot tests with insta for CLI output validation
  - _Requirements: All requirements verification_

- [ ] 21.3 Create performance and property-based testing - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Add performance tests with criterion for regression detection on critical paths (process enumeration, SQL execution, IPC throughput)
  - Implement property-based tests with proptest for edge case discovery in data models, SQL parsing, and collector-core event handling
  - Create criterion benchmarks for high-volume process monitoring (10,000+ processes, sustained monitoring)
  - Write benchmark tests for collector-core framework overhead and event source registration/deregistration
  - Add memory usage benchmarks for long-running monitoring scenarios and database growth patterns
  - Create criterion benchmarks for alert delivery under high-volume detection scenarios
  - _Requirements: All requirements verification_

- [ ] 21.4 Set up CI matrix and quality gates - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Set up GitHub Actions CI matrix for Linux, macOS, Windows with multiple Rust versions (stable, beta, MSRV)
  - Add automated quality gates: fmt-check, clippy strict, comprehensive test suite
  - Implement performance regression detection with criterion benchmarks
  - Add dependency scanning, SLSA provenance (Enterprise), and security validation
  - Create automated release pipeline with platform-specific packages and code signing
  - _Requirements: All requirements verification_

- [ ] 21. Comprehensive stress testing and load validation

- [ ] 21.1 Implement collector-core stress testing suite

  - Create stress tests for event batching under extreme load (100,000+ events/second)
  - Add stress tests for backpressure handling with multiple blocked event sources
  - Implement stress tests for graceful shutdown coordination under heavy load
  - Create memory pressure tests for event source registration/deregistration cycles
  - Add concurrent stress tests for multiple EventSource instances with resource contention
  - Write endurance tests for 24+ hour continuous operation under load
  - _Requirements: 11.1, 11.2, 12.1, 12.2, 13.1, 13.2, 13.5_

- [ ] 21.2 Process enumeration stress testing

  - Create stress tests with extremely high process counts (50,000+ processes)
  - Add stress tests for rapid process creation/termination scenarios
  - Implement memory pressure tests for process enumeration with limited resources
  - Create concurrent enumeration stress tests with multiple collectors
  - Add privilege boundary stress tests under resource exhaustion
  - Write platform-specific stress tests for OS-level resource limits
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 21.3 Database and storage stress testing

  - Create stress tests for redb database under extreme write loads (10,000+ records/second)
  - Add stress tests for concurrent read/write operations with resource contention
  - Implement stress tests for database growth and retention policy enforcement
  - Create memory pressure tests for large dataset queries and aggregations
  - Add stress tests for database corruption recovery and integrity validation
  - Write endurance tests for long-term database stability under continuous load
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 21.4 Alert delivery stress testing

  - Create stress tests for alert delivery under high-volume detection scenarios (1,000+ alerts/minute)
  - Add stress tests for multiple alert sink failures and recovery scenarios
  - Implement stress tests for network partition and connectivity issues
  - Create memory pressure tests for alert queuing and dead letter queue management
  - Add stress tests for circuit breaker behavior under sustained failures
  - Write endurance tests for alert delivery reliability over extended periods
  - _Requirements: 5.2, 5.3, 5.4, 5.5_

- [ ] 21.5 IPC communication stress testing

  - Create stress tests for IPC communication under extreme message loads
  - Add stress tests for connection failures and automatic reconnection scenarios
  - Implement stress tests for message serialization/deserialization under load
  - Create memory pressure tests for IPC buffer management and backpressure
  - Add stress tests for capability negotiation under rapid collector restarts
  - Write endurance tests for IPC stability over extended operation periods
  - _Requirements: 3.1, 3.2, 11.1, 11.2_

- [ ] 21.6 System-wide integration stress testing

  - Create end-to-end stress tests for complete monitoring workflows under extreme load

  - Add stress tests for system resource exhaustion and graceful degradation

  - Implement stress tests for configuration changes and hot-reloading under load

  - Create chaos engineering tests for random component failures and recovery

  - Add stress tests for security boundary enforcement under resource pressure

  - Write comprehensive load tests simulating real-world deployment scenarios

  - Requirements: All requirements (verification_daemoneye/issues/61)

  - Configure comprehensive CI matrix testing aligned with AGENTS.md OS Support Matrix

  - Add primary platform testing: Ubuntu 20.04+ LTS, RHEL/CentOS 8+, Debian 11+ LTS, macOS 14.0+ (Sonoma), Windows 10+/11/Server 2019+/Server 2022

  - Include architecture matrix: x86_64 and ARM64 for all primary platforms

  - Add secondary platform testing: Alpine 3.16+, Amazon Linux 2+, Ubuntu 18.04, RHEL 7, macOS 12.0+ (Monterey), FreeBSD 13.0+

  - Configure multiple Rust version testing (stable, beta, MSRV 1.70+) across primary platforms

  - Set up quality gates with clippy, rustfmt, security auditing (cargo audit, cargo deny), and overflow-checks validation

  - Create automated test reporting and coverage tracking with llvm-cov

  - Add container-based testing for Alpine and Amazon Linux deployments

  - Configure cross-compilation testing for ARM64 targets

  - _Requirements: All requirements verification_

- [ ] 22. Add advanced security testing and validation - [#62](https://github.com/EvilBit-Labs/DaemonEye/issues/62)

  - Implement comprehensive SQL injection prevention testing with OWASP test vectors and malicious input fuzzing
  - Add privilege boundary verification tests for all components with capability dropping validation
  - Create input validation fuzzing with cargo-fuzz for protobuf parsing, SQL validation, configuration loading, and collector-core event processing
  - Add memory safety verification with Miri and AddressSanitizer for unsafe code boundaries
  - Write penetration testing scenarios for IPC protocol, socket permissions, and component isolation
  - Create audit chain integrity testing with tampering detection and cryptographic verification
  - Add collector-core security testing for event source isolation and capability enforcement
  - Implement chaos engineering tests for component failure scenarios and recovery behavior
  - _Requirements: 3.5, 6.4, 6.5_

- [ ] 23. Integrate components and implement end-to-end workflows

- [ ] 23.1 Wire IPC communication between components via collector-core - [#63](https://github.com/EvilBit-Labs/DaemonEye/issues/63)

  - Integrate daemoneye-agent IPC client with collector-core framework from Task 4
  - Verify task distribution and result collection workflows work through collector-core runtime
  - Ensure existing protobuf + CRC32 framing is preserved through collector-core integration
  - Test cross-component communication with refactored procmond using collector-core from Task 7
  - Write integration tests for complete collector-core mediated IPC communication
  - _Requirements: All requirements integration_

- [ ] 23.2 Implement rule translation and execution pipeline - [#63](https://github.com/EvilBit-Labs/DaemonEye/issues/63)

  - Integrate SQL-to-IPC translation pipeline from Task 11 with collector-core framework from Task 4
  - Verify detection rule execution works with collector-core event sources and IPC distribution
  - Connect result aggregation from collector-core events to alert generation pipeline
  - Test complete detection pipeline with ProcessEventSource from Task 7 and collector-core runtime
  - Write integration tests for end-to-end rule execution through collector-core architecture
  - _Requirements: All requirements integration_

- [ ] 23.3 Connect alert generation to delivery pipeline - [#63](https://github.com/EvilBit-Labs/DaemonEye/issues/63)

  - Wire alert generation from detection results to multi-channel delivery
  - Implement alert deduplication and priority handling
  - Add delivery status tracking and retry coordination
  - Write end-to-end tests for alert generation and delivery
  - _Requirements: All requirements integration_

- [ ] 23.4 Add unified configuration and service management - [#63](https://github.com/EvilBit-Labs/DaemonEye/issues/63)

  - Implement configuration management across all three components
  - Add daemoneye-agent process lifecycle management for collector-core components
  - Create unified logging and health reporting
  - Write integration tests for service startup, shutdown, and configuration changes
  - _Requirements: All requirements integration_
