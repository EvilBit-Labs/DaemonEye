# Implementation Plan

- [ ] 1. Set up SQL-to-IPC detection engine foundation and core parsing infrastructure

- [ ] 1.1 Create sql-to-ipc-engine crate structure and dependencies

  - Create new `sql-to-ipc-engine` module in daemoneye-lib with proper Cargo.toml dependencies
  - Add sqlparser-rs, serde, and uuid dependencies with appropriate feature flags
  - Create basic module structure (parser, planner, executor, types) with public exports
  - Add workspace-level lints and ensure zero-warnings compilation
  - _Requirements: 1.1, 1.2_

- [ ] 1.2 Implement core data structures for detection plans and collection requirements

  - Create `DetectionPlan` struct with rule metadata, AST, and execution components
  - Implement `CollectionRequirements` with tables, predicates, projections, and joins
  - Add `TableReference`, `Predicate`, `Projection` types with serde serialization
  - Create `ExecutionCost` estimation struct for query optimization
  - Write unit tests for data structure serialization and validation
  - _Requirements: 1.1, 1.2, 1.3_

- [ ] 1.3 Build SqlAnalyzer with basic SQL parsing and AST validation

  - Implement `SqlAnalyzer` struct with SQLiteDialect parser initialization
  - Add `parse_rule` method that converts SQL string to AST with error handling
  - Create basic AST validation for SELECT-only statements and forbidden constructs
  - Implement table extraction from FROM clauses with alias support
  - Write unit tests for SQL parsing with valid and invalid SQL statements
  - _Requirements: 1.1, 1.2, 1.3, 1.4_

- [ ] 1.4 Add SQL security validation with function whitelist and statement restrictions

  - Create `SqlValidator` with comprehensive function whitelist (aggregations, string ops, datetime)
  - Implement statement type validation to reject non-SELECT statements
  - Add validation for forbidden functions (load_extension, readfile, system, random)
  - Create detailed error messages for security violations with suggestions
  - Write security validation tests with OWASP SQL injection test vectors
  - _Requirements: 1.4, 1.5_

- [ ] 1.5 Implement predicate and projection extraction from SQL AST

  - Add predicate extraction from WHERE clauses with operator and operand identification
  - Implement projection extraction from SELECT clauses with column and alias handling
  - Create JOIN requirement extraction with join type and condition analysis
  - Add aggregation detection for GROUP BY and HAVING clauses
  - Write comprehensive tests for AST analysis with complex SQL queries
  - _Requirements: 1.1, 1.2, 1.3, 1.5_

- [ ] 2. Implement schema registry and collector capability negotiation system

- [ ] 2.1 Create core schema registry data structures

  - Implement `CollectorDescriptor` with collector metadata, version, and table definitions
  - Create `TableSchema` with columns, keys, join hints, and pushdown capabilities
  - Add `PushdownCapabilities` with supported predicates, projections, and resource limits
  - Implement `JoinHint` for parent/child relationships and correlation metadata
  - Write unit tests for schema data structure validation and serialization
  - _Requirements: 3.1, 3.2_

- [ ] 2.2 Build SchemaCatalog for managing collector registrations

  - Implement `SchemaCatalog` with thread-safe collector and table storage
  - Add collector registration with schema validation and conflict detection
  - Create table lookup methods with collector mapping and capability queries
  - Implement schema versioning and backward compatibility checking
  - Write unit tests for catalog operations with multiple collectors and schema conflicts
  - _Requirements: 3.1, 3.2, 3.3_

- [ ] 2.3 Extend protobuf definitions for capability negotiation

  - Add `CollectionCapabilities` message to existing protobuf schema
  - Extend `DetectionTask` with optional supplemental rule data fields
  - Create `SchemaDescriptor` and `TableDefinition` protobuf messages
  - Add capability advertisement to collector startup handshake protocol
  - Write protobuf serialization tests and backward compatibility validation
  - _Requirements: 3.2, 3.3, 3.4_

