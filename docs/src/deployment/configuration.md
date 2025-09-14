# SentinelD Configuration Guide

This guide provides comprehensive configuration instructions for SentinelD, covering all aspects of system setup, tuning, and customization.

## Table of Contents

- [Configuration Overview](#configuration-overview)
- [Configuration Sources](#configuration-sources)
- [Configuration Structure](#configuration-structure)
- [Core Settings](#core-settings)
- [Database Configuration](#database-configuration)
- [Alerting Configuration](#alerting-configuration)
- [Security Configuration](#security-configuration)
- [Performance Tuning](#performance-tuning)
- [Platform-Specific Settings](#platform-specific-settings)
- [Advanced Configuration](#advanced-configuration)
- [Configuration Management](#configuration-management)
- [Troubleshooting](#troubleshooting)

## Configuration Overview

SentinelD uses a hierarchical configuration system that allows for flexible and maintainable settings across different environments and deployment scenarios.

### Configuration Philosophy

- **Hierarchical**: Multiple sources with clear precedence
- **Environment-Aware**: Different settings for dev/staging/prod
- **Secure**: Sensitive settings protected and encrypted
- **Validated**: All configuration validated at startup
- **Hot-Reloadable**: Most settings can be updated without restart

### Configuration Precedence

1. **Command-line flags** (highest precedence)
2. **Environment variables** (`SENTINELD_*`)
3. **User configuration file** (`~/.config/sentineld/config.yaml`)
4. **System configuration file** (`/etc/sentineld/config.yaml`)
5. **Embedded defaults** (lowest precedence)

## Configuration Sources

### Command-Line Flags

```bash
# Basic configuration
sentinelagent --config /path/to/config.yaml --log-level debug

# Override specific settings
sentinelagent --scan-interval 30000 --batch-size 1000

# Show effective configuration
sentinelcli config show --include-defaults
```

### Environment Variables

```bash
# Set environment variables
export SENTINELD_LOG_LEVEL=debug
export SENTINELD_SCAN_INTERVAL_MS=30000
export SENTINELD_DATABASE_PATH=/var/lib/sentineld/processes.db
export SENTINELD_ALERTING_SINKS_0_TYPE=syslog
export SENTINELD_ALERTING_SINKS_0_FACILITY=daemon

# Run with environment configuration
sentinelagent
```

### Configuration Files

**YAML Format** (recommended):

```yaml
# /etc/sentineld/config.yaml
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info
  data_dir: /var/lib/sentineld
  log_dir: /var/log/sentineld

database:
  path: /var/lib/sentineld/processes.db
  max_connections: 10
  retention_days: 30

alerting:
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
    - type: webhook
      enabled: false
      url: https://alerts.example.com/webhook
      headers:
        Authorization: Bearer ${WEBHOOK_TOKEN}
```

**JSON Format**:

```json
{
  "app": {
    "scan_interval_ms": 30000,
    "batch_size": 1000,
    "log_level": "info",
    "data_dir": "/var/lib/sentineld",
    "log_dir": "/var/log/sentineld"
  },
  "database": {
    "path": "/var/lib/sentineld/processes.db",
    "max_connections": 10,
    "retention_days": 30
  },
  "alerting": {
    "sinks": [
      {
        "type": "syslog",
        "enabled": true,
        "facility": "daemon"
      }
    ]
  }
}
```

**TOML Format**:

```toml
[app]
scan_interval_ms = 30000
batch_size = 1000
log_level = "info"
data_dir = "/var/lib/sentineld"
log_dir = "/var/log/sentineld"

[database]
path = "/var/lib/sentineld/processes.db"
max_connections = 10
retention_days = 30

[[alerting.sinks]]
type = "syslog"
enabled = true
facility = "daemon"
```

## Configuration Structure

### Complete Configuration Schema

```yaml
# Application settings
app:
  scan_interval_ms: 30000          # Process scan interval in milliseconds
  batch_size: 1000                 # Batch size for database operations
  log_level: info                  # Logging level (trace, debug, info, warn, error)
  data_dir: /var/lib/sentineld     # Data directory
  log_dir: /var/log/sentineld      # Log directory
  pid_file: /var/run/sentineld.pid # PID file location
  user: sentineld                  # User to run as
  group: sentineld                 # Group to run as
  max_memory_mb: 512               # Maximum memory usage in MB
  max_cpu_percent: 5.0             # Maximum CPU usage percentage

# Database configuration
database:
  path: /var/lib/sentineld/processes.db  # Database file path
  max_connections: 10                    # Maximum database connections
  retention_days: 30                     # Data retention period
  vacuum_interval_hours: 24              # Database vacuum interval
  wal_mode: true                         # Enable WAL mode
  synchronous: NORMAL                    # Synchronous mode
  cache_size: -64000                     # Cache size in KB (negative = KB)
  temp_store: MEMORY                     # Temporary storage location
  journal_mode: WAL                      # Journal mode

# Alerting configuration
alerting:
  enabled: true                          # Enable alerting
  max_queue_size: 10000                  # Maximum alert queue size
  delivery_timeout_ms: 5000              # Alert delivery timeout
  retry_attempts: 3                      # Number of retry attempts
  retry_delay_ms: 1000                   # Delay between retries
  circuit_breaker_threshold: 5           # Circuit breaker failure threshold
  circuit_breaker_timeout_ms: 60000      # Circuit breaker timeout

  # Alert sinks
  sinks:
    - type: syslog                       # Sink type
      enabled: true                      # Enable this sink
      facility: daemon                   # Syslog facility
      priority: info                     # Syslog priority
      tag: sentineld                     # Syslog tag

    - type: webhook                      # Webhook sink
      enabled: false                     # Disabled by default
      url: https://alerts.example.com/webhook
      method: POST                       # HTTP method
      timeout_ms: 5000                   # Request timeout
      retry_attempts: 3                  # Retry attempts
      headers:                           # Custom headers
        Authorization: Bearer ${WEBHOOK_TOKEN}
        Content-Type: application/json
      template: default                  # Alert template

    - type: file                         # File sink
      enabled: false                     # Disabled by default
      path: /var/log/sentineld/alerts.log
      format: json                       # Output format (json, text)
      rotation: daily                    # Log rotation (daily, weekly, monthly)
      max_files: 30                      # Maximum log files to keep

    - type: stdout                       # Standard output sink
      enabled: false                     # Disabled by default
      format: json                       # Output format (json, text)

# Security configuration
security:
  enable_privilege_dropping: true        # Enable privilege dropping
  drop_to_user: sentineld                # User to drop privileges to
  drop_to_group: sentineld               # Group to drop privileges to
  enable_audit_logging: true             # Enable audit logging
  audit_log_path: /var/log/sentineld/audit.log
  enable_integrity_checking: true        # Enable integrity checking
  hash_algorithm: blake3                 # Hash algorithm (blake3, sha256)
  enable_signature_verification: true    # Enable signature verification
  public_key_path: /etc/sentineld/public.key
  private_key_path: /etc/sentineld/private.key

  # Access control
  access_control:
    allowed_users: []                    # Allowed users (empty = all)
    allowed_groups: []                   # Allowed groups (empty = all)
    denied_users: []                     # Denied users
    denied_groups: []                    # Denied groups

  # Network security
  network:
    enable_tls: false                    # Enable TLS for network connections
    cert_file: /etc/sentineld/cert.pem
    key_file: /etc/sentineld/key.pem
    ca_file: /etc/sentineld/ca.pem
    verify_peer: true                    # Verify peer certificates

# Process collection configuration
collection:
  enable_process_collection: true        # Enable process collection
  enable_file_monitoring: false          # Enable file monitoring
  enable_network_monitoring: false       # Enable network monitoring
  enable_kernel_monitoring: false        # Enable kernel monitoring (Enterprise)

  # Process collection settings
  process_collection:
    include_children: true               # Include child processes
    include_threads: false               # Include thread information
    include_memory_maps: false           # Include memory map information
    include_file_descriptors: false      # Include file descriptor information
    max_processes: 10000                 # Maximum processes to collect
    exclude_patterns:                    # Process exclusion patterns
      - systemd*
      - kthreadd*
      - ksoftirqd*

  # File monitoring settings
  file_monitoring:
    watch_directories: []                # Directories to watch
    exclude_patterns:                    # File exclusion patterns
      - '*.tmp'
      - '*.log'
      - '*.cache'
    max_file_size_mb: 100                # Maximum file size to monitor

  # Network monitoring settings
  network_monitoring:
    enable_packet_capture: false         # Enable packet capture
    capture_interface: any               # Network interface to capture
    capture_filter: ''                   # BPF filter expression
    max_packet_size: 1500                # Maximum packet size
    buffer_size_mb: 100                  # Capture buffer size

# Detection engine configuration
detection:
  enable_detection: true                 # Enable detection engine
  rule_directory: /etc/sentineld/rules   # Rules directory
  rule_file_pattern: '*.sql'             # Rule file pattern
  enable_hot_reload: true                # Enable hot reloading
  reload_interval_ms: 5000               # Reload check interval
  max_concurrent_rules: 10               # Maximum concurrent rule executions
  rule_timeout_ms: 30000                 # Rule execution timeout
  enable_rule_caching: true              # Enable rule result caching
  cache_ttl_seconds: 300                 # Cache TTL in seconds

  # Rule execution settings
  execution:
    enable_parallel_execution: true      # Enable parallel rule execution
    max_parallel_rules: 5                # Maximum parallel rules
    enable_rule_optimization: true       # Enable rule optimization
    enable_query_planning: true          # Enable query planning

  # Alert generation
  alert_generation:
    enable_alert_deduplication: true     # Enable alert deduplication
    deduplication_window_ms: 60000       # Deduplication window
    enable_alert_aggregation: true       # Enable alert aggregation
    aggregation_window_ms: 300000        # Aggregation window
    max_alerts_per_rule: 1000            # Maximum alerts per rule

# Observability configuration
observability:
  enable_metrics: true                   # Enable metrics collection
  metrics_port: 9090                     # Metrics server port
  metrics_path: /metrics                 # Metrics endpoint path
  enable_health_checks: true             # Enable health checks
  health_check_port: 8080                # Health check port
  health_check_path: /health             # Health check endpoint

  # Tracing configuration
  tracing:
    enable_tracing: false                # Enable distributed tracing
    trace_endpoint: http://jaeger:14268/api/traces
    trace_sampling_rate: 0.1             # Trace sampling rate
    trace_service_name: sentineld        # Service name for traces

  # Logging configuration
  logging:
    enable_structured_logging: true      # Enable structured logging
    log_format: json                     # Log format (json, text)
    log_timestamp_format: rfc3339        # Timestamp format
    enable_log_rotation: true            # Enable log rotation
    max_log_file_size_mb: 100            # Maximum log file size
    max_log_files: 10                    # Maximum log files to keep

  # Performance monitoring
  performance:
    enable_profiling: false              # Enable performance profiling
    profile_output_dir: /tmp/sentineld/profiles
    enable_memory_profiling: false       # Enable memory profiling
    enable_cpu_profiling: false          # Enable CPU profiling

# Platform-specific configuration
platform:
  linux:
    enable_ebpf: false                   # Enable eBPF monitoring (Enterprise)
    ebpf_program_path: /etc/sentineld/ebpf/monitor.o
    enable_audit: false                  # Enable Linux audit integration
    audit_rules_path: /etc/sentineld/audit.rules

  windows:
    enable_etw: false                    # Enable ETW monitoring (Enterprise)
    etw_session_name: SentinelD
    enable_wmi: false                    # Enable WMI monitoring
    wmi_namespace: root\cimv2

  macos:
    enable_endpoint_security: false      # Enable EndpointSecurity (Enterprise)
    es_client_name: com.sentineld.monitor
    enable_system_events: false          # Enable system event monitoring

# Integration configuration
integrations:
  # SIEM integrations
  siem:
    splunk:
      enabled: false
      hec_url: https://splunk.example.com:8088/services/collector
      hec_token: ${SPLUNK_HEC_TOKEN}
      index: sentineld
      source: sentineld
      sourcetype: sentineld:processes

    elasticsearch:
      enabled: false
      url: https://elasticsearch.example.com:9200
      username: ${ELASTIC_USERNAME}
      password: ${ELASTIC_PASSWORD}
      index: sentineld-processes

    kafka:
      enabled: false
      brokers: [kafka1.example.com:9092, kafka2.example.com:9092]
      topic: sentineld.processes
      security_protocol: PLAINTEXT
      sasl_mechanism: PLAIN
      username: ${KAFKA_USERNAME}
      password: ${KAFKA_PASSWORD}

  # Export formats
  export:
    cef:
      enabled: false
      output_file: /var/log/sentineld/cef.log
      cef_version: '1.0'
      device_vendor: SentinelD
      device_product: Process Monitor
      device_version: 1.0.0

    stix:
      enabled: false
      output_file: /var/log/sentineld/stix.json
      stix_version: '2.1'
      stix_id: sentineld-process-monitor

    json:
      enabled: false
      output_file: /var/log/sentineld/events.json
      pretty_print: true
      include_metadata: true
```

## Core Settings

### Application Settings

**Basic Configuration**:

```yaml
app:
  scan_interval_ms: 30000          # How often to scan processes (30 seconds)
  batch_size: 1000                 # Number of processes to process in each batch
  log_level: info                  # Logging verbosity
  data_dir: /var/lib/sentineld     # Where to store data files
  log_dir: /var/log/sentineld      # Where to store log files
```

**Performance Tuning**:

```yaml
app:
  max_memory_mb: 512               # Limit memory usage to 512MB
  max_cpu_percent: 5.0             # Limit CPU usage to 5%
  scan_interval_ms: 60000          # Reduce scan frequency for lower CPU
  batch_size: 500                  # Smaller batches for lower memory
```

**Security Settings**:

```yaml
app:
  user: sentineld                  # Run as non-root user
  group: sentineld                 # Run as non-root group
  pid_file: /var/run/sentineld.pid # PID file location
```

### Logging Configuration

**Structured Logging**:

```yaml
observability:
  logging:
    enable_structured_logging: true
    log_format: json
    log_timestamp_format: rfc3339
    enable_log_rotation: true
    max_log_file_size_mb: 100
    max_log_files: 10
```

**Log Levels**:

```yaml
app:
  log_level: debug                 # trace, debug, info, warn, error
```

**Log Rotation**:

```yaml
observability:
  logging:
    enable_log_rotation: true
    max_log_file_size_mb: 100      # Rotate when file reaches 100MB
    max_log_files: 10              # Keep 10 rotated files
```

## Database Configuration

### SQLite Settings

**Basic Database Configuration**:

```yaml
database:
  path: /var/lib/sentineld/processes.db
  max_connections: 10
  retention_days: 30
```

**Performance Optimization**:

```yaml
database:
  wal_mode: true                   # Enable Write-Ahead Logging
  synchronous: NORMAL              # Balance safety and performance
  cache_size: -64000               # 64MB cache (negative = KB)
  temp_store: MEMORY               # Store temp tables in memory
  journal_mode: WAL                # Use WAL journal mode
```

**Maintenance Settings**:

```yaml
database:
  vacuum_interval_hours: 24        # Vacuum database every 24 hours
  retention_days: 30               # Keep data for 30 days
  enable_auto_vacuum: true         # Enable automatic vacuuming
```

### Database Security

**Access Control**:

```yaml
database:
  enable_encryption: false         # Enable database encryption
  encryption_key: ${DB_ENCRYPTION_KEY}
  enable_access_control: true      # Enable access control
  allowed_users: [sentineld]       # Allowed database users
```

## Alerting Configuration

### Alert Sinks

**Syslog Sink**:

```yaml
alerting:
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
      priority: info
      tag: sentineld
```

**Webhook Sink**:

```yaml
alerting:
  sinks:
    - type: webhook
      enabled: true
      url: https://alerts.example.com/webhook
      method: POST
      timeout_ms: 5000
      retry_attempts: 3
      headers:
        Authorization: Bearer ${WEBHOOK_TOKEN}
        Content-Type: application/json
```

**File Sink**:

```yaml
alerting:
  sinks:
    - type: file
      enabled: true
      path: /var/log/sentineld/alerts.log
      format: json
      rotation: daily
      max_files: 30
```

### Alert Processing

**Deduplication and Aggregation**:

```yaml
detection:
  alert_generation:
    enable_alert_deduplication: true
    deduplication_window_ms: 60000
    enable_alert_aggregation: true
    aggregation_window_ms: 300000
    max_alerts_per_rule: 1000
```

**Delivery Settings**:

```yaml
alerting:
  max_queue_size: 10000
  delivery_timeout_ms: 5000
  retry_attempts: 3
  retry_delay_ms: 1000
  circuit_breaker_threshold: 5
  circuit_breaker_timeout_ms: 60000
```

## Security Configuration

### Privilege Management

**Privilege Dropping**:

```yaml
security:
  enable_privilege_dropping: true
  drop_to_user: sentineld
  drop_to_group: sentineld
```

**Access Control**:

```yaml
security:
  access_control:
    allowed_users: []               # Empty = all users
    allowed_groups: []              # Empty = all groups
    denied_users: [root]            # Deny root user
    denied_groups: [wheel]          # Deny wheel group
```

### Audit and Integrity

**Audit Logging**:

```yaml
security:
  enable_audit_logging: true
  audit_log_path: /var/log/sentineld/audit.log
```

**Integrity Checking**:

```yaml
security:
  enable_integrity_checking: true
  hash_algorithm: blake3
  enable_signature_verification: true
  public_key_path: /etc/sentineld/public.key
  private_key_path: /etc/sentineld/private.key
```

### Network Security

**TLS Configuration**:

```yaml
security:
  network:
    enable_tls: true
    cert_file: /etc/sentineld/cert.pem
    key_file: /etc/sentineld/key.pem
    ca_file: /etc/sentineld/ca.pem
    verify_peer: true
```

## Performance Tuning

### Process Collection

**Collection Settings**:

```yaml
collection:
  process_collection:
    include_children: true
    include_threads: false
    include_memory_maps: false
    include_file_descriptors: false
    max_processes: 10000
    exclude_patterns:
      - systemd*
      - kthreadd*
      - ksoftirqd*
```

**Performance Optimization**:

```yaml
app:
  scan_interval_ms: 60000          # Reduce scan frequency
  batch_size: 500                  # Smaller batches
  max_memory_mb: 256               # Limit memory usage
  max_cpu_percent: 3.0             # Limit CPU usage
```

### Database Performance

**Connection Pooling**:

```yaml
database:
  max_connections: 20              # Increase connection pool
  cache_size: -128000              # 128MB cache
  temp_store: MEMORY               # Use memory for temp tables
```

**Query Optimization**:

```yaml
detection:
  execution:
    enable_rule_optimization: true
    enable_query_planning: true
    enable_parallel_execution: true
    max_parallel_rules: 5
```

### Memory Management

**Memory Limits**:

```yaml
app:
  max_memory_mb: 512               # Hard memory limit
  max_cpu_percent: 5.0             # CPU usage limit
```

**Garbage Collection**:

```yaml
app:
  gc_interval_ms: 300000           # Garbage collection interval
  gc_threshold_mb: 100             # GC threshold
```

## Platform-Specific Settings

### Linux Configuration

**eBPF Monitoring** (Enterprise):

```yaml
platform:
  linux:
    enable_ebpf: true
    ebpf_program_path: /etc/sentineld/ebpf/monitor.o
    enable_audit: true
    audit_rules_path: /etc/sentineld/audit.rules
```

**System Integration**:

```yaml
platform:
  linux:
    enable_systemd_integration: true
    systemd_unit: sentineld.service
    enable_logrotate: true
    logrotate_config: /etc/logrotate.d/sentineld
```

### Windows Configuration

**ETW Monitoring** (Enterprise):

```yaml
platform:
  windows:
    enable_etw: true
    etw_session_name: SentinelD
    enable_wmi: true
    wmi_namespace: root\cimv2
```

**Service Integration**:

```yaml
platform:
  windows:
    service_name: SentinelD Agent
    service_display_name: SentinelD Security Monitoring Agent
    service_description: Monitors system processes for security threats
```

### macOS Configuration

**EndpointSecurity** (Enterprise):

```yaml
platform:
  macos:
    enable_endpoint_security: true
    es_client_name: com.sentineld.monitor
    enable_system_events: true
```

**LaunchDaemon Integration**:

```yaml
platform:
  macos:
    launchdaemon_plist: /Library/LaunchDaemons/com.sentineld.agent.plist
    enable_console_logging: true
```

## Advanced Configuration

### Custom Rules

**Rule Directory**:

```yaml
detection:
  rule_directory: /etc/sentineld/rules
  rule_file_pattern: '*.sql'
  enable_hot_reload: true
  reload_interval_ms: 5000
```

**Rule Execution**:

```yaml
detection:
  max_concurrent_rules: 10
  rule_timeout_ms: 30000
  enable_rule_caching: true
  cache_ttl_seconds: 300
```

### Custom Integrations

**SIEM Integration**:

```yaml
integrations:
  siem:
    splunk:
      enabled: true
      hec_url: https://splunk.example.com:8088/services/collector
      hec_token: ${SPLUNK_HEC_TOKEN}
      index: sentineld
      source: sentineld
      sourcetype: sentineld:processes
```

**Export Formats**:

```yaml
integrations:
  export:
    cef:
      enabled: true
      output_file: /var/log/sentineld/cef.log
      cef_version: '1.0'
      device_vendor: SentinelD
      device_product: Process Monitor
      device_version: 1.0.0
```

### Custom Templates

**Alert Templates**:

```yaml
alerting:
  templates:
    default: |
      {
        "timestamp": "{{.Timestamp}}",
        "rule": "{{.RuleName}}",
        "severity": "{{.Severity}}",
        "process": {
          "pid": {{.Process.PID}},
          "name": "{{.Process.Name}}",
          "path": "{{.Process.ExecutablePath}}"
        }
      }

    syslog: |
      {{.Timestamp}} {{.Severity}} {{.RuleName}}: Process {{.Process.Name}} (PID {{.Process.PID}}) triggered alert
```

## Configuration Management

### Configuration Validation

**Validate Configuration**:

```bash
# Validate configuration file
sentinelcli config validate /path/to/config.yaml

# Check configuration syntax
sentinelcli config check

# Show effective configuration
sentinelcli config show --include-defaults
```

**Configuration Testing**:

```bash
# Test configuration without starting service
sentinelagent --config /path/to/config.yaml --dry-run

# Test specific settings
sentinelcli config test --setting app.scan_interval_ms
```

### Configuration Updates

**Hot Reload**:

```bash
# Reload configuration without restart
sentinelcli config reload

# Update specific setting
sentinelcli config set app.scan_interval_ms 60000

# Update multiple settings
sentinelcli config set app.scan_interval_ms 60000 app.batch_size 500
```

**Configuration Backup**:

```bash
# Backup current configuration
sentinelcli config backup --output /backup/sentineld-config-$(date +%Y%m%d).yaml

# Restore configuration
sentinelcli config restore --input /backup/sentineld-config-20240101.yaml
```

### Environment Management

**Development Environment**:

```yaml
# config-dev.yaml
app:
  log_level: debug
  scan_interval_ms: 10000
  batch_size: 100

database:
  path: /tmp/sentineld-dev.db
  retention_days: 1
```

**Production Environment**:

```yaml
# config-prod.yaml
app:
  log_level: info
  scan_interval_ms: 60000
  batch_size: 1000

database:
  path: /var/lib/sentineld/processes.db
  retention_days: 30
```

**Staging Environment**:

```yaml
# config-staging.yaml
app:
  log_level: info
  scan_interval_ms: 30000
  batch_size: 500

database:
  path: /var/lib/sentineld/processes-staging.db
  retention_days: 7
```

## Troubleshooting

### Configuration Issues

**Invalid Configuration**:

```bash
# Check configuration syntax
sentinelcli config check

# Validate configuration
sentinelcli config validate

# Show configuration errors
sentinelcli config show --errors
```

**Missing Settings**:

```bash
# Show all settings with defaults
sentinelcli config show --include-defaults

# Show specific setting
sentinelcli config get app.scan_interval_ms

# Set missing setting
sentinelcli config set app.scan_interval_ms 30000
```

**Permission Issues**:

```bash
# Check file permissions
ls -la /etc/sentineld/config.yaml

# Fix permissions
sudo chown sentineld:sentineld /etc/sentineld/config.yaml
sudo chmod 644 /etc/sentineld/config.yaml
```

### Performance Issues

**High CPU Usage**:

```yaml
# Reduce scan frequency
app:
  scan_interval_ms: 120000          # 2 minutes

# Reduce batch size
app:
  batch_size: 250

# Exclude more processes
collection:
  process_collection:
    exclude_patterns:
      - "systemd*"
      - "kthreadd*"
      - "ksoftirqd*"
      - "migration*"
      - "rcu_*"
```

**High Memory Usage**:

```yaml
# Limit memory usage
app:
  max_memory_mb: 256

# Reduce batch size
app:
  batch_size: 250

# Enable garbage collection
app:
  gc_interval_ms: 300000
  gc_threshold_mb: 100
```

**Slow Database Operations**:

```yaml
# Optimize database settings
database:
  cache_size: -128000              # 128MB cache
  temp_store: MEMORY
  synchronous: NORMAL
  wal_mode: true

# Enable query optimization
detection:
  execution:
    enable_rule_optimization: true
    enable_query_planning: true
```

### Debugging Configuration

**Enable Debug Logging**:

```yaml
app:
  log_level: debug

observability:
  logging:
    enable_structured_logging: true
    log_format: json
```

**Configuration Debugging**:

```bash
# Show effective configuration
sentinelcli config show --include-defaults --format json

# Test configuration
sentinelagent --config /path/to/config.yaml --dry-run

# Check configuration sources
sentinelcli config sources
```

**Performance Debugging**:

```yaml
observability:
  performance:
    enable_profiling: true
    profile_output_dir: /tmp/sentineld/profiles
    enable_memory_profiling: true
    enable_cpu_profiling: true
```

---

*This configuration guide provides comprehensive instructions for configuring SentinelD. For additional help, consult the troubleshooting section or contact support.*
