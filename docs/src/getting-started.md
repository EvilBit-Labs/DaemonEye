# Getting Started with SentinelD

This guide will help you get SentinelD up and running quickly on your system. SentinelD is designed to be simple to deploy while providing powerful security monitoring capabilities

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

SentinelD requires elevated privileges for process monitoring. The system is designed to:

1. **Request minimal privileges** during startup
2. **Drop privileges immediately** after initialization
3. **Continue operating** with standard user privileges

**Linux**: Requires `CAP_SYS_PTRACE` capability (or root) **Windows**: Requires `SeDebugPrivilege` (or Administrator) **macOS**: Requires appropriate entitlements (or root)

## Installation

### Option 1: Pre-built Binaries (Recommended)

1. **Download the latest release**:

   ```bash
   # Linux
   wget https://github.com/sentineld/sentineld/releases/latest/download/sentineld-linux-x86_64.tar.gz
   tar -xzf sentineld-linux-x86_64.tar.gz

   # macOS
   curl -L https://github.com/sentineld/sentineld/releases/latest/download/sentineld-macos-x86_64.tar.gz | tar -xz

   # Windows
   # Download and extract from GitHub releases
   ```

2. **Install to system directories**:

   ```bash
   # Linux/macOS
   sudo cp procmond sentinelagent sentinelcli /usr/local/bin/
   sudo chmod +x /usr/local/bin/procmond /usr/local/bin/sentinelagent /usr/local/bin/sentinelcli

   # Windows
   # Copy to C:\Program Files\SentinelD\
   ```

### Option 2: From Source

1. **Install Rust** (1.85+):

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env
   ```

2. **Clone and build**:

   ```bash
   git clone https://github.com/sentineld/sentineld.git
   cd sentineld
   cargo build --release
   ```

3. **Install built binaries**:

   ```bash
   sudo cp target/release/procmond target/release/sentinelagent target/release/sentinelcli /usr/local/bin/
   ```

### Option 3: Package Managers

**Homebrew (macOS)**:

```bash
brew install sentineld/sentineld/sentineld
```

**APT (Ubuntu/Debian)**:

```bash
# Add repository (when available)
sudo apt update
sudo apt install sentineld
```

**YUM/DNF (RHEL/CentOS)**:

```bash
# Add repository (when available)
sudo yum install sentineld
```

## Quick Start

### 1. Create Configuration Directory

```bash
# Linux/macOS
sudo mkdir -p /etc/sentineld
sudo chown $USER:$USER /etc/sentineld

# Windows
mkdir C:\ProgramData\SentinelD
```

### 2. Generate Initial Configuration

```bash
# Generate default configuration
sentinelcli config init --output /etc/sentineld/config.yaml
```

This creates a basic configuration file:

```yaml
# SentinelD Configuration
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info

database:
  event_store_path: /var/lib/sentineld/events.redb
  audit_ledger_path: /var/lib/sentineld/audit.sqlite
  retention_days: 30

detection:
  rules_path: /etc/sentineld/rules
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
sudo mkdir -p /var/lib/sentineld
sudo chown $USER:$USER /var/lib/sentineld

# Windows
mkdir C:\ProgramData\SentinelD\data
```

### 4. Start the Services

**Option A: Manual Start (Testing)**

```bash
# Terminal 1: Start sentinelagent (manages procmond)
sentinelagent --config /etc/sentineld/config.yaml

# Terminal 2: Use CLI for queries
sentinelcli --config /etc/sentineld/config.yaml query "SELECT * FROM processes LIMIT 10"
```

**Option B: System Service (Production)**

```bash
# Linux (systemd)
sudo cp scripts/systemd/sentineld.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable sentineld
sudo systemctl start sentineld

# macOS (launchd)
sudo cp scripts/launchd/com.sentineld.agent.plist /Library/LaunchDaemons/
sudo launchctl load /Library/LaunchDaemons/com.sentineld.agent.plist