- [ ] 2.4 Implement dynamic schema updates and change notifications

  - Add schema update methods with atomic replacement and rollback support
  - Create change notification system for schema registry subscribers
  - Implement schema compatibility validation for updates and migrations
  - Add schema change event logging for audit and debugging purposes
  - Write integration tests for schema updates with active query plans
  - _Requirements: 3.3, 3.4, 3.5_

- [ ] 2.5 Create collection planning from schema catalog

  - Implement `plan_collection` method that maps requirements to collector tasks
  - Add collector selection logic based on table ownership and capabilities
  - Create task generation with capability-based operation partitioning
  - Implement error handling for missing collectors and unsupported operations
  - Write integration tests with mock collectors and complex collection requirements
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5_

- [ ] 3. Build pushdown planner for optimal query distribution between collectors and agent

- [ ] 3.1 Create basic PushdownPlanner structure and predicate analysis

  - Implement `PushdownPlanner` struct with schema catalog integration
  - Add predicate analysis to classify operations as pushdown-eligible or pipeline-only
  - Create `can_pushdown_predicate` method based on collector capabilities
  - Implement basic operation partitioning between collectors and agent pipeline
  - Write unit tests for predicate classification with various SQL operators
  - _Requirements: 2.1, 2.2, 2.3_

- [ ] 3.2 Implement DetectionTask generation with resource constraints

  - Create `DetectionTask` generation from pushdown operations and collector capabilities
  - Add rate limiting and resource constraint application based on collector limits
  - Implement task validation against collector schema and capability matrix
  - Create task serialization for IPC communication with existing protobuf protocol
  - Write unit tests for task generation with various collector configurations
  - _Requirements: 2.1, 2.2, 2.4_

- [ ] 3.3 Add projection pushdown and column optimization

  - Implement projection analysis to determine required columns for each collector
  - Create column requirement extraction from SELECT, WHERE, and JOIN clauses
  - Add projection pushdown optimization to minimize data transfer
  - Implement column mapping between SQL aliases and collector schema
  - Write unit tests for projection optimization with complex SELECT statements
  - _Requirements: 2.1, 2.2, 2.3_

- [ ] 3.4 Build cost estimation and optimization strategy selection

  - Implement `CostEstimator` with basic selectivity estimation for predicates
  - Add cost-based decision making for pushdown vs pipeline execution
  - Create optimization strategy selection based on estimated data volumes
  - Implement driving table selection for multi-table queries
  - Write unit tests for cost estimation with various query patterns
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5_

- [ ] 3.5 Create query plan caching and invalidation system

  - Implement query plan caching with SQL fingerprinting and parameter binding
  - Add cache invalidation on schema changes and collector availability updates
  - Create plan cache statistics and hit rate monitoring
  - Implement cache size limits and LRU eviction policy
  - Write integration tests for plan caching with schema updates and collector changes
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5_

- [ ] 4. Implement redb-based operator pipeline engine for complex SQL operations

- [ ] 4.1 Create basic RedbStorage with table definitions and key encoding

  - Implement `RedbStorage` struct with redb database initialization and configuration
  - Create table definitions for processes, network, filesystem, and YARA events
  - Add composite key encoding for time-based partitioning (timestamp, sequence)
  - Implement basic CRUD operations with proper error handling and transactions
  - Write unit tests for storage operations and key encoding/decoding
  - _Requirements: 4.1, 4.2_

- [ ] 4.2 Add secondary indexes for efficient query execution

  - Create secondary index tables for pid, name_hash, exe_hash, and path_prefix lookups
  - Implement index maintenance during record insertion and updates
  - Add set-based intersection operations for multi-index queries
  - Create index selection logic based on query predicates and selectivity
  - Write performance tests for index operations with large datasets
  - _Requirements: 4.1, 4.2, 4.3_

- [ ] 4.3 Implement batch write operations with group commit optimization

  - Add batch insertion methods with configurable batch sizes and commit intervals
  - Implement group commit optimization to reduce fsync overhead
  - Create backpressure handling for high-volume write scenarios
  - Add write performance monitoring and throughput metrics
  - Write stress tests for batch operations with concurrent readers and writers
  - _Requirements: 4.1, 4.2, 4.3_

