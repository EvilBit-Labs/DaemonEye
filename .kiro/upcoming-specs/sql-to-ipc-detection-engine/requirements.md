# Requirements Document

## Introduction

The SQL-to-IPC Detection Engine represents a revolutionary approach to security monitoring that bridges the gap between familiar SQL-based detection rules and high-performance, distributed collection systems. This system enables security operators to write detection rules in standard SQL while automatically translating them into efficient, pushdown-optimized collection tasks for specialized collectors.

The engine operates through a two-layer architecture: a Stream Layer for low-latency filtering where collectors push only needed records, and an Operator Pipeline Layer where SQL is parsed into logical plans and executed over redb for complex operations like joins and aggregations. This approach combines the familiarity of SQL with the performance benefits of purpose-built collection systems.

## Requirements

### Requirement 1

**User Story:** As a security analyst, I want to write detection rules using familiar SQL syntax, so that I can leverage my existing query skills without learning proprietary rule languages.

#### Acceptance Criteria

1. WHEN writing detection rules THEN the system SHALL accept standard SQLite dialect syntax for SELECT statements
2. WHEN parsing SQL THEN the system SHALL use sqlparser crate for AST generation and validation
3. WHEN validating queries THEN the system SHALL only allow SELECT statements with approved functions (aggregations, string ops, date/time helpers)
4. WHEN SQL contains forbidden constructs THEN the system SHALL reject the query with detailed error messages
5. WHEN rules reference tables THEN the system SHALL validate against the registered collector schema catalog

### Requirement 2

**User Story:** As a detection engineer, I want the system to automatically optimize my SQL queries by pushing simple operations to collectors, so that I get maximum performance without manual optimization.

#### Acceptance Criteria

1. WHEN analyzing SQL AST THEN the system SHALL identify pushdown-eligible predicates (equality, inequality, LIKE, REGEXP)
2. WHEN generating collection tasks THEN the system SHALL push projections and simple filters to collectors
3. WHEN collectors cannot handle predicates THEN the system SHALL execute them in the agent's operator pipeline
4. WHEN multiple collectors are involved THEN the system SHALL coordinate collection tasks across all required sources
5. WHEN pushdown is not possible THEN the system SHALL gracefully fall back to agent-side processing

### Requirement 3

**User Story:** As a system architect, I want collectors to advertise their capabilities through a schema registry, so that the query planner can make optimal pushdown decisions.

#### Acceptance Criteria

1. WHEN collectors start THEN they SHALL register their schema descriptors with table definitions and column types
2. WHEN advertising capabilities THEN collectors SHALL specify supported pushdown operations (predicates, projections, ordering)
3. WHEN schema changes THEN collectors SHALL update their descriptors and notify the agent
4. WHEN planning queries THEN the agent SHALL validate table references against the current schema catalog
5. WHEN capabilities are insufficient THEN the system SHALL provide clear error messages about unsupported operations

### Requirement 4

**User Story:** As a performance engineer, I want the system to execute complex operations (joins, aggregations) efficiently over redb storage, so that I can handle large datasets without memory exhaustion.

#### Acceptance Criteria

1. WHEN executing joins THEN the system SHALL use bounded memory with configurable cardinality caps
2. WHEN performing aggregations THEN the system SHALL require explicit time windows to prevent unbounded state
3. WHEN memory limits are reached THEN the system SHALL switch join strategies automatically (SHJ to INLJ)
4. WHEN processing large datasets THEN the system SHALL use secondary indexes for efficient lookups
5. WHEN operations exceed limits THEN the system SHALL emit partial results with diagnostic metrics

### Requirement 5

**User Story:** As a security operator, I want automatic correlation between related data sources through JOIN triggers, so that I can get comprehensive analysis without complex rule authoring.

#### Acceptance Criteria

1. WHEN SQL contains JOINs THEN the system SHALL automatically trigger collection of the second half of the join
2. WHEN using AUTO JOIN syntax THEN the system SHALL always collect related data for specified relationships
3. WHEN conditional auto-JOINs are specified THEN the system SHALL only collect when trigger conditions are met
4. WHEN multiple auto-JOINs are defined THEN the system SHALL coordinate collection across all required sources
5. WHEN auto-JOIN collection fails THEN the system SHALL continue with available data and log the failure

### Requirement 6

**User Story:** As a threat hunter, I want to integrate complex pattern matching (YARA, eBPF) with SQL-based rules, so that I can combine behavioral analysis with signature-based detection.

#### Acceptance Criteria

1. WHEN rules require YARA analysis THEN the system SHALL trigger YARA collectors and make results queryable via SQL
2. WHEN using specialty collectors THEN the system SHALL support supplemental rule data (YARA rules, eBPF programs)
3. WHEN combining pattern matching with behavioral data THEN the system SHALL enable JOINs between specialty results and process data
4. WHEN specialty analysis fails THEN the system SHALL continue with available data sources
5. WHEN multiple pattern engines are available THEN the system SHALL coordinate analysis across all engines

### Requirement 7

**User Story:** As a security architect, I want the detection pipeline to operate as a reactive system, so that initial detections can trigger deeper analysis automatically.

#### Acceptance Criteria

