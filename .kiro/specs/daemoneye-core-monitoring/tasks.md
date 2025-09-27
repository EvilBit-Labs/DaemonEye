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
  - Add metrics collection for IPC client performance baseline establishment (monitoring thresholds will be set after optimization)
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
  - Create criterion benchmarks to establish baseline performance metrics for message throughput and latency (no performance targets set, collecting baselines for future optimization)
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

- [x] 4.2 Implement IPC server integration in collector-core - [#86](https://github.com/EvilBit-Labs/DaemonEye/issues/86)

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
  - Add criterion benchmarks for event batching, backpressure handling, and graceful shutdown coordination (collecting baseline performance data for future optimization)
  - Create property-based tests for CollectionEvent serialization and capability negotiation
  - Implement chaos testing for EventSource failure scenarios and recovery behavior
  - Add criterion benchmarks for collector-core runtime overhead and event throughput (establishing baseline metrics for future performance tuning)
  - Write security tests for EventSource isolation and capability enforcement
  - Create compatibility tests ensuring collector-core works with existing procmond and daemoneye-agent
  - _Requirements: 11.1, 11.2, 12.1, 12.2, 13.1, 13.2, 13.5_

- [-] 5. Implement cross-platform process enumeration in procmond - [#89](https://github.com/EvilBit-Labs/DaemonEye/issues/89)

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

- [x] 5.3 Implement Linux-specific optimizations - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Create LinuxProcessCollector with direct /proc filesystem access for enhanced performance
  - Add CAP_SYS_PTRACE capability detection and privilege management
  - Implement enhanced metadata collection (memory maps, file descriptors, network connections)
  - Add support for process namespaces and container detection
  - Handle permission denied gracefully for restricted processes
  - Write Linux-specific integration tests
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [x] 5.4 Implement macOS-specific optimizations using third-party crates - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Refactor macOSProcessCollector to use well-maintained third-party crates instead of direct libc
  - Replace libproc/sysctl with enhanced sysinfo + security-framework + core-foundation + mac-sys-info (procfs is Linux-only)
  - Implement proper entitlements detection using security-framework crate for accurate code signing and bundle info
  - Add System Integrity Protection (SIP) awareness using mac-sys-info for system information
  - Handle sandboxed process restrictions gracefully with security-framework entitlements parsing
  - Maintain all existing capabilities while improving safety and accuracy
  - Write comprehensive integration tests for third-party crate approach
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [x] 5.5 Add basic process collection for secondary and minimally supported platforms - [#104](https://github.com/EvilBit-Labs/DaemonEye/issues/104)

  - Create FallbackProcessCollector for secondary platforms (FreeBSD, OpenBSD, NetBSD, etc.)
  - Use sysinfo crate as the primary collection mechanism for maximum compatibility
  - Implement graceful capability detection and feature availability reporting
  - Add platform detection logic to select appropriate collector implementation
  - Handle platform-specific limitations with clear error messages and fallback behavior
  - Write unit tests for fallback collector with mock platform detection
  - Add integration tests for FreeBSD and other secondary platforms where possible
  - _Requirements: 1.1, 1.5_

- [x] 5.6 Implement Windows-specific optimizations using third-party crates - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

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

- [ ] 5.7 Implement Monitor Collector behavior and event-driven architecture - [#89](https://github.com/EvilBit-Labs/DaemonEye/issues/89)

  - [x] 5.7.1 Create process lifecycle event detection infrastructure

  - **Prerequisites**: Complete Task 7.1 (ProcessEventSource implementation) before starting this task

  - Implement ProcessLifecycleTracker for detecting process start, stop, and modification events

  - Create ProcessSnapshot structure for maintaining process state between enumeration cycles

  - Add process state comparison logic to identify lifecycle changes (new, terminated, modified)

  - Implement efficient process tracking with PID reuse detection and timestamp validation

  - Create ProcessLifecycleEvent enum with Start, Stop, Modified, and Suspicious variants

  - Write unit tests for process lifecycle detection with mock process data

  - _Requirements: 11.1, 11.2, 11.5_

  - [x] 5.7.3 Create CollectionEvent::TriggerRequest emission system

  - Extend CollectionEvent enum with TriggerRequest variant for analysis collector coordination

  - Create TriggerRequest structure with target collector, analysis type, and priority fields

  - Implement trigger condition evaluation based on suspicious behavior detection results

  - Add trigger deduplication to prevent redundant analysis requests for the same process

  - Create trigger metadata tracking for correlation and debugging purposes

  - Implement trigger rate limiting to prevent analysis collector overload

  - Write unit tests for trigger request generation and deduplication logic

  - _Requirements: 11.1, 11.2, 11.5_

  - [x] 5.7.4 Add collector-advertised trigger conditions and SQL-to-IPC integration

  - Create TriggerCapabilities structure for collectors to advertise their trigger conditions (not user configurable)

  - Implement trigger condition advertisement in collector capability negotiation with daemoneye-agent

  - Add SQL-to-IPC detection engine integration for evaluating when triggers should fire based on user-written SQL rules

  - Create priority-based trigger queuing with backpressure handling for high-volume scenarios

  - Implement trigger condition validation against collector-advertised capabilities

  - Add trigger statistics collection for monitoring SQL rule evaluation and trigger firing

  - Write unit tests for capability advertisement, SQL rule evaluation, and trigger firing logic

  - _Requirements: 11.1, 11.2, 11.5_

  - [x] 5.7.5 Implement event bus communication infrastructure

  - Create EventBus trait for inter-collector communication and coordination

  - Implement LocalEventBus for single-node collector coordination using tokio channels

  - Add event routing and filtering based on collector capabilities and subscriptions

  - Create event correlation tracking across multiple analysis stages with correlation IDs

  - Implement event persistence and replay capabilities for reliability and debugging

  - Add backpressure handling for high-volume event scenarios with bounded channels

  - Write unit tests for event bus functionality and message routing

  - _Requirements: 11.1, 11.2, 11.5_

  - [x] 5.7.6 Add trigger request emission for analysis collectors

  - Create generic TriggerRequest emission system for coordinating with analysis collectors

  - Implement trigger request generation based on collector-advertised capabilities

  - Add trigger request validation against available collector capabilities from schema registry

  - Create trigger request routing to appropriate analysis collectors via event bus

  - Implement trigger request correlation tracking for forensic analysis

  - Add trigger request timeout and error handling for collector communication

  - Write integration tests for trigger request workflows with mock analysis collectors

  - _Requirements: 11.1, 11.2, 11.5_

  - [ ] 5.7.7 Create analysis chain coordination capabilities

  - Implement AnalysisChainCoordinator for managing multi-stage analysis workflows

  - Create analysis workflow definitions with stage dependencies and execution order

  - Add analysis result aggregation and correlation across multiple collector types

  - Implement workflow timeout and cancellation support for long-running analysis

  - Create analysis chain status tracking and progress reporting

  - Add workflow recovery and retry logic for failed analysis stages

  - Write integration tests for complex analysis workflows with multiple collectors

  - _Requirements: 11.1, 11.2, 11.5_

  - [ ] 5.7.8 Ensure collector-core framework compliance

  - Refactor Monitor Collector implementation to fully comply with collector-core EventSource trait

  - Implement proper async_trait EventSource with start/stop lifecycle and capability reporting

  - Add SourceCaps declaration with PROCESS, REALTIME, and SYSTEM_WIDE capabilities

  - Ensure proper shutdown signal handling with cooperative cancellation and timeout enforcement

  - Implement event batching and backpressure handling for collector-core integration

  - Add comprehensive error handling with graceful degradation and structured logging

  - Write unit tests for EventSource trait compliance and collector-core integration

  - _Requirements: 11.1, 11.2, 11.5_

  - [ ] 5.7.9 Add performance monitoring and baseline establishment

  - Implement performance metrics collection for events/second throughput monitoring

  - Add CPU usage tracking during continuous monitoring operations

  - Create memory usage monitoring for process tracking and event generation

  - Implement trigger event latency measurement and statistics collection

  - Add resource usage tracking for system impact assessment

  - Create criterion benchmarks for establishing baseline performance metrics

  - Write performance monitoring tests with high process churn scenarios (10,000+ processes)

  - _Requirements: 11.1, 11.2, 11.5_

  - [ ] 5.7.10 Create comprehensive testing suite for Monitor Collector behavior

  - Write unit tests for all Monitor Collector components and behavior patterns

  - Add integration tests for collector coordination and trigger workflows

  - Create property-based tests for process lifecycle detection edge cases

  - Implement chaos testing for event bus communication and collector failures

  - Add performance regression tests with baseline validation

  - Create end-to-end tests for complete Monitor Collector workflows

  - Write security tests for trigger validation and access control

  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 5.8 Add comprehensive cross-platform testing - [#39](https://github.com/EvilBit-Labs/DaemonEye/issues/39)

  - Create cross-platform integration tests for all ProcessCollector implementations
  - Implement privilege escalation/dropping tests for all platforms
  - Add criterion benchmarks with high process counts (10,000+ processes) (do not set an expected minimum performance, just collect the values)
  - Create compatibility tests for different OS versions and configurations
  - Write property-based tests for process enumeration edge cases
  - _Requirements: 1.1, 1.5, 6.1, 6.2_
  -

- [ ] 5.9 Validate GitHub issue #89 performance and acceptance criteria - [#89](https://github.com/EvilBit-Labs/DaemonEye/issues/89)

  - Establish baseline performance metrics for CPU usage during continuous monitoring (GitHub issue targets will be validated after optimization)
  - Collect baseline memory usage metrics during normal operation (GitHub issue targets will be validated after optimization)
  - Establish baseline metrics for process enumeration timing with varying system loads (GitHub issue targets will be validated after optimization)
  - Collect baseline metrics for trigger event latency (GitHub issue targets will be validated after optimization)
  - Add resource usage tracking to establish baseline metrics for system impact (optimization will be performed after baseline collection)
  - Create criterion benchmarks to establish baseline performance metrics for GitHub issue #89 acceptance criteria (collecting data for future validation and optimization)
  - Implement scalability testing for high process churn environments (10,000+ processes)
  - Add comprehensive testing coverage (>90% unit tests, integration tests, cross-platform compatibility)
  - Validate event generation and triggering functionality meets GitHub issue specifications
  - Ensure data integration with collector-core pipeline and structured logging requirements
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [ ] 6. Implement executable integrity verification with SHA-256 hashing - [#40](https://github.com/EvilBit-Labs/DaemonEye/issues/40)

  - Create HashComputer trait for cryptographic hashing of executable files
  - Implement SHA-256 hash computation for accessible executable files in ProcessMessageHandler
  - Handle missing or inaccessible executable files without failing enumeration
  - Store hash_algorithm field ('sha256') with computed hash values in ProtoProcessRecord
  - Write criterion benchmarks to establish baseline performance metrics for hashing impact on enumeration speed (collecting data for future optimization)
  - _Requirements: 2.1, 2.2, 2.4_

- [-] 7. Create ProcessEventSource and refactor procmond to use collector-core

- [x] 7.1 Create ProcessEventSource wrapping existing ProcessMessageHandler - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - Implement ProcessEventSource that wraps existing ProcessMessageHandler from procmond/src/lib.rs
  - Reuse existing enumerate_processes() and convert_process_to_record() logic
  - Integrate with existing database::DatabaseManager and storage layer
  - Preserve existing sysinfo-based process enumeration and hash computation
  - Add CollectionEvent::Process generation from existing ProtoProcessRecord
  - Write unit tests for ProcessEventSource integration with existing components
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 7.2 Refactor procmond to use collector-core framework - [#76](https://github.com/EvilBit-Labs/DaemonEye/issues/76)

  - **Prerequisites**: Complete Task 5.7 (Monitor Collector behavior) before integrating with collector-core
  - Refactor procmond main.rs to use collector-core Collector instead of direct IPC server
  - Preserve existing CLI parsing, configuration loading, and database initialization
  - Register ProcessEventSource with collector-core runtime
  - Integrate Monitor Collector behavior from Task 5.7 with collector-core framework
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

- [x] 9. Implement procmond as Monitor Collector with daemoneye-agent lifecycle management - [#89](https://github.com/EvilBit-Labs/DaemonEye/issues/89)

- [ ] 9.1 Add proc-daemon crate dependency and cross-platform service infrastructure

  - Add `proc-daemon` crate dependency for cross-platform daemon/service functionality
  - Create `ServiceManager` trait for cross-platform service lifecycle management
  - Implement platform-specific service managers: `UnixServiceManager` and `WindowsServiceManager`
  - Add service configuration structure with startup options, working directory, and user context
  - Create daemoneye-agent service wrapper that manages procmond collector lifecycle
  - Write unit tests for service manager trait implementations and configuration validation
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.2 Integrate Monitor Collector behavior with service lifecycle management

  - Integrate Task 5.8 Monitor Collector implementation with daemoneye-agent service management
  - Add service-specific configuration for Monitor Collector behavior and trigger conditions
  - Implement service restart and recovery handling for Monitor Collector processes
  - Add service health monitoring integration for Monitor Collector status reporting
  - Create service-level coordination between Monitor Collector and analysis collectors
  - Write integration tests for service lifecycle with Monitor Collector behavior
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 9.3 Add collector orchestration and trigger system integration

  - Implement trigger system for coordinating with analysis collectors (binary hasher, memory analyzer)
  - Create event bus communication between procmond and other collectors
  - Add support for triggering Binary Hasher collector for suspicious executables
  - Implement event correlation and analysis chain coordination
  - Create priority-based triggering (Critical, High, Normal, Low) based on threat assessment
  - Write integration tests for collector coordination and trigger workflows
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 9.4 Implement daemoneye-agent as system service with collector supervision

  - Create Unix daemon functionality using proc-daemon crate for Linux and macOS
  - Add Windows service functionality with Service Control Manager (SCM) integration
  - Implement collector process lifecycle management and supervision within daemoneye-agent
  - Add collector process health monitoring with heartbeat checks and restart policies
  - Create graceful shutdown coordination for all managed collector processes
  - Write integration tests for service lifecycle and collector supervision
  - _Requirements: 6.1, 6.2, 6.4, 6.5, 11.1, 11.2_

- [ ] 9.5 Add event-driven architecture and collector coordination

  - Implement event bus system for inter-collector communication as specified in GitHub issue #89
  - Create event routing and filtering based on collector capabilities
  - Add event correlation tracking across multiple analysis stages
  - Implement backpressure handling for high-volume event scenarios
  - Create event persistence and replay capabilities for reliability
  - Add support for "Two-Tier Collector Framework" and "Event Bus System" dependencies from GitHub issue #89
  - Implement efficient triggering of analysis collectors only when needed (event-driven requirement)
  - Write integration tests for event-driven workflows and collector coordination
  - _Requirements: 11.1, 11.2, 12.1, 12.2_

- [ ] 9.6 Integrate with existing daemoneye-agent architecture

  - Modify daemoneye-agent main.rs to support both interactive and service/daemon modes
  - Add CLI flags for service operations: --install, --uninstall, --start, --stop, --status
  - Integrate collector supervision with existing configuration system and database initialization
  - Preserve existing IPC client functionality and detection engine integration
  - Add service-specific logging configuration with file rotation and retention policies
  - Write integration tests ensuring service mode maintains identical functionality to interactive mode
  - _Requirements: 6.1, 6.2, 6.4, 6.5, 11.1, 11.2_

- [ ] 9.7 Add service monitoring and health reporting capabilities

  - Implement service health endpoints for external monitoring systems
  - Add service status reporting with component health aggregation
  - Create service metrics export for monitoring collector process status and establishing performance baselines
  - Implement service configuration validation and startup diagnostics
  - Add service log aggregation from all managed collector processes
  - Create service recovery actions for common failure scenarios and collector crashes
  - Write monitoring integration tests with simulated component failures and recovery validation
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [ ] 9.8 Create service deployment and configuration management

  - Create systemd service unit files for Linux with proper dependencies and security settings
  - Add launchd plist files for macOS with appropriate permissions and startup configuration
  - Create Windows service installer with proper registry entries and security descriptors
  - Implement service configuration validation and deployment verification
  - Add service update and migration procedures for configuration changes
  - Create service deployment documentation with platform-specific installation guides
  - Write deployment automation scripts for common service management scenarios
  - _Requirements: 6.1, 6.2, 6.4, 6.5_

- [ ] 9.9 Add comprehensive service testing and validation

  - Create integration tests for service installation, startup, and shutdown on all platforms
  - Add service failure scenario testing with collector process crashes and recovery
  - Implement service upgrade testing with configuration migration and data preservation
  - Create service security testing for privilege boundaries and access controls
  - Add service performance baseline collection under high load with multiple collector processes (establishing metrics for future optimization)
  - Write service compatibility tests across different OS versions and configurations
  - Create chaos testing for various failure modes and recovery validation
  - Add end-to-end tests for procmond Monitor Collector triggering analysis collectors
  - Validate all GitHub issue #89 acceptance criteria including Monitor Collector requirements
  - Test continuous operation, event generation, and collector coordination as specified
  - Validate daemoneye-agent integration, health check endpoints, and graceful shutdown
  - Test cross-platform compatibility (Linux, Windows, macOS) per GitHub issue requirements
  - Collect baseline performance metrics for GitHub issue #89 requirements (validation will occur after optimization phase)
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
  - Add criterion benchmarks comparing SQL-to-IPC vs placeholder detection performance (establishing baseline metrics for future optimization)
  - Implement security testing for SQL parsing and pushdown optimization
  - Create integration tests for reactive pipeline orchestration and auto-correlation
  - Write compatibility tests ensuring existing detection workflows continue to function
  - Add criterion benchmarks for SQL parsing, planning, and execution phases (establishing baseline performance metrics for future optimization)
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
  - Implement performance baseline metrics reporting and resource usage tracking (thresholds will be set after optimization)
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

- [ ] 20. Add criterion benchmarks for performance-sensitive functions

- [ ] 20.1 Benchmark process enumeration and collection functions

  - Create criterion benchmarks for ProcessCollector trait implementations (sysinfo, Linux, macOS, Windows)
  - Add benchmarks for process enumeration with varying process counts (100, 1000, 10000+ processes)
  - Benchmark cross-platform process metadata extraction and conversion functions
  - Create benchmarks for ProcessEventSource event generation and batching
  - Add memory usage benchmarks for process data structures and serialization
  - Benchmark process filtering and predicate evaluation performance
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 20.2 Benchmark SHA-256 hashing and integrity verification functions

  - Create criterion benchmarks for HashComputer trait implementations
  - Add benchmarks for SHA-256 computation with varying file sizes (1KB, 1MB, 100MB+)
  - Benchmark executable file access and hash computation under different privilege levels
  - Create benchmarks for hash verification and comparison operations
  - Add memory usage benchmarks for hash computation and storage
  - Benchmark concurrent hashing operations and resource contention
  - _Requirements: 2.1, 2.2, 2.4_

- [ ] 20.3 Benchmark IPC communication and serialization functions

  - Create criterion benchmarks for IpcCodec protobuf serialization and deserialization
  - Add benchmarks for IPC message framing, CRC32 validation, and transport overhead
  - Benchmark IpcClientManager connection establishment and reconnection logic
  - Create benchmarks for concurrent IPC communication with multiple collectors
  - Add memory usage benchmarks for IPC message buffers and connection pooling
  - Benchmark capability negotiation and schema registry operations
  - _Requirements: 3.1, 3.2, 11.1, 11.2_

- [ ] 20.4 Benchmark database operations and storage functions

  - Create criterion benchmarks for redb database initialization and table creation
  - Add benchmarks for ProcessRecord, Alert, and DetectionRule serialization/deserialization
  - Benchmark database write operations with varying batch sizes and commit strategies
  - Create benchmarks for database query operations and index performance
  - Add memory usage benchmarks for database connections and transaction handling
  - Benchmark concurrent database access and lock contention scenarios
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 20.5 Benchmark alert delivery and notification functions

  - Create criterion benchmarks for alert generation and formatting operations
  - Add benchmarks for multi-channel alert delivery (stdout, syslog, webhook, email)
  - Benchmark alert deduplication and rate limiting algorithms
  - Create benchmarks for alert delivery retry logic and circuit breaker operations
  - Add memory usage benchmarks for alert queuing and batching mechanisms
  - Benchmark concurrent alert delivery and sink performance under load
  - _Requirements: 5.2, 5.3, 5.4, 5.5_

- [ ] 20.6 Benchmark SQL parsing and detection engine functions

  - Create criterion benchmarks for SQL parsing and AST generation using sqlparser-rs
  - Add benchmarks for SQL validation and security checking operations
  - Benchmark detection rule compilation and optimization phases
  - Create benchmarks for rule execution and pattern matching performance
  - Add memory usage benchmarks for compiled rules and execution contexts
  - Benchmark concurrent rule execution and resource sharing scenarios
  - _Requirements: 3.1, 3.2, 3.3, 3.5_

- [ ] 20.7 Benchmark collector-core framework functions

  - Create criterion benchmarks for EventSource trait registration and lifecycle management
  - Add benchmarks for CollectionEvent generation, batching, and routing performance
  - Benchmark capability negotiation and schema registry lookup operations
  - Create benchmarks for event bus communication and inter-collector coordination
  - Add memory usage benchmarks for collector runtime overhead and event buffering
  - Benchmark graceful shutdown coordination and resource cleanup operations
  - _Requirements: 11.1, 11.2, 11.5_

- [ ] 20.8 Create performance regression detection and CI integration

  - Set up criterion benchmark baseline storage and comparison infrastructure
  - Add automated performance regression detection in CI pipeline
  - Create performance alert thresholds and notification mechanisms
  - Implement benchmark result visualization and trend analysis
  - Add performance budget framework for critical code paths (budgets will be set after baseline collection)
  - Create benchmark documentation and performance baseline collection guidelines
  - _Requirements: All performance requirements verification_

- [ ] 21. Implement comprehensive observability and metrics - [#60](https://github.com/EvilBit-Labs/DaemonEye/issues/60)

  - Add Prometheus-compatible metrics export for collection rate, detection latency, alert delivery
  - Create structured logging with correlation IDs and performance baseline metrics collection
  - Implement HTTP health endpoints (localhost-only) for external monitoring
  - Add resource utilization metrics (CPU, memory, disk usage) and error rate tracking
  - Write metrics accuracy tests and Prometheus scraping compatibility verification
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [x] 22. Create comprehensive test suite and quality assurance

- [ ] 22.1 Implement unit test coverage - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Add unit tests for all core functionality targeting >85% code coverage
  - Set up llvm-cov for code coverage measurement and reporting
  - Create test utilities and mock objects for isolated testing
  - Write unit tests for error handling and edge cases
  - _Requirements: All requirements verification_

- [ ] 22.2 Add integration and CLI testing - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Implement integration tests with insta for CLI snapshot testing
  - Add cross-component interaction tests for interprocess-based IPC communication
  - Create end-to-end workflow tests for complete monitoring scenarios
  - Write snapshot tests with insta for CLI output validation
  - _Requirements: All requirements verification_

- [ ] 22.3 Create performance and property-based testing - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Add criterion benchmarks for baseline collection on critical paths (process enumeration, SQL execution, IPC throughput) - regression detection will be enabled after optimization
  - Implement property-based tests with proptest for edge case discovery in data models, SQL parsing, and collector-core event handling
  - Create criterion benchmarks for high-volume process monitoring baseline collection (10,000+ processes, sustained monitoring) - performance targets will be set after optimization
  - Write benchmark tests for collector-core framework overhead and event source registration/deregistration
  - Add memory usage benchmarks for long-running monitoring scenarios and database growth patterns
  - Create criterion benchmarks for alert delivery under high-volume detection scenarios
  - _Requirements: All requirements verification_

- [ ] 22.4 Set up CI matrix and quality gates - [#61](https://github.com/EvilBit-Labs/DaemonEye/issues/61)

  - Set up GitHub Actions CI matrix for Linux, macOS, Windows with multiple Rust versions (stable, beta, MSRV)
  - Add automated quality gates: fmt-check, clippy strict, comprehensive test suite
  - Implement performance regression detection with criterion benchmarks
  - Add dependency scanning, SLSA provenance (Enterprise), and security validation
  - Create automated release pipeline with platform-specific packages and code signing
  - _Requirements: All requirements verification_

- [ ] 23. Comprehensive stress testing and load validation

- [ ] 23.1 Implement collector-core stress testing suite

  - Create stress tests for event batching under extreme load (100,000+ events/second)
  - Add stress tests for backpressure handling with multiple blocked event sources
  - Implement stress tests for graceful shutdown coordination under heavy load
  - Create memory pressure tests for event source registration/deregistration cycles
  - Add concurrent stress tests for multiple EventSource instances with resource contention
  - Write endurance tests for 24+ hour continuous operation under load
  - _Requirements: 11.1, 11.2, 12.1, 12.2, 13.1, 13.2, 13.5_

- [ ] 23.2 Process enumeration stress testing

  - Create stress tests with extremely high process counts (50,000+ processes)
  - Add stress tests for rapid process creation/termination scenarios
  - Implement memory pressure tests for process enumeration with limited resources
  - Create concurrent enumeration stress tests with multiple collectors
  - Add privilege boundary stress tests under resource exhaustion
  - Write platform-specific stress tests for OS-level resource limits
  - _Requirements: 1.1, 1.5, 6.1, 6.2_

- [ ] 23.3 Database and storage stress testing

  - Create stress tests for redb database under extreme write loads (10,000+ records/second)
  - Add stress tests for concurrent read/write operations with resource contention
  - Implement stress tests for database growth and retention policy enforcement
  - Create memory pressure tests for large dataset queries and aggregations
  - Add stress tests for database corruption recovery and integrity validation
  - Write endurance tests for long-term database stability under continuous load
  - _Requirements: 1.3, 4.4, 7.4_

- [ ] 23.4 Alert delivery stress testing

  - Create stress tests for alert delivery under high-volume detection scenarios (1,000+ alerts/minute)
  - Add stress tests for multiple alert sink failures and recovery scenarios
  - Implement stress tests for network partition and connectivity issues
  - Create memory pressure tests for alert queuing and dead letter queue management
  - Add stress tests for circuit breaker behavior under sustained failures
  - Write endurance tests for alert delivery reliability over extended periods
  - _Requirements: 5.2, 5.3, 5.4, 5.5_

- [ ] 23.5 IPC communication stress testing

  - Create stress tests for IPC communication under extreme message loads
  - Add stress tests for connection failures and automatic reconnection scenarios
  - Implement stress tests for message serialization/deserialization under load
  - Create memory pressure tests for IPC buffer management and backpressure
  - Add stress tests for capability negotiation under rapid collector restarts
  - Write endurance tests for IPC stability over extended operation periods
  - _Requirements: 3.1, 3.2, 11.1, 11.2_

- [ ] 23.6 System-wide integration stress testing

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

- [ ] 24. Add advanced security testing and validation - [#62](https://github.com/EvilBit-Labs/DaemonEye/issues/62)

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