- [ ] 4.4 Build basic JoinExecutor with Index Nested Loop Join strategy

  - Implement `JoinExecutor` struct with join strategy selection logic
  - Create Index Nested Loop Join (INLJ) implementation using secondary indexes
  - Add join key extraction and record merging for equi-joins
  - Implement LEFT JOIN support with null-extended results
  - Write unit tests for INLJ with various join patterns and data sizes
  - _Requirements: 4.1, 4.2, 4.3_

- [ ] 4.5 Add MaterializedRelationCache for fast parent/child joins

  - Implement `MaterializedRelationCache` (MRC) for parent process information
  - Create cache population and maintenance during process event ingestion
  - Add fast parent/child join execution using MRC lookup instead of full table scan
  - Implement cache invalidation and refresh policies for stale data
  - Write performance benchmarks comparing MRC vs full table joins
  - _Requirements: 4.1, 4.2, 4.3, 4.4_

- [ ] 4.6 Implement BoundedSymmetricHashJoin with cardinality caps

  - Create `BoundedSymmetricHashJoin` with configurable memory limits
  - Add hash table population with cardinality caps and LRU eviction
  - Implement automatic fallback to INLJ when memory limits are exceeded
  - Create join result streaming with bounded memory usage
  - Write memory usage tests and cardinality limit validation
  - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

- [ ] 4.7 Build AggregationExecutor with bounded hash aggregation

  - Implement `AggregationExecutor` with GROUP BY and HAVING support
  - Create bounded hash aggregation with configurable memory limits
  - Add time window enforcement for aggregations to prevent unbounded state
  - Implement aggregation result streaming with incremental updates
  - Write aggregation tests with large cardinality groups and memory constraints
  - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

- [ ] 4.8 Create OperatorPipeline for coordinating complex query execution

  - Implement `OperatorPipeline` that orchestrates scan, filter, join, and aggregation operations
  - Add pipeline operation sequencing based on query execution plan
  - Create result streaming between pipeline stages with backpressure handling
  - Implement query timeout and cancellation support across all pipeline stages
  - Write integration tests for complete pipeline execution with complex SQL queries
  - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

- [ ] 5. Build reactive pipeline orchestrator for cascading analysis and auto-correlation

- [ ] 5.1 Create basic ReactiveOrchestrator structure and trigger rule system

  - Implement `ReactiveOrchestrator` struct with collector client management
  - Create `TriggerRule` data structure for defining analysis triggers
  - Add `AnalysisType` enum for different types of cascading analysis (YARA, PE, memory)
  - Implement basic trigger rule evaluation against incoming events
  - Write unit tests for trigger rule matching with various event patterns
  - _Requirements: 5.1, 5.2_

- [ ] 5.2 Add analysis task queuing and prioritization

  - Implement analysis task queue with priority ordering and resource limits
  - Create `AnalysisTask` structure with task metadata and execution context
  - Add task deduplication to avoid redundant analysis of the same data
  - Implement task timeout and cancellation support for long-running analysis
  - Write unit tests for task queuing, prioritization, and deduplication logic
  - _Requirements: 5.1, 5.2, 5.3_

- [ ] 5.3 Implement correlation result caching and management

  - Create correlation result cache with TTL-based expiration and size limits
  - Add correlation metadata tracking for debugging and audit purposes
  - Implement cache invalidation when source data changes or expires
  - Create correlation ID generation and tracking across analysis stages
  - Write unit tests for correlation caching and metadata management
  - _Requirements: 5.1, 5.2, 5.3, 5.4_

- [ ] 5.4 Build analysis task execution and collector integration

  - Implement analysis task execution with collector client communication
  - Add support for YARA scanning tasks with rule compilation and execution
  - Create PE analysis task execution with file metadata extraction
  - Implement memory analysis task execution with process memory inspection
  - Write integration tests for analysis task execution with mock collectors
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

- [ ] 5.5 Create AutoJoinManager for automatic correlation detection

  - Implement `AutoJoinManager` with auto-JOIN rule registration and evaluation
  - Create `AutoJoinRule` structure for defining automatic correlation patterns
  - Add implicit correlation detection for table references not in FROM clauses
  - Implement automatic collection triggering when JOINs reference missing data
  - Write unit tests for auto-JOIN detection and correlation triggering
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

