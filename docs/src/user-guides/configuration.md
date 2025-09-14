# SentinelD Configuration Guide

This guide provides comprehensive information about configuring SentinelD for different deployment scenarios and requirements.

## Table of Contents

- [Configuration Overview](#configuration-overview)
- [Configuration Hierarchy](#configuration-hierarchy)
- [Core Configuration](#core-configuration)
- [Alerting Configuration](#alerting-configuration)
- [Database Configuration](#database-configuration)
- [Platform-Specific Configuration](#platform-specific-configuration)
- [Business Tier Configuration](#business-tier-configuration)
- [Enterprise Tier Configuration](#enterprise-tier-configuration)
- [Environment Variables](#environment-variables)
- [Configuration Examples](#configuration-examples)
- [Troubleshooting](#troubleshooting)

## Configuration Overview

SentinelD uses a hierarchical configuration system that allows you to override settings at different levels. The configuration is loaded in the following order (later sources override earlier ones):

1. **Embedded defaults** (lowest precedence)
2. **System configuration files** (`/etc/sentineld/config.yaml`)
3. **User configuration files** (`~/.config/sentineld/config.yaml`)
4. **Environment variables** (`SENTINELD_*`)
5. **Command-line flags** (highest precedence)

## Configuration Hierarchy

### File Locations

**System Configuration**:

- Linux: `/etc/sentineld/config.yaml`
- macOS: `/Library/Application Support/SentinelD/config.yaml`
- Windows: `C:\ProgramData\SentinelD\config.yaml`

**User Configuration**:

- Linux/macOS: `~/.config/sentineld/config.yaml`
- Windows: `%APPDATA%\SentinelD\config.yaml`

**Service-Specific Configuration**:

- Linux: `/etc/sentineld/procmond.yaml`, `/etc/sentineld/sentinelagent.yaml`
- macOS: `/Library/Application Support/SentinelD/procmond.yaml`
- Windows: `C:\ProgramData\SentinelD\procmond.yaml`

### Configuration Formats

SentinelD supports multiple configuration formats:

- **YAML** (recommended): Human-readable, supports comments
- **JSON**: Machine-readable, no comments
- **TOML**: Alternative human-readable format

## Core Configuration

### Application Settings

```yaml
app:
  # Scan interval in milliseconds
  scan_interval_ms: 30000

  # Batch size for process collection
  batch_size: 1000

  # Log level: debug, info, warn, error
  log_level: info

  # Data retention period in days
  retention_days: 30

  # Maximum memory usage in MB
  max_memory_mb: 512

  # Enable performance monitoring
  enable_metrics: true

  # Metrics collection interval in seconds
  metrics_interval_secs: 60
```

### Process Collection Settings

```yaml
collection:
  # Enable process enumeration
  enable_process_collection: true

  # Enable executable hashing
  enable_hash_computation: true

  # Hash algorithm (sha256, sha1, md5)
  hash_algorithm: sha256

  # Skip hashing for system processes
  skip_system_processes: true

  # Skip hashing for temporary files
  skip_temp_files: true

  # Maximum hash computation time per process (ms)
  max_hash_time_ms: 5000

  # Enable enhanced process metadata collection
  enable_enhanced_metadata: false
```

### Detection Engine Settings

```yaml
detection:
  # Path to detection rules directory
  rules_path: /etc/sentineld/rules

  # Enable rule hot-reloading
  enable_hot_reload: true

  # Rule execution timeout in seconds
  rule_timeout_secs: 30

  # Maximum memory per rule execution (MB)
  max_rule_memory_mb: 128

  # Enable rule performance monitoring
  enable_rule_metrics: true

  # Rule execution concurrency
  max_concurrent_rules: 10

  # Enable rule validation
  enable_rule_validation: true
```

## Alerting Configuration

### Alert Sinks

```yaml
alerting:
  # Enable alerting
  enabled: true

  # Alert deduplication window in minutes
  dedupe_window_minutes: 60

  # Maximum alert queue size
  max_queue_size: 10000

  # Alert processing concurrency
  max_concurrent_deliveries: 5

  # Sink configurations
  sinks:
    # Standard output sink
    - type: stdout
      enabled: true
      format: json    # json, text, csv

    # File output sink
    - type: file
      enabled: false
      path: /var/log/sentineld/alerts.json
      format: json
      rotation:
        max_size_mb: 100
        max_files: 10

    # Syslog sink
    - type: syslog
      enabled: true
      facility: daemon
      tag: sentineld
      host: localhost
      port: 514
      protocol: udp    # udp, tcp

    # Webhook sink
    - type: webhook
      enabled: false
      url: https://your-siem.com/webhook
      method: POST
      headers:
        Authorization: Bearer ${WEBHOOK_TOKEN}
        Content-Type: application/json
      timeout_secs: 30
      retry_attempts: 3
      retry_delay_ms: 1000

    # Email sink
    - type: email
      enabled: false
      smtp_host: smtp.example.com
      smtp_port: 587
      smtp_username: ${SMTP_USERNAME}
      smtp_password: ${SMTP_PASSWORD}
      smtp_tls: true
      from: sentineld@example.com
      to: [security@example.com]
      subject: 'SentinelD Alert: {severity} - {title}'

    # Splunk HEC sink (Business Tier)
    - type: splunk_hec
      enabled: false
      endpoint: https://splunk.example.com:8088/services/collector
      token: ${SPLUNK_HEC_TOKEN}
      index: sentineld
      source_type: sentineld:alert
      sourcetype: sentineld:alert

    # Elasticsearch sink (Business Tier)
    - type: elasticsearch
      enabled: false
      hosts: [https://elastic.example.com:9200]
      username: ${ELASTIC_USERNAME}
      password: ${ELASTIC_PASSWORD}
      index_pattern: sentineld-{YYYY.MM.DD}
      pipeline: sentineld-alerts

    # Kafka sink (Business Tier)
    - type: kafka
      enabled: false
      brokers: [kafka.example.com:9092]
      topic: sentineld.alerts
      security_protocol: SASL_SSL
      sasl_mechanism: PLAIN
      sasl_username: ${KAFKA_USERNAME}
      sasl_password: ${KAFKA_PASSWORD}
```

### Alert Filtering

```yaml
alerting:
  # Global alert filters
  filters:
    # Minimum severity level
    min_severity: low    # low, medium, high, critical

    # Exclude specific rules
    exclude_rules: [test-rule, debug-rule]

    # Include only specific rules
    include_rules: []  # Empty means all rules

    # Exclude specific hosts
    exclude_hosts: [test-server, dev-workstation]

    # Include only specific hosts
    include_hosts: []  # Empty means all hosts

    # Time-based filtering
    time_filters:
      # Exclude alerts during maintenance windows
      maintenance_windows:
        - start: 02:00
          end: 04:00
          days: [sunday]
        - start: 12:00
          end: 13:00
          days: [monday, tuesday, wednesday, thursday, friday]
```

## Database Configuration

### Event Store (redb)

```yaml
database:
  # Event store configuration
  event_store:
    # Database file path
    path: /var/lib/sentineld/events.redb

    # Maximum database size in MB
    max_size_mb: 10240

    # Enable WAL mode for better performance
    wal_mode: true

    # WAL checkpoint interval in seconds
    wal_checkpoint_interval_secs: 300

    # Connection pool size
    max_connections: 10

    # Connection timeout in seconds
    connection_timeout_secs: 30

    # Idle connection timeout in seconds
    idle_timeout_secs: 600
```

### Audit Ledger (SQLite)

```yaml
database:
  # Audit ledger configuration
  audit_ledger:
    # Database file path
    path: /var/lib/sentineld/audit.sqlite

    # Enable WAL mode for durability
    wal_mode: true

    # WAL checkpoint mode (NORMAL, FULL, RESTART, TRUNCATE)
    wal_checkpoint_mode: FULL

    # Synchronous mode (OFF, NORMAL, FULL)
    synchronous: FULL

    # Journal mode (DELETE, TRUNCATE, PERSIST, MEMORY, WAL)
    journal_mode: WAL

    # Cache size in KB
    cache_size_kb: 2000

    # Page size in bytes
    page_size_bytes: 4096
```

### Data Retention

```yaml
database:
  # Data retention policies
  retention:
    # Process data retention in days
    process_data_days: 30

    # Alert data retention in days
    alert_data_days: 90

    # Audit log retention in days
    audit_log_days: 365

    # Enable automatic cleanup
    enable_cleanup: true

    # Cleanup interval in hours
    cleanup_interval_hours: 24

    # Cleanup batch size
    cleanup_batch_size: 1000
```

## Platform-Specific Configuration

### Linux Configuration

```yaml
platform:
  linux:
    # Enable eBPF monitoring (Enterprise Tier)
    enable_ebpf: false

    # eBPF program path
    ebpf_program_path: /usr/lib/sentineld/sentinel_monitor.o

    # eBPF ring buffer size
    ebpf_ring_buffer_size: 1048576  # 1MB

    # Enable process namespace monitoring
    enable_namespace_monitoring: true

    # Enable cgroup monitoring
    enable_cgroup_monitoring: true

    # Process collection method
    collection_method: sysinfo    # sysinfo, ebpf, hybrid

    # Privilege requirements
    privileges:
      # Required capabilities
      capabilities: [SYS_PTRACE]

      # Drop privileges after initialization
      drop_privileges: true

      # Privilege drop timeout in seconds
      privilege_drop_timeout_secs: 30
```

### Windows Configuration

```yaml
platform:
  windows:
    # Enable ETW monitoring (Enterprise Tier)
    enable_etw: false

    # ETW session name
    etw_session_name: SentinelD

    # ETW buffer size in KB
    etw_buffer_size_kb: 64

    # ETW maximum buffers
    etw_max_buffers: 100

    # Enable registry monitoring
    enable_registry_monitoring: false

    # Enable file system monitoring
    enable_filesystem_monitoring: false

    # Process collection method
    collection_method: sysinfo    # sysinfo, etw, hybrid

    # Privilege requirements
    privileges:
      # Required privileges
      privileges: [SeDebugPrivilege]

      # Drop privileges after initialization
      drop_privileges: true
```

### macOS Configuration

```yaml
platform:
  macos:
    # Enable EndpointSecurity monitoring (Enterprise Tier)
    enable_endpoint_security: false

    # EndpointSecurity event types
    es_event_types:
      - ES_EVENT_TYPE_NOTIFY_EXEC
      - ES_EVENT_TYPE_NOTIFY_FORK
      - ES_EVENT_TYPE_NOTIFY_EXIT

    # Enable file system monitoring
    enable_filesystem_monitoring: false

    # Enable network monitoring
    enable_network_monitoring: false

    # Process collection method
    collection_method: sysinfo    # sysinfo, endpoint_security, hybrid

    # Privilege requirements
    privileges:
      # Required entitlements
      entitlements: [com.apple.security.cs.allow-jit]

      # Drop privileges after initialization
      drop_privileges: true
```

## Business Tier Configuration

### Security Center

```yaml
business_tier:
  # License configuration
  license:
    # License key
    key: ${SENTINELD_LICENSE_KEY}

    # License validation endpoint (optional)
    validation_endpoint:

    # Offline validation only
    offline_only: true

  # Security Center configuration
  security_center:
    # Enable Security Center
    enabled: false

    # Security Center endpoint
    endpoint: https://security-center.example.com:8443

    # Client certificate path
    client_cert_path: /etc/sentineld/agent.crt

    # Client key path
    client_key_path: /etc/sentineld/agent.key

    # CA certificate path
    ca_cert_path: /etc/sentineld/ca.crt

    # Connection timeout in seconds
    connection_timeout_secs: 30

    # Heartbeat interval in seconds
    heartbeat_interval_secs: 30

    # Retry configuration
    retry:
      max_attempts: 3
      base_delay_ms: 1000
      max_delay_ms: 30000
      backoff_multiplier: 2.0
```

### Rule Packs

```yaml
business_tier:
  # Rule pack configuration
  rule_packs:
    # Enable automatic updates
    auto_update: true

    # Update interval in hours
    update_interval_hours: 24

    # Rule pack sources
    sources:
      - name: official
        url: https://rules.sentineld.com/packs/
        signature_key: ed25519:public-key
        enabled: true

      - name: custom
        url: https://internal-rules.company.com/
        signature_key: ed25519:custom-key
        enabled: true

    # Local rule pack directory
    local_directory: /etc/sentineld/rule-packs

    # Signature validation
    signature_validation:
      enabled: true
      strict_mode: true
      allowed_keys: [ed25519:official-key, ed25519:custom-key]
```

### Enhanced Connectors

```yaml
business_tier:
  # Enhanced output connectors
  enhanced_connectors:
    # Splunk HEC connector
    splunk_hec:
      enabled: false
      endpoint: https://splunk.example.com:8088/services/collector
      token: ${SPLUNK_HEC_TOKEN}
      index: sentineld
      source_type: sentineld:alert
      sourcetype: sentineld:alert
      batch_size: 100
      batch_timeout_ms: 5000

    # Elasticsearch connector
    elasticsearch:
      enabled: false
      hosts: [https://elastic.example.com:9200]
      username: ${ELASTIC_USERNAME}
      password: ${ELASTIC_PASSWORD}
      index_pattern: sentineld-{YYYY.MM.DD}
      pipeline: sentineld-alerts
      batch_size: 1000
      batch_timeout_ms: 10000

    # Kafka connector
    kafka:
      enabled: false
      brokers: [kafka.example.com:9092]
      topic: sentineld.alerts
      security_protocol: SASL_SSL
      sasl_mechanism: PLAIN
      sasl_username: ${KAFKA_USERNAME}
      sasl_password: ${KAFKA_PASSWORD}
      batch_size: 100
      batch_timeout_ms: 5000
```

## Enterprise Tier Configuration

### Kernel Monitoring

```yaml
enterprise_tier:
  # Kernel monitoring configuration
  kernel_monitoring:
    # Enable kernel monitoring
    enabled: false

    # Monitoring method
    method: auto    # auto, ebpf, etw, endpoint_security, disabled

    # eBPF configuration (Linux)
    ebpf:
      enabled: false
      program_path: /usr/lib/sentineld/sentinel_monitor.o
      ring_buffer_size: 2097152  # 2MB
      max_events_per_second: 10000

    # ETW configuration (Windows)
    etw:
      enabled: false
      session_name: SentinelD
      buffer_size_kb: 128
      max_buffers: 200
      providers:
        - name: Microsoft-Windows-Kernel-Process
          guid: 22FB2CD6-0E7B-422B-A0C7-2FAD1FD0E716
          level: 5
          keywords: 0xFFFFFFFFFFFFFFFF

    # EndpointSecurity configuration (macOS)
    endpoint_security:
      enabled: false
      event_types:
        - ES_EVENT_TYPE_NOTIFY_EXEC
        - ES_EVENT_TYPE_NOTIFY_FORK
        - ES_EVENT_TYPE_NOTIFY_EXIT
        - ES_EVENT_TYPE_NOTIFY_OPEN
        - ES_EVENT_TYPE_NOTIFY_CLOSE
```

### Federation

```yaml
enterprise_tier:
  # Federation configuration
  federation:
    # Enable federation
    enabled: false

    # Federation tier
    tier: agent    # agent, regional, primary

    # Regional Security Center
    regional_center:
      endpoint: https://regional-center.example.com:8443
      certificate_path: /etc/sentineld/regional.crt
      key_path: /etc/sentineld/regional.key

    # Primary Security Center
    primary_center:
      endpoint: https://primary-center.example.com:8443
      certificate_path: /etc/sentineld/primary.crt
      key_path: /etc/sentineld/primary.key

    # Data synchronization
    sync:
      # Sync interval in minutes
      interval_minutes: 5

      # Sync batch size
      batch_size: 1000

      # Enable compression
      compression: true

      # Enable encryption
      encryption: true
```

### STIX/TAXII Integration

```yaml
enterprise_tier:
  # STIX/TAXII configuration
  stix_taxii:
    # Enable STIX/TAXII integration
    enabled: false

    # TAXII servers
    servers:
      - name: threat-intel-server
        url: https://threat-intel.example.com/taxii2/
        username: ${TAXII_USERNAME}
        password: ${TAXII_PASSWORD}
        collections: [malware-indicators, attack-patterns]

    # Polling configuration
    polling:
      # Poll interval in minutes
      interval_minutes: 60

      # Maximum indicators per poll
      max_indicators: 10000

      # Indicator confidence threshold
      min_confidence: 50

    # Indicator conversion
    conversion:
      # Convert STIX indicators to detection rules
      auto_convert: true

      # Rule template for converted indicators
      rule_template: stix-indicator-{id}

      # Rule severity mapping
      severity_mapping:
        low: low
        medium: medium
        high: high
        critical: critical
```

## Environment Variables

### Core Variables

```bash
# Application settings
export SENTINELD_LOG_LEVEL=info
export SENTINELD_SCAN_INTERVAL_MS=30000
export SENTINELD_BATCH_SIZE=1000
export SENTINELD_RETENTION_DAYS=30

# Database settings
export SENTINELD_DATABASE_PATH=/var/lib/sentineld/events.redb
export SENTINELD_AUDIT_LEDGER_PATH=/var/lib/sentineld/audit.sqlite

# Alerting settings
export SENTINELD_ALERTING_ENABLED=true
export SENTINELD_WEBHOOK_URL=https://your-siem.com/webhook
export SENTINELD_WEBHOOK_TOKEN=your-webhook-token

# Platform settings
export SENTINELD_ENABLE_EBPF=false
export SENTINELD_ENABLE_ETW=false
export SENTINELD_ENABLE_ENDPOINT_SECURITY=false
```

### Business Tier Variables

```bash
# Security Center
export SENTINELD_SECURITY_CENTER_ENABLED=false
export SENTINELD_SECURITY_CENTER_ENDPOINT=https://security-center.example.com:8443
export SENTINELD_CLIENT_CERT_PATH=/etc/sentineld/agent.crt
export SENTINELD_CLIENT_KEY_PATH=/etc/sentineld/agent.key

# Enhanced connectors
export SPLUNK_HEC_TOKEN=your-splunk-token
export ELASTIC_USERNAME=your-elastic-username
export ELASTIC_PASSWORD=your-elastic-password
export KAFKA_USERNAME=your-kafka-username
export KAFKA_PASSWORD=your-kafka-password
```

### Enterprise Tier Variables

```bash
# Kernel monitoring
export SENTINELD_KERNEL_MONITORING_ENABLED=false
export SENTINELD_EBPF_ENABLED=false
export SENTINELD_ETW_ENABLED=false
export SENTINELD_ENDPOINT_SECURITY_ENABLED=false

# Federation
export SENTINELD_FEDERATION_ENABLED=false
export SENTINELD_REGIONAL_CENTER_ENDPOINT=https://regional.example.com:8443

# STIX/TAXII
export TAXII_USERNAME=your-taxii-username
export TAXII_PASSWORD=your-taxii-password
```

## Configuration Examples

### Basic Production Configuration

```yaml
# /etc/sentineld/config.yaml
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info
  retention_days: 30
  enable_metrics: true

collection:
  enable_process_collection: true
  enable_hash_computation: true
  hash_algorithm: sha256
  skip_system_processes: true

detection:
  rules_path: /etc/sentineld/rules
  enable_hot_reload: true
  rule_timeout_secs: 30
  max_concurrent_rules: 10

alerting:
  enabled: true
  dedupe_window_minutes: 60
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
      tag: sentineld
    - type: webhook
      enabled: true
      url: https://your-siem.com/webhook
      headers:
        Authorization: Bearer ${WEBHOOK_TOKEN}

database:
  event_store:
    path: /var/lib/sentineld/events.redb
    max_size_mb: 10240
    wal_mode: true
  audit_ledger:
    path: /var/lib/sentineld/audit.sqlite
    wal_mode: true
    synchronous: FULL
```

### High-Performance Configuration

```yaml
# /etc/sentineld/config.yaml
app:
  scan_interval_ms: 15000  # More frequent scanning
  batch_size: 2000         # Larger batches
  log_level: warn          # Less verbose logging
  retention_days: 7        # Shorter retention
  max_memory_mb: 1024      # More memory
  enable_metrics: true

collection:
  enable_process_collection: true
  enable_hash_computation: true
  hash_algorithm: sha256
  skip_system_processes: true
  max_hash_time_ms: 2000   # Faster hash computation

detection:
  rules_path: /etc/sentineld/rules
  enable_hot_reload: true
  rule_timeout_secs: 15    # Faster rule execution
  max_concurrent_rules: 20 # More concurrent rules
  max_rule_memory_mb: 64   # Less memory per rule

alerting:
  enabled: true
  dedupe_window_minutes: 30
  max_concurrent_deliveries: 10
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
      tag: sentineld
    - type: kafka
      enabled: true
      brokers: [kafka.example.com:9092]
      topic: sentineld.alerts
      batch_size: 100
      batch_timeout_ms: 1000

database:
  event_store:
    path: /var/lib/sentineld/events.redb
    max_size_mb: 20480
    wal_mode: true
    wal_checkpoint_interval_secs: 60
    max_connections: 20
  retention:
    process_data_days: 7
    alert_data_days: 30
    enable_cleanup: true
    cleanup_interval_hours: 6
```

### Airgapped Environment Configuration

```yaml
# /etc/sentineld/config.yaml
app:
  scan_interval_ms: 60000  # Less frequent scanning
  batch_size: 500          # Smaller batches
  log_level: info
  retention_days: 90       # Longer retention
  enable_metrics: true

collection:
  enable_process_collection: true
  enable_hash_computation: true
  hash_algorithm: sha256
  skip_system_processes: true

detection:
  rules_path: /etc/sentineld/rules
  enable_hot_reload: false  # Disable hot reload
  rule_timeout_secs: 60
  max_concurrent_rules: 5

alerting:
  enabled: true
  dedupe_window_minutes: 120
  sinks:
    - type: file
      enabled: true
      path: /var/log/sentineld/alerts.json
      format: json
      rotation:
        max_size_mb: 50
        max_files: 20
    - type: syslog
      enabled: true
      facility: daemon
      tag: sentineld

database:
  event_store:
    path: /var/lib/sentineld/events.redb
    max_size_mb: 5120
    wal_mode: true
  audit_ledger:
    path: /var/lib/sentineld/audit.sqlite
    wal_mode: true
    synchronous: FULL
    journal_mode: WAL
```

## Troubleshooting

### Configuration Validation

```bash
# Validate configuration file
sentinelcli config validate /etc/sentineld/config.yaml

# Validate current configuration
sentinelcli config validate

# Check for configuration issues
sentinelcli config check

# Show effective configuration
sentinelcli config show --include-defaults
```

### Common Configuration Issues

**Invalid YAML Syntax**:

```bash
# Check YAML syntax
python -c "import yaml; yaml.safe_load(open('/etc/sentineld/config.yaml'))"

# Use online YAML validator
# https://www.yamllint.com/
```

**Missing Required Fields**:

```bash
# Check for missing required fields
sentinelcli config check --strict

# Show configuration with defaults
sentinelcli config show --include-defaults
```

**Permission Issues**:

```bash
# Check file permissions
ls -la /etc/sentineld/config.yaml
ls -la /var/lib/sentineld/

# Fix permissions
sudo chown sentineld:sentineld /var/lib/sentineld/
sudo chmod 755 /var/lib/sentineld/
```

**Environment Variable Issues**:

```bash
# Check environment variables
env | grep SENTINELD

# Test environment variable substitution
sentinelcli config show --environment
```

### Configuration Debugging

**Enable Debug Logging**:

```yaml
app:
  log_level: debug
```

**Configuration Loading Debug**:

```bash
# Show configuration loading process
sentinelcli config debug

# Show configuration sources
sentinelcli config sources
```

**Test Configuration Changes**:

```bash
# Test configuration without applying
sentinelcli config test /path/to/new-config.yaml

# Apply configuration with validation
sentinelcli config apply /path/to/new-config.yaml --validate
```

---

*This configuration guide provides comprehensive information about configuring SentinelD for different deployment scenarios. For additional help, consult the troubleshooting section or contact support.*
