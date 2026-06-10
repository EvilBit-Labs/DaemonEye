# Requirements Document

## Introduction

DaemonEye is a security-focused, high-performance process monitoring system designed to detect suspicious activity on systems through continuous process monitoring, behavioral analysis, and pattern recognition. The system provides security operations teams with a reliable, high-performance threat detection solution that operates independently of external dependencies while maintaining audit-grade integrity.

This specification covers the core monitoring functionality including the collector-core framework, process enumeration, detection rule execution, and alert generation across the three-component architecture: procmond (privileged collector), daemoneye-agent (detection orchestrator), and daemoneye-cli (operator interface). The collector-core framework enables extensible monitoring capabilities across multiple domains (process, network, filesystem, performance) while maintaining shared infrastructure and operational consistency.

## Priority Tiers

Decided 2026-06-09 to sequence solo-maintainer effort toward OSS v1.0.0:

| Tier                    | Requirements  | Meaning                                                                              |
| ----------------------- | ------------- | ------------------------------------------------------------------------------------ |
| Core v1.0               | R1–R10        | Must be satisfied for the v1.0.0 release                                             |
| Enabling infrastructure | R11, R14      | Framework/eventbus work that core monitoring depends on                              |
| Future (post-v1.0)      | R12, R15, R16 | Multi-collector and triggered-analysis extensions; interface contracts only for v1.0 |
| Design constraint       | R13           | Open-core boundary properties verified by conformance tests, not a deliverable       |

## Requirements

### Requirement 1

**User Story:** As a security operations analyst, I want to continuously monitor all system processes with minimal performance impact, so that I can detect suspicious activity without degrading system performance.

#### Acceptance Criteria

1. WHEN the system starts THEN procmond SHALL enumerate all accessible processes within 5 seconds for systems with up to 10,000 processes
2. WHEN collecting process data THEN the system SHALL maintain less than 5% sustained CPU usage during continuous monitoring
3. WHEN enumerating processes THEN the system SHALL collect minimum metadata including PID, PPID, name, executable path, and command line arguments
4. WHEN enhanced privileges are available THEN the system SHALL collect additional metadata including memory usage, CPU percentage, and start time
5. WHEN a process is inaccessible THEN the system SHALL log the issue and continue enumeration without failing
6. WHEN storing command line arguments THEN the system SHALL apply argument sanitization by default, redacting values following known sensitive flags (e.g., --password, --token, --api-key), with unmasked collection available only via explicit operator configuration

### Requirement 2

**User Story:** As a security engineer, I want to verify executable integrity through cryptographic hashing, so that I can detect tampered or malicious binaries.

#### Acceptance Criteria

1. WHEN enumerating processes THEN the system SHALL compute SHA-256 hashes for accessible executable files
2. WHEN an executable file is missing or inaccessible THEN the system SHALL record this state without failing the enumeration
3. WHEN storing process data THEN the system SHALL include hash_algorithm field set to 'sha256' with the computed hash value
4. WHEN hashing executables THEN hashing SHALL run asynchronously outside the Requirement 1 enumeration deadline, SHALL reuse cached hashes for unchanged executables, and SHALL keep total sustained CPU within the Requirement 1 budget
5. WHEN system processes restrict file access THEN the system SHALL skip hashing gracefully and continue processing
6. WHEN the running process image differs from the on-disk executable (deleted or replaced executable) THEN the system SHALL record this mismatch state as distinct metadata; the executable hash attests on-disk state at collection time, not the executing image
7. WHEN computing executable hashes THEN the system SHALL compute a SHA-256 identity hash and SHOULD compute an ssdeep fuzzy hash stored alongside it; WHEN a process executable's fuzzy-hash similarity to its previously recorded value falls below a configurable threshold THEN the system SHALL record a binary-change observation

### Requirement 3

**User Story:** As a threat hunter, I want to execute SQL-based detection rules against process data, so that I can identify suspicious patterns and behaviors using flexible queries.

#### Acceptance Criteria

1. WHEN loading detection rules THEN the system SHALL parse and validate SQL queries using AST validation (sqlparser) to prevent injection attacks
2. WHEN validating SQL THEN the system SHALL only allow SELECT statements with approved functions (COUNT, SUM, AVG, MIN, MAX, LENGTH, SUBSTR, datetime functions)
3. WHEN executing queries THEN the system SHALL execute only the derived, load-time-validated query representation against a read-only view of the event store; the original SQL dialect SHALL never reach the execution layer
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
6. WHEN alerts are generated THEN the system SHALL publish them to the embedded daemoneye-eventbus broker on the alerts topic; configured alert sinks SHALL consume alerts from the broker for delivery, enabling multi-consumer dispatch