- [ ] 5.6 Extend SQL parser for AUTO JOIN syntax support

  - Create custom SQL dialect extension for AUTO JOIN and WHEN clauses
  - Implement parser modifications to handle extended JOIN syntax
  - Add AST node types for auto-JOIN specifications and trigger conditions
  - Create SQL validation for auto-JOIN syntax and semantic correctness
  - Write parser tests for extended SQL syntax with various auto-JOIN patterns
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

- [ ] 6. Integrate specialty collectors for advanced pattern matching (YARA, eBPF, PE analysis)

- [ ] 6.1 Extend protobuf schema for supplemental rule data

  - Add `SupplementalRuleData` message to existing protobuf definitions
  - Create `YaraRules`, `NetworkAnalysis`, and `PlatformSpecific` rule data types
  - Extend `DetectionTask` message with optional supplemental rule data field
  - Add rule data serialization and validation for different specialty collector types
  - Write protobuf serialization tests and backward compatibility validation
  - _Requirements: 6.1, 6.2_

- [ ] 6.2 Create YARA rule data structures and compilation support

  - Implement `YaraRule` structure with rule content, scan targets, and options
  - Create `YaraScanOptions` with timeout, fast scan, and memory limit configuration
  - Add YARA rule compilation and validation with error handling
  - Implement rule caching to avoid recompilation of frequently used rules
  - Write unit tests for YARA rule compilation and caching behavior
  - _Requirements: 6.1, 6.2, 6.3_

- [ ] 6.3 Build YaraCollector using collector-core framework

  - Create `YaraCollector` implementing the EventSource trait from collector-core
  - Add YARA scanning capabilities with file content and memory region support
  - Implement scan result generation and streaming to daemoneye-agent
  - Create YARA scan result data structures with match details and metadata
  - Write integration tests for YaraCollector with collector-core runtime
  - _Requirements: 6.1, 6.2, 6.3, 6.4_

- [ ] 6.4 Implement network analysis collector with cross-platform support

  - Create `NetworkAnalysisCollector` with platform detection and capability adaptation
  - Add network pattern matching with regex support for IP addresses and payloads
  - Implement network filter configuration with protocol, port, and direction filtering
  - Create network analysis result streaming with connection metadata and pattern matches
  - Write cross-platform tests with graceful degradation for unsupported platforms
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

- [ ] 6.5 Create PE analysis collector for Windows executable inspection

  - Implement `PeAnalysisCollector` for Windows PE file metadata extraction
  - Add import table analysis with suspicious API detection capabilities
  - Create section analysis with entropy calculation and packing detection
  - Implement entry point analysis and code signature verification
  - Write tests for PE analysis integration with process monitoring workflows
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

- [ ] 6.6 Add specialty collector result integration with SQL pipeline

  - Create table schemas for YARA, network analysis, and PE analysis results
  - Implement result storage in redb with appropriate indexing for JOIN operations
  - Add specialty result correlation with process data through foreign key relationships
  - Create SQL query support for specialty collector results with proper type handling
  - Write integration tests for SQL queries combining process data with specialty analysis results
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

- [ ] 7. Build comprehensive error handling and recovery system

- [ ] 7.1 Create ErrorRecoveryManager with circuit breaker pattern

  - Implement `ErrorRecoveryManager` with per-collector circuit breaker tracking
  - Create `CircuitBreaker` with configurable failure thresholds and state transitions
  - Add circuit breaker state management (Closed, Open, HalfOpen) with timeout handling
  - Implement failure recording and success rate tracking for circuit breaker decisions
  - Write unit tests for circuit breaker state transitions and failure threshold behavior
  - _Requirements: 9.1, 9.2_

- [ ] 7.2 Add retry policies with exponential backoff and jitter

  - Create `RetryPolicy` with configurable max attempts, base delay, and backoff multiplier
  - Implement exponential backoff calculation with jitter to prevent thundering herd
  - Add retry attempt tracking and logging for debugging and monitoring
  - Create retry decision logic based on error type and circuit breaker state
  - Write unit tests for retry policy behavior and backoff calculation
  - _Requirements: 9.1, 9.2, 9.3_

