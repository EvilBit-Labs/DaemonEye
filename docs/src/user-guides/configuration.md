# DaemonEye Configuration Guide

This guide provides comprehensive information about configuring DaemonEye for different deployment scenarios and requirements.

## Table of Contents

- [Configuration Overview](#configuration-overview)
- [Configuration Hierarchy](#configuration-hierarchy)
- [Core Configuration](#core-configuration)
- [Alerting Configuration](#alerting-configuration)
- [Database Configuration](#database-configuration)
- [Platform-Specific Configuration](#platform-specific-configuration)
- [Environment Variables](#environment-variables)
- [Configuration Examples](#configuration-examples)
- [Troubleshooting](#troubleshooting)

## Configuration Overview

DaemonEye uses a hierarchical configuration system that allows you to override settings at different levels. The configuration is loaded in the following order (later sources override earlier ones):

1. **Embedded defaults** (lowest precedence)
2. **System configuration files** (`/etc/daemoneye/config.yaml`)
3. **User configuration files** (`~/.config/daemoneye/config.yaml`)
4. **Environment variables** (`DAEMONEYE_*`)
5. **Command-line flags** (highest precedence)

## Configuration Hierarchy

### File Locations

**System Configuration**:

- Linux: `/etc/daemoneye/config.yaml`
- macOS: `/Library/Application Support/DaemonEye/config.yaml`
- Windows: `C:\ProgramData\DaemonEye\config.yaml`

**User Configuration**:

- Linux/macOS: `~/.config/daemoneye/config.yaml`
- Windows: `%APPDATA%\DaemonEye\config.yaml`

**Component-Specific Configuration**:

- Components use the same configuration file with component-specific sections
- Environment variables can override specific component settings
- Command-line flags provide the highest precedence overrides

### Configuration Formats

DaemonEye supports multiple configuration formats:

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

# EventBus broker configuration (daemoneye-agent)
broker:
  # Unix socket path for EventBus broker
  socket_path: /tmp/daemoneye-eventbus.sock

  # Broker startup timeout in seconds
  startup_timeout_seconds: 30

  # Maximum message buffer size
  max_message_buffer_size: 10000

  # Message processing timeout in milliseconds
  message_timeout_ms: 5000
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
  rules_path: /etc/daemoneye/rules

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
      path: /var/log/daemoneye/alerts.json
      format: json
      rotation:
        max_size_mb: 100
        max_files: 10

    # Syslog sink
    - type: syslog
      enabled: true
      facility: daemon
      tag: daemoneye
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
      from: daemoneye@example.com
      to: [security@example.com]
      subject: 'DaemonEye Alert: {severity} - {title}'
```

Additional sink types (Splunk HEC, Elasticsearch, Kafka, and others) are available in commercial tiers.

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

### Database Configuration (redb)

```yaml
database:
  # Database file path
  path: /var/lib/daemoneye/events.redb

  # Data retention period in days
  retention_days: 30

  # Maximum database size in MB
  max_size_mb: 10240

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
    # Enable process namespace monitoring
    enable_namespace_monitoring: true

    # Enable cgroup monitoring
    enable_cgroup_monitoring: true

    # Process collection method
    collection_method: sysinfo

    # Privilege requirements
    privileges:
      # Required capabilities
      capabilities: [SYS_PTRACE]

      # Drop privileges after initialization
      drop_privileges: true

      # Privilege drop timeout in seconds
      privilege_drop_timeout_secs: 30
```

Kernel-level collection (eBPF) is available in commercial tiers.

### Windows Configuration

```yaml
platform:
  windows:
    # Enable registry monitoring
    enable_registry_monitoring: false

    # Enable file system monitoring
    enable_filesystem_monitoring: false

    # Process collection method
    collection_method: sysinfo

    # Privilege requirements
    privileges:
      # Required privileges
      privileges: [SeDebugPrivilege]

      # Drop privileges after initialization
      drop_privileges: true
```

Kernel-level collection (ETW) is available in commercial tiers.

### macOS Configuration

```yaml
platform:
  macos:
    # Enable file system monitoring
    enable_filesystem_monitoring: false

    # Enable network monitoring
    enable_network_monitoring: false

    # Process collection method
    collection_method: sysinfo

    # Privilege requirements
    privileges:
      # Required entitlements
      entitlements: [com.apple.security.cs.allow-jit]

      # Drop privileges after initialization
      drop_privileges: true
```

Kernel-level collection (EndpointSecurity) is available in commercial tiers.

## Environment Variables

### Core Variables

```bash
# Application settings
export DAEMONEYE_LOG_LEVEL=info
export DAEMONEYE_SCAN_INTERVAL_MS=30000
export DAEMONEYE_BATCH_SIZE=1000
export DAEMONEYE_RETENTION_DAYS=30

# Database settings
export DAEMONEYE_DATABASE_PATH=/var/lib/daemoneye/events.redb
export DAEMONEYE_AUDIT_LEDGER_PATH=/var/lib/daemoneye/audit.sqlite

# Alerting settings
export DAEMONEYE_ALERTING_ENABLED=true
export DAEMONEYE_WEBHOOK_URL=https://your-siem.com/webhook
export DAEMONEYE_WEBHOOK_TOKEN=your-webhook-token

# Platform settings
export DAEMONEYE_ENABLE_EBPF=false
export DAEMONEYE_ENABLE_ETW=false
export DAEMONEYE_ENABLE_ENDPOINT_SECURITY=false
```

## Configuration Examples

### Basic Production Configuration

```yaml
# /etc/daemoneye/config.yaml
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info

database:
  path: /var/lib/daemoneye/events.redb
  retention_days: 30
  max_size_mb: 10240
  enable_cleanup: true

# EventBus broker configuration
broker:
  socket_path: /tmp/daemoneye-eventbus.sock
  startup_timeout_seconds: 30
  max_message_buffer_size: 10000

# Platform-specific settings
platform:
  linux:
    enable_ebpf: false
  windows:
    enable_etw: false
  macos:
    enable_endpoint_security: false
```

### High-Performance Configuration

```yaml
# /etc/daemoneye/config.yaml
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
  rules_path: /etc/daemoneye/rules
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
      tag: daemoneye
    - type: kafka
      enabled: true
      brokers: [kafka.example.com:9092]
      topic: daemoneye.alerts
      batch_size: 100
      batch_timeout_ms: 1000

database:
  event_store:
    path: /var/lib/daemoneye/events.redb
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
# /etc/daemoneye/config.yaml
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
  rules_path: /etc/daemoneye/rules
  enable_hot_reload: false  # Disable hot reload
  rule_timeout_secs: 60
  max_concurrent_rules: 5

alerting:
  enabled: true
  dedupe_window_minutes: 120
  sinks:
    - type: file
      enabled: true
      path: /var/log/daemoneye/alerts.json
      format: json
      rotation:
        max_size_mb: 50
        max_files: 20
    - type: syslog
      enabled: true
      facility: daemon
      tag: daemoneye

database:
  event_store:
    path: /var/lib/daemoneye/events.redb
    max_size_mb: 5120
    wal_mode: true
  audit_ledger:
    path: /var/lib/daemoneye/audit.sqlite
    wal_mode: true
    synchronous: FULL
    journal_mode: WAL
```

## Troubleshooting

### Configuration Validation

```bash
# Validate configuration file
daemoneye-cli config validate /etc/daemoneye/config.yaml

# Validate current configuration
daemoneye-cli config validate

# Check for configuration issues
daemoneye-cli config check

# Show effective configuration
daemoneye-cli config show --include-defaults
```

### Common Configuration Issues

**Invalid YAML Syntax**:

```bash
# Check YAML syntax
python -c "import yaml; yaml.safe_load(open('/etc/daemoneye/config.yaml'))"

# Use online YAML validator
# https://www.yamllint.com/
```

**Missing Required Fields**:

```bash
# Check for missing required fields
daemoneye-cli config check --strict

# Show configuration with defaults
daemoneye-cli config show --include-defaults
```

**Permission Issues**:

```bash
# Check file permissions
ls -la /etc/daemoneye/config.yaml
ls -la /var/lib/daemoneye/

# Fix permissions
sudo chown daemoneye:daemoneye /var/lib/daemoneye/
sudo chmod 755 /var/lib/daemoneye/
```

**Environment Variable Issues**:

```bash
# Check environment variables
env | grep DAEMONEYE

# Test environment variable substitution
daemoneye-cli config show --environment
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
daemoneye-cli config debug

# Show configuration sources
daemoneye-cli config sources
```

**Test Configuration Changes**:

```bash
# Test configuration without applying
daemoneye-cli config test /path/to/new-config.yaml

# Apply configuration with validation
daemoneye-cli config apply /path/to/new-config.yaml --validate
```

---

*This configuration guide provides comprehensive information about configuring DaemonEye for different deployment scenarios. For additional help, consult the troubleshooting section or contact support.*