# Windows (Service)
# Run as Administrator
sc create "SentinelD Agent" binPath="C:\Program Files\SentinelD\sentinelagent.exe --config C:\ProgramData\SentinelD\config.yaml"
sc start "SentinelD Agent"
```

### 5. Verify Installation

```bash
# Check service status
sentinelcli health

# View recent processes
sentinelcli query "SELECT pid, name, executable_path FROM processes ORDER BY collection_time DESC LIMIT 10"

# Check alerts
sentinelcli alerts list

# View system metrics
sentinelcli metrics
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
      tag: sentineld
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
      path: /var/log/sentineld/alerts.json
      format: json
```

## Creating Your First Detection Rule

### 1. Create Rules Directory

```bash
mkdir -p /etc/sentineld/rules
```

### 2. Create a Simple Rule

Create `/etc/sentineld/rules/suspicious-processes.sql`:

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
sentinelcli rules validate /etc/sentineld/rules/suspicious-processes.sql

# Test the rule
sentinelcli rules test /etc/sentineld/rules/suspicious-processes.sql

# Enable the rule
sentinelcli rules enable suspicious-processes
```

### 4. Monitor for Alerts

```bash
# Watch for new alerts
sentinelcli alerts watch

# List recent alerts
sentinelcli alerts list --limit 10

# Export alerts
sentinelcli alerts export --format json --output alerts.json
```

## Common Operations

### Querying Process Data

**Basic Queries**:

```bash
# List all processes
sentinelcli query "SELECT * FROM processes LIMIT 10"

# Find processes by name
sentinelcli query "SELECT * FROM processes WHERE name = 'chrome'"

# Find high CPU processes
sentinelcli query "SELECT * FROM processes WHERE cpu_usage > 50.0"

# Find processes by user
sentinelcli query "SELECT * FROM processes WHERE user_id = '1000'"
```

**Advanced Queries**:

```bash
# Process tree analysis
sentinelcli query "
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
sentinelcli query "
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
sentinelcli rules list

# Enable/disable rules
sentinelcli rules enable rule-name
sentinelcli rules disable rule-name

# Validate rule syntax
sentinelcli rules validate rule-file.sql

# Test rule execution
sentinelcli rules test rule-file.sql

# Import/export rules
sentinelcli rules import rules-bundle.tar.gz
sentinelcli rules export --output rules-backup.tar.gz
```

### System Health Monitoring

```bash
# Check overall health
sentinelcli health

# Check component status
sentinelcli health --component procmond
sentinelcli health --component sentinelagent

# View performance metrics
sentinelcli metrics

# Check database status
sentinelcli database status

# View recent logs
sentinelcli logs --tail 50
```

## Troubleshooting

### Common Issues

**Permission Denied**:

```bash
# Check if running with sufficient privileges
sudo sentinelcli health

# Verify capability requirements
getcap /usr/local/bin/procmond
```

**Database Locked**:

```bash
# Check for running processes
ps aux | grep sentinel

# Stop services and restart
sudo systemctl stop sentineld
sudo systemctl start sentineld
```

**No Processes Detected**:

```bash
# Check scan interval
sentinelcli config get app.scan_interval_ms

# Verify database path
sentinelcli config get database.event_store_path

# Check logs for errors
sentinelcli logs --level error
```

### Debug Mode

Enable debug logging for troubleshooting:

```yaml
app:
  log_level: debug
```

Or use command-line flag:

```bash
sentinelagent --config /etc/sentineld/config.yaml --log-level debug
```

### Getting Help

- **Documentation**: Check the full documentation in `docs/`
- **Logs**: Review logs with `sentinelcli logs`
- **Health Checks**: Use `sentinelcli health` for system status
- **Community**: Join discussions on GitHub or community forums

## Next Steps

Now that you have SentinelD running:

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

*Congratulations! You now have SentinelD running and monitoring your system. The system will continue to collect process data and execute detection rules according to your configuration.*