- [ ] 7.3 Implement graceful degradation and fallback strategies

  - Create `FallbackStrategy` enum for different degradation approaches
  - Add collector availability tracking and capability-based query plan adjustment
  - Implement partial result generation when some collectors are unavailable
  - Create fallback to agent-side processing when collector pushdown fails
  - Write integration tests for graceful degradation scenarios with collector failures
  - _Requirements: 9.1, 9.2, 9.3, 9.4_

- [ ] 7.4 Add query execution timeout and resource limit enforcement

  - Implement query timeout with cancellation token propagation across pipeline stages
  - Create memory limit monitoring and enforcement for join and aggregation operations
  - Add resource cleanup for cancelled or failed query executions
  - Implement partial result emission when resource limits are exceeded
  - Write stress tests for resource exhaustion and timeout scenarios
  - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5_

- [ ] 7.5 Create comprehensive error classification and recovery actions

  - Implement error classification system for different types of failures
  - Add recovery action selection based on error type and system state
  - Create error correlation and pattern detection for proactive failure handling
  - Implement error reporting and alerting for critical system failures
  - Write chaos testing scenarios for various failure modes and recovery validation
  - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5_

- [ ] 8. Create comprehensive observability and performance monitoring

- [ ] 8.1 Implement core metrics collection infrastructure

  - Create `MetricsCollector` with Prometheus-compatible metric types (Counter, Gauge, Histogram)
  - Add metric registration and collection for rule evaluation latency and match rates
  - Implement collector performance metrics for IPC throughput and queue depths
  - Create storage operation metrics for redb write latency and index operations
  - Write unit tests for metrics collection and Prometheus format export
  - _Requirements: 10.1, 10.2_

- [ ] 8.2 Add detailed pipeline performance monitoring

  - Implement query execution timing metrics for each pipeline stage
  - Create memory usage tracking for join and aggregation operations
  - Add pushdown optimization metrics (pushdown ratio, fallback frequency)
  - Implement collector task distribution and response time tracking
  - Write performance monitoring tests with synthetic workloads and validation
  - _Requirements: 10.1, 10.2, 10.3_

- [ ] 8.3 Create correlation ID system for distributed tracing

  - Implement correlation ID generation and propagation across all system components
  - Add correlation ID injection into log messages and metric labels
  - Create trace context propagation through IPC communication with collectors
  - Implement correlation ID tracking in query execution plans and alert generation
  - Write tracing integration tests with multi-collector queries and correlation validation
  - _Requirements: 11.1, 11.2_

- [ ] 8.4 Add execution plan logging and performance breakdown

  - Implement execution plan logging with optimization decisions and cost estimates
  - Create step-by-step query execution tracing with timing and resource usage
  - Add performance breakdown reporting for pushdown vs pipeline execution
  - Implement query plan visualization and debugging information export
  - Write execution tracing tests with complex queries and performance analysis
  - _Requirements: 11.1, 11.2, 11.3_

- [ ] 8.5 Create alert correlation metadata and forensic tracking

  - Implement alert correlation metadata with source data references and execution context
  - Add forensic tracking for alert generation with rule execution details
  - Create alert delivery tracking with success rates, retry counts, and latency metrics
  - Implement alert correlation ID linking for multi-stage detection workflows
  - Write forensic tracking tests with complex detection scenarios and metadata validation
  - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5_

- [ ] 9. Build integration layer with existing DaemonEye components

- [ ] 9.1 Create SQL-to-IPC engine integration interface for daemoneye-agent

  - Create `SqlToIpcEngine` wrapper that implements existing `DetectionEngine` trait
  - Add configuration integration for SQL-to-IPC engine with existing daemoneye-agent config
  - Implement rule loading and validation using SQL-to-IPC parser instead of placeholder logic
  - Create integration layer that bridges SQL-to-IPC results with existing alert generation
  - Write unit tests for integration interface and configuration compatibility
  - _Requirements: 12.1, 12.2_

