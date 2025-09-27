# Getting Started with DaemonEye

This guide will help you get DaemonEye up and running quickly on your system. DaemonEye is designed to be simple to deploy while providing powerful security monitoring capabilities

---

## Table of Contents

[TOC]

---

## Prerequisites

### System Requirements

**Minimum Requirements**:

- **OS**: Linux (kernel 3.10+), macOS (10.14+), or Windows (10+)
- **RAM**: 512MB available memory
- **Disk**: 1GB free space
- **CPU**: Any x86_64 or ARM64 processor

**Recommended Requirements**:

- **OS**: Linux (kernel 4.15+), macOS (11+), or Windows (11+)
- **RAM**: 2GB+ available memory
- **Disk**: 10GB+ free space
- **CPU**: 2+ cores

### Privilege Requirements

DaemonEye requires elevated privileges for process monitoring. The system is designed to:

1. **Request minimal privileges** during startup
2. **Drop privileges immediately** after initialization
3. **Continue operating** with standard user privileges

- **Linux**: Requires `CAP_SYS_PTRACE` capability (or root)
- **Windows**: Requires `SeDebugPrivilege` (or Administrator)
- **macOS**: Requires appropriate entitlements (or root)

## Installation

### Option 1: Pre-built Binaries (Recommended)

1. **Download the latest release**:

   ```bash
   # Linux
   wget https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-linux-x86_64.tar.gz
   tar -xzf daemoneye-linux-x86_64.tar.gz

   # macOS
   curl -L https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-macos-x86_64.tar.gz | tar -xz

   # Windows
   # Download and extract from GitHub releases
   ```

2. **Install to system directories**:

   ```bash
   # Linux/macOS
   sudo cp procmond daemoneye-agent daemoneye-cli /usr/local/bin/
   sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli

   # Windows
   # Copy to C:\Program Files\DaemonEye\
   ```

### Option 2: From Source

1. **Install Rust** (1.85+):

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env
   ```

2. **Clone and build**:

   ```bash
   git clone https://github.com/daemoneye/daemoneye.git
   cd daemoneye
   cargo build --release
   ```

3. **Install built binaries**:

   ```bash
   sudo cp target/release/procmond target/release/daemoneye-agent target/release/daemoneye-cli /usr/local/bin/
   ```

### Option 3: Package Managers

**Homebrew (macOS)**:

```bash
brew install daemoneye/daemoneye/daemoneye
```

**APT (Ubuntu/Debian)**:

```bash
# Add repository (when available)
sudo apt update
sudo apt install daemoneye
```

**YUM/DNF (RHEL/CentOS)**:

```bash
# Add repository (when available)
sudo yum install daemoneye
```

## Quick Start

### 1. Create Configuration Directory

```bash
# Linux/macOS
sudo mkdir -p /etc/daemoneye
sudo chown $USER:$USER /etc/daemoneye

# Windows
mkdir C:\ProgramData\DaemonEye
```

### 2. Generate Initial Configuration

```bash
# Generate default configuration
daemoneye-cli config init --output /etc/daemoneye/config.yaml
```

This creates a basic configuration file:

```yaml
# DaemonEye Configuration
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info

database:
  event_store_path: /var/lib/daemoneye/events.redb
  audit_ledger_path: /var/lib/daemoneye/audit.sqlite
  retention_days: 30

detection:
  rules_path: /etc/daemoneye/rules
  enabled_rules: ['*']

alerting:
  sinks:
    - type: stdout
      enabled: true
    - type: syslog
      enabled: true
      facility: daemon

# Platform-specific settings
platform:
  linux:
    enable_ebpf: false  # Requires kernel 4.15+
  windows:
    enable_etw: false   # Requires Windows 10+
  macos:
    enable_endpoint_security: false  # Requires macOS 10.15+
```

### 3. Create Data Directory

```bash
# Linux/macOS
sudo mkdir -p /var/lib/daemoneye
sudo chown $USER:$USER /var/lib/daemoneye