1. WHEN initial collectors detect events THEN the system SHALL evaluate trigger rules for additional analysis
2. WHEN JOIN requirements are detected THEN the system SHALL automatically initiate collection of missing data
3. WHEN analysis results arrive THEN the system SHALL re-evaluate rules with the complete dataset
4. WHEN cascading analysis is triggered THEN the system SHALL manage analysis queues with priority ordering
5. WHEN analysis chains become too deep THEN the system SHALL apply circuit breakers to prevent resource exhaustion

### Requirement 8

**User Story:** As a database engineer, I want the system to use redb efficiently with proper indexing and partitioning, so that query performance remains consistent as data volume grows.

#### Acceptance Criteria

1. WHEN storing events THEN the system SHALL partition by time with configurable bucket sizes (hourly/daily)
2. WHEN creating indexes THEN the system SHALL use fixed-width keys for optimal B-tree packing
3. WHEN executing queries THEN the system SHALL use set-based intersections instead of row scans
4. WHEN planning queries THEN the system SHALL select driving indexes based on estimated selectivity
5. WHEN maintaining indexes THEN the system SHALL support hot/cold index management based on rule usage

### Requirement 9

**User Story:** As a reliability engineer, I want comprehensive error handling and recovery mechanisms, so that the system remains operational despite collector failures or data corruption.

#### Acceptance Criteria

1. WHEN collectors disconnect THEN the system SHALL mark affected tables as unavailable and continue with remaining sources
2. WHEN schema mismatches occur THEN the system SHALL request updated descriptors and reconcile differences
3. WHEN data corruption is detected THEN the system SHALL validate checksums and request retransmission
4. WHEN rate limits are exceeded THEN the system SHALL apply backpressure and throttle collection requests
5. WHEN recovery is needed THEN the system SHALL replay missed events from last known good sequence numbers

### Requirement 10

**User Story:** As a security operations manager, I want comprehensive observability into the detection pipeline, so that I can monitor performance and troubleshoot issues effectively.

#### Acceptance Criteria

1. WHEN processing queries THEN the system SHALL emit metrics for rule evaluation latency and match rates
2. WHEN collectors operate THEN the system SHALL track IPC message rates, serialization time, and queue depths
3. WHEN storage operations occur THEN the system SHALL monitor redb write latency and index maintenance time
4. WHEN alerts are generated THEN the system SHALL track delivery success rates and retry counts
5. WHEN debugging is needed THEN the system SHALL provide detailed execution plans and performance breakdowns

### Requirement 11

**User Story:** As a compliance officer, I want all detection activities recorded with full traceability, so that I can audit rule execution and alert generation for forensic analysis.

#### Acceptance Criteria

1. WHEN rules execute THEN the system SHALL record execution metadata with correlation IDs
2. WHEN alerts are generated THEN the system SHALL store pointers to source data used in detection
3. WHEN JOINs are performed THEN the system SHALL optionally record correlation metadata for debugging
4. WHEN data is collected THEN collectors SHALL log collection activities in tamper-evident audit logs
5. WHEN investigations are needed THEN the system SHALL enable reconstruction of detection logic from audit trails

### Requirement 12

**User Story:** As a platform developer, I want the SQL-to-IPC architecture to support future collector types and analysis engines, so that the system can evolve with new security monitoring needs.

#### Acceptance Criteria

1. WHEN new collector types are added THEN the system SHALL support them through the existing schema registry
2. WHEN specialty analysis engines are integrated THEN the system SHALL handle their results through unified interfaces
3. WHEN new SQL constructs are needed THEN the system SHALL support dialect extensions while maintaining compatibility
4. WHEN cross-platform deployment is required THEN the system SHALL adapt to platform-specific collector capabilities
5. WHEN enterprise features are added THEN the system SHALL maintain backward compatibility with existing rule sets

### Requirement 13

**User Story:** As a security engineer, I want regex patterns in SQL rules to be compiled and cached with configurable performance bounds, so that pattern matching operations are fast and secure against ReDoS attacks.

#### Acceptance Criteria

1. WHEN SQL rules contain REGEXP operators THEN the system SHALL compile regex patterns at rule load time with configurable compilation timeouts
2. WHEN regex patterns are compiled THEN the system SHALL cache compiled patterns with LRU eviction and configurable cache size limits
3. WHEN executing regex operations THEN the system SHALL enforce per-pattern execution timeouts to prevent catastrophic backtracking
4. WHEN regex patterns exceed complexity limits THEN the system SHALL reject patterns with clear error messages about performance constraints
5. WHEN monitoring regex performance THEN the system SHALL track compilation time, execution time, memory usage, and cache hit rates

### Requirement 14

**User Story:** As a system administrator, I want configurable regex performance limits and monitoring, so that I can tune pattern matching performance for my deployment's security and performance requirements.

#### Acceptance Criteria

1. WHEN configuring regex limits THEN the system SHALL support per-pattern execution timeout (default: 10ms)
2. WHEN configuring regex limits THEN the system SHALL support pattern compilation timeout (default: 100ms)
3. WHEN configuring regex limits THEN the system SHALL support memory limit per pattern (default: 1MB)
4. WHEN patterns exceed configured limits THEN the system SHALL log performance violations and optionally disable problematic patterns
5. WHEN monitoring regex operations THEN the system SHALL expose metrics for pattern performance, cache effectiveness, and rejection rates