- [ ] 9.2 Replace existing detection engine with SQL-to-IPC implementation

  - Modify daemoneye-agent main loop to use SQL-to-IPC engine instead of placeholder detection
  - Update existing rule execution workflow to use SQL parsing and pushdown planning
  - Integrate SQL-to-IPC query execution with existing database storage and alert management
  - Maintain backward compatibility with existing detection rule file formats
  - Write integration tests comparing old vs new detection engine behavior
  - _Requirements: 12.1, 12.2, 12.3_

- [ ] 9.3 Integrate SQL-to-IPC with existing IPC client infrastructure

  - Modify existing `IpcClientManager` to handle SQL-to-IPC generated detection tasks
  - Add collector capability negotiation to existing IPC client connection workflow
  - Integrate schema registry with existing collector discovery and connection management
  - Update existing IPC protocol handling to support supplemental rule data
  - Write integration tests for IPC client with SQL-to-IPC task generation and execution
  - _Requirements: 12.1, 12.2, 12.3, 12.4_

- [ ] 9.4 Extend collector-core EventSource trait for SQL-to-IPC task handling

  - Modify collector-core `EventSource` trait to accept `DetectionTask` with supplemental rules
  - Add capability advertisement methods to EventSource for schema registry integration
  - Update existing ProcessEventSource to handle SQL-to-IPC generated tasks
  - Create task validation and execution logic in collector-core runtime
  - Write unit tests for EventSource modifications and task handling
  - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5_

- [ ] 9.5 Add schema registry integration to collector-core runtime

  - Integrate schema registry with collector-core startup and capability advertisement
  - Add automatic schema descriptor generation from EventSource implementations
  - Create schema update notification system for collector capability changes
  - Implement schema validation and compatibility checking in collector-core
  - Write integration tests for schema registry with existing procmond and collector-core
  - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5_

- [ ] 10. Create comprehensive testing suite and performance validation

- [ ] 10.1 Create property-based testing framework for SQL parsing

  - Set up proptest framework with SQL query generators for valid SQLite syntax
  - Implement roundtrip property tests for SQL parsing and AST reconstruction
  - Create property tests for SQL security validation with malicious input generation
  - Add generative testing for predicate pushdown correctness across various SQL patterns
  - Write property tests for query plan equivalence between original and optimized queries
  - _Requirements: All requirements verification_

- [ ] 10.2 Build mock collector framework for integration testing

  - Create `MockCollector` trait with configurable behavior and response patterns
  - Implement `TestHarness` for orchestrating multiple mock collectors with realistic latencies
  - Add mock collector capability simulation with schema registration and task handling
  - Create test data generators for realistic process, network, and filesystem events
  - Write integration test utilities for collector failure simulation and recovery validation
  - _Requirements: All requirements verification_

- [ ] 10.3 Implement end-to-end integration tests for complex scenarios

  - Create integration tests for multi-collector SQL queries with JOIN operations
  - Add tests for reactive pipeline orchestration with cascading YARA and PE analysis
  - Implement integration tests for auto-JOIN trigger scenarios with correlation validation
  - Create tests for error handling and graceful degradation with collector failures
  - Write integration tests for schema registry updates and query plan invalidation
  - _Requirements: All requirements verification_

- [ ] 10.4 Add performance benchmarking with criterion

  - Implement criterion benchmarks for SQL parsing and AST analysis performance
  - Create benchmarks for pushdown planning and query optimization with various complexity levels
  - Add join execution benchmarks comparing INLJ, SHJ, and MRC strategies
  - Implement storage operation benchmarks for redb write throughput and index performance
  - Write collector communication benchmarks for IPC throughput and serialization overhead
  - _Requirements: All requirements verification_

- [ ] 10.5 Create memory usage and resource limit validation tests

  - Implement memory usage tests for bounded join operations with cardinality limits
  - Create resource exhaustion tests for aggregation operations with large group cardinalities
  - Add memory leak detection tests for long-running query execution scenarios
  - Implement timeout validation tests for query cancellation and resource cleanup
  - Write stress tests for concurrent query execution with resource contention
  - _Requirements: All requirements verification_