### Requirement 6

**User Story:** As a system administrator, I want the monitoring system to operate with minimal privileges and drop them after initialization, so that the security risk is minimized.

#### Acceptance Criteria

1. WHEN starting procmond THEN the system SHALL request only minimal required privileges for process enumeration
2. WHEN enhanced access is configured THEN the system SHALL optionally request platform-specific privileges (CAP_SYS_PTRACE on Linux, SeDebugPrivilege on Windows)
3. WHEN initialization completes THEN procmond SHALL drop all privileges except the declared minimum platform-specific set required for steady-state collection, logging the retained set for the audit trail
4. WHEN operating post-initialization THEN daemoneye-agent and daemoneye-cli SHALL run as dedicated non-root users; procmond SHALL hold only its declared retained capability set
5. WHEN privilege operations occur THEN the system SHALL log all privilege changes for audit trail

### Requirement 7

**User Story:** As a compliance officer, I want all security-relevant events recorded in a tamper-evident audit ledger, so that I can verify system integrity for forensic analysis.

#### Acceptance Criteria

1. WHEN security events occur THEN the system SHALL record them in an append-only audit ledger with monotonic sequence numbers
2. WHEN creating audit entries THEN the system SHALL include timestamp, actor, action, payload hash, previous hash, and entry hash
3. WHEN computing hashes THEN the system SHALL use BLAKE3 for fast, cryptographically secure hash computation
4. WHEN verifying integrity THEN the system SHALL provide chain verification function to detect tampering
5. WHEN audit events are recorded THEN the system SHALL maintain proper ordering and millisecond-precision timestamps
6. WHEN audit entries are committed THEN the system SHALL periodically produce Ed25519-signed checkpoints over the chain head, exportable for off-host anchoring, so that chain rewrites by a host-resident adversary are detectable

### Requirement 8

**User Story:** As an operator, I want a command-line interface to query historical data and manage the system, so that I can investigate incidents and maintain the monitoring system.

#### Acceptance Criteria

1. WHEN querying data THEN daemoneye-cli SHALL execute user-provided SQL queries through the same AST validation and SELECT-only allowlist as detection rules (Requirement 3), with all user values bound as parameters rather than interpolated
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
5. WHEN importing bundles THEN the system SHALL verify the bundle's Ed25519 signature against configured trusted keys, reject unsigned or signature-invalid bundles, and apply valid bundles atomically with conflict resolution

### Requirement 10

**User Story:** As a DevOps engineer, I want the system to provide comprehensive observability and health monitoring, so that I can maintain operational visibility and integrate with existing monitoring infrastructure.

#### Acceptance Criteria

1. WHEN logging events THEN the system SHALL use structured JSON format with consistent field naming and configurable log levels
2. WHEN exposing metrics THEN the system SHALL provide Prometheus-compatible metrics for collection rate, detection latency, and alert delivery via the local CLI/IPC path or textfile export by default; any HTTP metrics listener SHALL be opt-in, loopback-only by default, and documented as a stealth and attack-surface tradeoff
3. WHEN monitoring health THEN the system SHALL expose component-level health via daemoneye-cli over the local IPC/database path; any HTTP health endpoint SHALL be opt-in and disabled by default
4. WHEN tracking performance THEN the system SHALL embed performance metrics in log entries with correlation IDs
5. WHEN integrating with monitoring systems THEN the system SHALL respect NO_COLOR and TERM=dumb environment variables for console output

### Requirement 11

**User Story:** As a system architect, I want a reusable collector-core framework that enables multiple collection components, so that I can extend monitoring capabilities across different domains while maintaining shared infrastructure.

#### Acceptance Criteria

1. WHEN creating collection components THEN the system SHALL provide a universal EventSource trait that abstracts collection methodology from operational infrastructure
2. WHEN registering event sources THEN the collector-core SHALL support multiple concurrent sources with unified event processing and daemoneye-eventbus communication
3. WHEN handling different event types THEN the system SHALL support extensible CollectionEvent enum covering process, network, filesystem, and performance domains
4. WHEN managing component lifecycle THEN the collector-core SHALL provide consistent start/stop, health checks, and graceful shutdown across all registered sources
5. WHEN configuring components THEN the system SHALL share common configuration loading, validation, and environment handling across all collection types