# Windows
mkdir C:\ProgramData\DaemonEye\data
```

### 4. Start the Services

#### Option A: Manual Start (Testing)

```bash
# Terminal 1: Start daemoneye-agent (manages procmond automatically)
daemoneye-agent --database /var/lib/daemoneye/events.redb --log-level info

# Terminal 2: Use CLI for database queries and health checks
daemoneye-cli --database /var/lib/daemoneye/events.redb --format json

# Terminal 3: Run procmond directly for testing (optional)
procmond --database /var/lib/daemoneye/events.redb --interval 30 --enhanced-metadata
```

#### Option B: System Service (Production)

```bash
# Linux (systemd)
sudo cp scripts/systemd/daemoneye.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable daemoneye
sudo systemctl start daemoneye

# macOS (launchd)
sudo cp scripts/launchd/com.daemoneye.agent.plist /Library/LaunchDaemons/
sudo launchctl load /Library/LaunchDaemons/com.daemoneye.agent.plist

# Windows (Service)
# Run as Administrator
sc create "DaemonEye Agent" binPath="C:\Program Files\DaemonEye\daemoneye-agent.exe --config C:\ProgramData\DaemonEye\config.yaml"
sc start "DaemonEye Agent"
```

### 5. Verify Installation

```bash
# Check database statistics and health
daemoneye-cli --database /var/lib/daemoneye/events.redb --format human

# View database statistics in JSON format
daemoneye-cli --database /var/lib/daemoneye/events.redb --format json

# Test procmond collection with enhanced metadata
procmond --database /var/lib/daemoneye/events.redb --interval 30 --enhanced-metadata --compute-hashes

# Check component help
daemoneye-agent --help
daemoneye-cli --help
procmond --help
```

## Basic Configuration

### Essential Settings

**Scan Interval**: How often to collect process data

```yaml
app:
  scan_interval_ms: 30000  # 30 seconds
```

**Database Retention**: How long to keep data

```yaml
database:
  retention_days: 30  # Keep data for 30 days
```

**Log Level**: Verbosity of logging

```yaml
app:
  log_level: info    # debug, info, warn, error
```

### Alert Configuration

**Enable Syslog Alerts**:

```yaml
alerting:
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
      tag: daemoneye
```

**Enable Webhook Alerts**:

```yaml
alerting:
  sinks:
    - type: webhook
      enabled: true
      url: https://your-siem.com/webhook
      headers:
        Authorization: Bearer your-token
```

**Enable File Output**:

```yaml
alerting:
  sinks:
    - type: file
      enabled: true
      path: /var/log/daemoneye/alerts.json
      format: json
```

## Creating Your First Detection Rule

### 1. Create Rules Directory

```bash
mkdir -p /etc/daemoneye/rules
```

### 2. Create a Simple Rule

Create `/etc/daemoneye/rules/suspicious-processes.sql`:

```sql
-- Detect processes with suspicious names
SELECT
    pid,
    name,
    executable_path,
    command_line,
    collection_time
FROM processes
WHERE
    name IN ('malware.exe', 'backdoor.exe', 'trojan.exe')
    OR name LIKE '%suspicious%'
    OR executable_path LIKE '%temp%'
ORDER BY collection_time DESC;
```

### 3. Test the Rule

```bash
# Validate the rule
daemoneye-cli rules validate /etc/daemoneye/rules/suspicious-processes.sql

# Test the rule
daemoneye-cli rules test /etc/daemoneye/rules/suspicious-processes.sql

# Enable the rule
daemoneye-cli rules enable suspicious-processes
```

### 4. Monitor for Alerts

```bash
# Watch for new alerts
daemoneye-cli alerts watch

# List recent alerts
daemoneye-cli alerts list --limit 10