- [ ] 11. Create documentation and operator guides

- [ ] 11.1 Write SQL dialect reference documentation

  - Document supported SQL constructs (SELECT, FROM, WHERE, JOIN, GROUP BY, HAVING)
  - Create function reference with approved functions and security restrictions
  - Add SQL syntax examples for common detection patterns and use cases
  - Document SQL limitations and unsupported features with alternative approaches
  - Create SQL security guidelines and best practices for rule authors
  - _Requirements: All requirements verification_

- [ ] 11.2 Create technical rule authoring and optimization reference

  - Write technical rule authoring best practices with performance optimization techniques
  - Document pushdown optimization behavior and how to write pushdown-friendly queries
  - Create technical troubleshooting guide for common SQL-to-IPC errors and resolution steps
  - Add performance tuning reference for complex queries with joins and aggregations
  - Create technical example rule library with commented detection patterns for common threats
  - _Requirements: All requirements verification_

- [ ] 11.3 Add collector capability and integration documentation

  - Document collector capability matrix with supported operations and limitations
  - Create collector integration guide for adding new specialty collectors
  - Add schema registry documentation with capability negotiation and updates
  - Document supplemental rule data formats for YARA, network analysis, and PE analysis
  - Create collector development guide with EventSource implementation examples
  - _Requirements: All requirements verification_

- [ ] 11.4 Write system administration and deployment guide

  - Document SQL-to-IPC engine configuration and regex performance tuning
  - Create regex pattern authoring guidelines with performance best practices
  - Add troubleshooting guide for regex timeout and memory violations
  - Document regex monitoring and alerting configuration
  - Create example configurations for different deployment scenarios
  - _Requirements: 13.1, 13.2, 13.3, 13.4, 13.5, 14.1, 14.2, 14.3, 14.4, 14.5_

- [ ] 11.5 Create comprehensive user training tutorials and hands-on rule development guides

  - Write step-by-step interactive tutorial for writing first detection rule with SQL-to-IPC engine
  - Create beginner's hands-on training guide with practical exercises and common detection patterns
  - Add intermediate training tutorial covering JOINs, aggregations, and multi-collector correlation with exercises
  - Create advanced training guide for reactive pipeline orchestration and AUTO JOIN syntax with real scenarios
  - Write interactive regex pattern training tutorial with security best practices and hands-on exercises
  - Add specialty collector integration training tutorial (YARA, network analysis, PE analysis) with practical labs
  - Create rule testing and validation training guide with mock data, test harnesses, and hands-on validation
  - Write performance optimization training tutorial with pushdown strategies, query tuning, and benchmarking exercises
  - Add interactive troubleshooting training for common rule authoring mistakes and debugging techniques
  - Create comprehensive training rule library with real-world detection scenarios, exercises, and guided walkthroughs
  - _Requirements: All requirements verification_

- [ ] 12. Implement regex pattern compilation and caching system with performance bounds

- [ ] 12.1 Create core RegexCompiler with compilation timeout and memory limits

  - Implement `RegexCompiler` struct with configurable timeout and memory limits
  - Add pattern complexity validation to detect exponential backtracking patterns
  - Create compilation with timeout enforcement using thread-based execution
  - Implement memory usage estimation and validation before compilation
  - Write unit tests for compilation timeout, memory limits, and complexity validation
  - _Requirements: 13.1, 13.4, 14.1, 14.2, 14.3_

- [ ] 12.2 Build LRU cache for compiled regex patterns with performance monitoring

  - Implement thread-safe LRU cache for `CompiledRegex` objects with configurable size limits
  - Add cache hit/miss tracking and performance metrics collection
  - Create cache eviction policies based on pattern usage and age
  - Implement cache statistics reporting for monitoring and debugging
  - Write unit tests for cache operations, eviction, and thread safety
  - _Requirements: 13.2, 13.5, 14.5_