### Requirement 12

**User Story:** As a platform developer, I want the collector-core framework to enable future monitoring components like network, filesystem, and performance monitoring, so that I can build comprehensive behavioral analysis capabilities.

#### Acceptance Criteria

1. WHEN implementing new collection components THEN the system SHALL provide shared IPC server logic, configuration management, and logging infrastructure through collector-core
2. WHEN developing network monitoring THEN the collector-core SDK SHALL expose the NetworkEvent variant and EventSource trait surface such that a future network collector (connection tracking, traffic analysis, DNS monitoring) CAN be built without modifying existing components
3. WHEN implementing filesystem monitoring THEN the collector-core SDK SHALL expose the FilesystemEvent variant and EventSource trait surface such that a future filesystem collector (file operations, access patterns, bulk operation detection) CAN be built without modifying existing components
4. WHEN adding performance monitoring THEN the collector-core SDK SHALL expose the PerformanceEvent variant and EventSource trait surface such that a future performance collector (resource utilization, system metrics, anomaly detection) CAN be built without modifying existing components
5. WHEN correlating multi-domain events THEN daemoneye-agent SHALL receive unified event streams from multiple collection components for behavioral analysis

### Requirement 13

**User Story:** As a system architect, I want the collector-core framework to enable shared infrastructure between OSS and enterprise features, so that both versions can leverage the same proven operational foundation while supporting different collection capabilities.

#### Acceptance Criteria

1. WHEN managing licensing THEN the collector-core SHALL remain Apache-2.0 licensed while enabling proprietary EventSource implementations
2. WHEN defining the collector-core public API THEN the API SHALL NOT require types defined outside this repository
3. WHEN extending capabilities THEN the system SHALL support capability negotiation that indicates available monitoring features without exposing implementation details
4. WHEN verifying extensibility THEN capability negotiation SHALL be exercised by conformance tests using a mock out-of-tree EventSource implementation

### Requirement 14

**User Story:** As a system architect, I want to migrate from crossbeam-based event bus to daemoneye-eventbus message broker, so that I can leverage industrial-grade IPC capabilities for multi-process communication and future scalability.

#### Acceptance Criteria

1. WHEN migrating event bus infrastructure THEN the system SHALL replace crossbeam channels with daemoneye-eventbus crate for inter-component communication
2. WHEN implementing daemoneye-eventbus integration THEN the system SHALL support embedded broker deployment within daemoneye-agent
3. WHEN establishing message broker capabilities THEN the system SHALL provide pub/sub patterns for event distribution and RPC patterns for control messages
4. WHEN the migration is complete THEN end-to-end event delivery through the broker SHALL meet the existing performance budgets (alert latency < 100ms per rule, sustained CPU < 5%) with no regression versus the recorded pre-migration baseline
5. WHEN operating the message broker THEN the system SHALL support multiple transport layers including in-process channels, Unix domain sockets, and Windows named pipes; TCP transport SHALL be an explicitly non-default, opt-in capability disabled in standard configuration (see the no-inbound-network boundary in AGENTS.md and the IPC ADR)
6. WHEN establishing message broker connections THEN the system SHALL require mutual authentication between broker and collectors (authentication enabled by default), with the credential distributed out-of-band at collector spawn
7. WHEN the daemoneye-eventbus in-process transport meets the Requirement 14 performance criteria THEN the legacy crossbeam event-distribution path SHALL be removed (no dual-bus end state)

### Requirement 15

**User Story:** As a security operations engineer, I want daemoneye-eventbus-based message broker to coordinate between multiple collector processes and the agent, so that I can scale monitoring capabilities across different domains while maintaining centralized control.

#### Acceptance Criteria

1. WHEN coordinating multiple collectors THEN the message broker SHALL route events between procmond, future netmond, fsmond, and perfmond processes via pub/sub topics
2. WHEN managing collector lifecycle THEN daemoneye-agent SHALL use RPC calls through the message broker to start, stop, and monitor collector processes
3. WHEN distributing detection tasks THEN the system SHALL publish tasks to appropriate collector topics based on capability negotiation
4. WHEN aggregating results THEN collectors SHALL publish events to domain-specific topics (events.process.*, events.network.*, etc.) for agent consumption
5. WHEN handling control messages THEN the system SHALL use RPC patterns for health checks, configuration updates, and graceful shutdown coordination
6. WHEN a detection task requires capabilities that no registered collector advertises THEN the system SHALL fail the rule's evaluation with a logged, operator-visible error surfaced via CLI rule/health status rather than silently dropping the task