# Export alerts
daemoneye-cli alerts export --format json --output alerts.json
```

## Common Operations

### Querying Process Data

**Basic Queries**:

```bash
# List all processes
daemoneye-cli query "SELECT * FROM processes LIMIT 10"

# Find processes by name
daemoneye-cli query "SELECT * FROM processes WHERE name = 'chrome'"

# Find high CPU processes
daemoneye-cli query "SELECT * FROM processes WHERE cpu_usage > 50.0"

# Find processes by user
daemoneye-cli query "SELECT * FROM processes WHERE user_id = '1000'"
```

**Advanced Queries**:

```bash
# Process tree analysis
daemoneye-cli query "
SELECT
    p1.pid as parent_pid,
    p1.name as parent_name,
    p2.pid as child_pid,
    p2.name as child_name
FROM processes p1
JOIN processes p2 ON p1.pid = p2.ppid
WHERE p1.name = 'systemd'
"

# Suspicious process patterns
daemoneye-cli query "
SELECT
    pid,
    name,
    executable_path,
    COUNT(*) as occurrence_count
FROM processes
WHERE executable_path LIKE '%temp%'
GROUP BY pid, name, executable_path
HAVING occurrence_count > 5
"
```

### Managing Rules

```bash
# List all rules
daemoneye-cli rules list

# Enable/disable rules
daemoneye-cli rules enable rule-name
daemoneye-cli rules disable rule-name

# Validate rule syntax
daemoneye-cli rules validate rule-file.sql

# Test rule execution
daemoneye-cli rules test rule-file.sql

# Import/export rules
daemoneye-cli rules import rules-bundle.tar.gz
daemoneye-cli rules export --output rules-backup.tar.gz
```

### System Health Monitoring

```bash
# Check overall health
daemoneye-cli health

# Check component status
daemoneye-cli health --component procmond
daemoneye-cli health --component daemoneye-agent

# View performance metrics
daemoneye-cli metrics

# Check database status
daemoneye-cli database status

# View recent logs
daemoneye-cli logs --tail 50
```

## Troubleshooting

### Common Issues

**Permission Denied**:

```bash
# Check if running with sufficient privileges
sudo daemoneye-cli health

# Verify capability requirements
getcap /usr/local/bin/procmond
```

**Database Locked**:

```bash
# Check for running processes
ps aux | grep daemoneye

# Stop services and restart
sudo systemctl stop daemoneye
sudo systemctl start daemoneye
```

**No Processes Detected**:

```bash
# Check scan interval
daemoneye-cli config get app.scan_interval_ms

# Verify database path
daemoneye-cli config get database.event_store_path

# Check logs for errors
daemoneye-cli logs --level error
```

### Debug Mode

Enable debug logging for troubleshooting:

```yaml
app:
  log_level: debug
```

Or use command-line flag:

```bash
daemoneye-agent --config /etc/daemoneye/config.yaml --log-level debug
```

### Getting Help

- **Documentation**: Check the full documentation in `docs/`
- **Logs**: Review logs with `daemoneye-cli logs`
- **Health Checks**: Use `daemoneye-cli health` for system status
- **Community**: Join discussions on GitHub or community forums

## Next Steps

Now that you have DaemonEye running:

1. **Read the [Operator Guide](../user-guides/operator-guide.md)** for detailed usage instructions
2. **Explore [Configuration Guide](../user-guides/configuration.md)** for advanced configuration
3. **Learn [Rule Development](../user-guides/rule-development.md)** for creating custom detection rules
4. **Review [Security Architecture](../architecture/security-model.md)** for understanding the security model
5. **Check [Deployment Guide](../deployment/installation.md)** for production deployment

## Support

- **Documentation**: Comprehensive guides in the `docs/` directory
- **Issues**: Report bugs and request features on GitHub
- **Community**: Join discussions and get help from the community
- **Security**: Follow responsible disclosure for security issues

---

*Congratulations! You now have DaemonEye running and monitoring your system. The system will continue to collect process data and execute detection rules according to your configuration.*