- [ ] 12.3 Add regex execution with per-pattern timeout enforcement

  - Implement `execute_pattern` method with configurable per-pattern execution timeouts
  - Add thread-based timeout enforcement to prevent catastrophic backtracking
  - Create performance statistics tracking for execution time and violation counts
  - Implement graceful timeout handling with detailed error reporting
  - Write unit tests for execution timeout, performance tracking, and error handling
  - _Requirements: 13.3, 13.5, 14.1, 14.4_

- [ ] 12.4 Create comprehensive regex performance monitoring and alerting

  - Implement `RegexMetrics` with atomic counters for cache hits, timeouts, and slow executions
  - Add performance threshold monitoring with configurable alert thresholds
  - Create structured logging for regex performance violations and debugging
  - Implement metrics export for Prometheus integration and monitoring dashboards
  - Write unit tests for metrics collection, threshold detection, and alerting
  - _Requirements: 13.5, 14.4, 14.5_

- [ ] 12.5 Integrate regex compiler with SQL parser and AST analyzer

  - Modify `SqlAnalyzer` to extract REGEXP operators and compile patterns during rule parsing
  - Add `CompiledRegexPattern` to `DetectionPlan` with column references and performance stats
  - Create regex pattern validation during SQL rule loading with clear error messages
  - Implement pattern precompilation for frequently used rules and rule packs
  - Write integration tests for SQL parsing with regex compilation and validation
  - _Requirements: 13.1, 13.2, 13.4, 13.5_

- [ ] 12.6 Add regex pattern complexity analysis and safety validation

  - Implement pattern complexity scoring based on quantifiers, alternation, and lookarounds
  - Create safety validation to reject patterns with exponential complexity characteristics
  - Add pattern length limits and nested quantifier detection for ReDoS prevention
  - Implement pattern rewriting suggestions for unsafe patterns when possible
  - Write comprehensive tests for complexity analysis with OWASP ReDoS test vectors
  - _Requirements: 13.4, 14.3, 14.4_

- [ ] 12.7 Create regex configuration management and runtime tuning

  - Implement `RegexConfig` with hierarchical configuration loading (defaults, files, env, CLI)
  - Add runtime configuration updates for timeout limits and cache sizes without restart
  - Create configuration validation with reasonable bounds and security constraints
  - Implement configuration export and import for deployment standardization
  - Write unit tests for configuration loading, validation, and runtime updates
  - _Requirements: 14.1, 14.2, 14.3, 14.5_

- [ ] 12.8 Build comprehensive regex testing framework with property-based tests

  - Create property-based tests using proptest for regex compilation and execution correctness
  - Implement timeout violation tests with patterns designed to trigger backtracking
  - Add memory exhaustion tests with large pattern sets and cache pressure scenarios
  - Create performance regression tests with baseline timing and memory usage validation
  - Write chaos testing for concurrent compilation, execution, and cache operations
  - _Requirements: 13.1, 13.2, 13.3, 13.4, 13.5_

- [ ] 12.9 Add regex performance benchmarking with criterion and regression detection

  - Implement criterion benchmarks for regex compilation time across pattern complexity levels
  - Create execution time benchmarks for various pattern types and text sizes
  - Add cache performance benchmarks for hit rates and lookup latency under load
  - Implement memory usage benchmarks for pattern compilation and cache overhead
  - Write benchmark regression detection with automated performance alerts in CI
  - _Requirements: 13.2, 13.3, 13.5, 14.5_

- [ ] 12.10 Create regex pattern optimization and recommendation system

  - Implement pattern analysis to suggest performance optimizations for slow patterns
  - Add automatic pattern rewriting for common inefficient constructs when safe
  - Create pattern performance profiling with execution time breakdown and bottleneck identification
  - Implement pattern usage analytics to identify frequently used patterns for precompilation
  - Write unit tests for pattern optimization, rewriting, and performance profiling
  - _Requirements: 13.5, 14.4, 14.5_ options and tuning parameters
  - Create deployment guide for multi-collector environments with capacity planning
  - Add monitoring setup guide with metrics collection and alerting configuration
  - Document backup and recovery procedures for redb storage and configuration
  - Create troubleshooting runbook for production issues and performance optimization
  - _Requirements: All requirements verification_