### Requirement 16

**User Story:** As a platform developer, I want the daemoneye-eventbus message broker to enable easy expansion to additional monitoring and triggered collectors, so that I can build comprehensive behavioral analysis capabilities with minimal integration complexity.

#### Acceptance Criteria

1. WHEN adding new monitoring collectors THEN the daemoneye-eventbus broker SHALL provide standardized pub/sub patterns for seamless integration without modifying existing components
2. WHEN implementing triggered collectors THEN the system SHALL support event-driven activation where monitoring events automatically trigger specialized analysis collectors (illustrative examples: YARA, PE analysis, memory inspection — not core-monitoring deliverables)
3. WHEN coordinating multiple collector types THEN the daemoneye-eventbus broker SHALL enable complex workflows where one collector's results can trigger cascading analysis by other collectors
4. WHEN the system starts THEN static monitor collectors (e.g., procmond) SHALL be registered from configuration; dynamic runtime registration of triggerable analysis collectors is a future extension and SHALL NOT be required for core monitoring
5. WHEN managing collector dependencies THEN the system SHALL provide topic-based coordination that allows collectors to subscribe to relevant event streams and publish results for downstream processing

## Deferred / Open Questions

### From 2026-06-09 design review

- **Executable hash algorithm intent:** **Resolved (2026-06-09):** Dual-hash strategy. SHA-256 stays as the exact identity hash (threat-intel interoperability: VirusTotal/MISP are SHA-256-keyed); ssdeep fuzzy hashing (CTPH) is added for substantial-change detection — the actual goal — via a pure-Rust implementation (e.g., the `fuzzyhash` crate; no FFI, compatible with `unsafe_code = "forbid"`). BLAKE3 remains the audit-ledger/forensic hash (unchanged). See R2 AC7.
- **procmond steady-state privilege model:** **Resolved (2026-06-09):** procmond is the only elevated component and retains only the minimum platform-specific capability set required for steady-state collection (e.g., CAP_DAC_READ_SEARCH / CAP_SYS_PTRACE on Linux when enhanced access is configured; SeDebugPrivilege on Windows), dropping everything else after init; the retained set is expected to evolve as collection features are added and is logged at startup. daemoneye-agent and daemoneye-cli run as dedicated non-root users. See R6 AC3/AC4.
- **R14 migration end-state:** **Resolved (2026-06-09):** Option (a) — full removal. daemoneye-eventbus becomes the single event-routing layer; in-process needs use plain tokio channels. The crossbeam-based `HighPerformanceEventBus` in collector-core is a sanctioned *transitional* hot path only until the eventbus in-process route demonstrably meets the performance budgets (the no-regression criterion in R14 AC4 is the gate), then `high_performance_event_bus.rs` and the crossbeam dependency are removed. See R14 AC7.
- **ShadowHunt trigger→trace requirements:** **Resolved (2026-06-09):** Hunt orchestration and adjudication are managed server-side by the commercial tier (sold separately, not in this repo). The Community tier owns the host-side primitives only: TraceCommand intake, trace_id-stamped focused capture, and RBAC-gated local trace initiation — these will be specified as interface contracts in a future Community requirements increment so the commercial tier can drive them.
- **Priority tiers / v1.0 cut:** **Resolved (2026-06-09):** Tiering adopted as proposed — core v1.0 (R1–R10), enabling infrastructure (R11, R14), future (R12, R15, R16), design constraint (R13). See the Priority Tiers section above.
- **Alert flow vs eventbus:** **Resolved (2026-06-09):** Alerts publish to the embedded daemoneye-eventbus broker on the alerts topic; configured sinks consume from the broker for delivery, enabling multi-consumer dispatch. See R5 AC6.
- **Spec merge (decided 2026-06-09, execution pending):** The `.kiro/upcoming-specs/sql-to-ipc-detection-engine/` triplet will be merged into this spec as part of the pending ADR-0006 rewrite, in a dedicated session/PR. Detection-engine requirements fold in as a numbered group superseding the R3 overlap; design sections are rewritten DataFusion-first; detection tasks slot into the renumbered task sequence with the MVP gate (parse → validate → pushdown → DataFusion execution → agent integration) ahead of speculative work; `upcoming-specs/` is then deleted. The remaining open questions in that spec (scope cuts, AUTO JOIN positioning, rule evaluation model, Sigma interop) are resolved during the merge.
