# SentinelD Operator Guide

This guide provides comprehensive instructions for operators managing SentinelD in production environments. It covers day-to-day operations, troubleshooting, and advanced configuration.

## Table of Contents

- [System Overview](#system-overview)
- [Basic Operations](#basic-operations)
- [Process Monitoring](#process-monitoring)
- [Alert Management](#alert-management)
- [Rule Management](#rule-management)
- [System Health Monitoring](#system-health-monitoring)
- [Configuration Management](#configuration-management)
- [Troubleshooting](#troubleshooting)
- [Best Practices](#best-practices)

## System Overview

### Component Status

Check the overall health of your SentinelD installation:

```bash
# Overall system health
sentinelcli health

# Component-specific health
sentinelcli health --component procmond
sentinelcli health --component sentinelagent
sentinelcli health --component database
```

**Expected Output**:

```text
System Health: Healthy
├── procmond: Running (PID: 1234)
├── sentinelagent: Running (PID: 1235)
├── database: Connected
└── alerting: All sinks operational
```

### Service Management

**Start Services**:

```bash
# Linux (systemd)
sudo systemctl start sentineld

# macOS (launchd)
sudo launchctl load /Library/LaunchDaemons/com.sentineld.agent.plist

# Windows (Service)
sc start "SentinelD Agent"
```

**Stop Services**:

```bash
# Linux (systemd)
sudo systemctl stop sentineld

# macOS (launchd)
sudo launchctl unload /Library/LaunchDaemons/com.sentineld.agent.plist

# Windows (Service)
sc stop "SentinelD Agent"
```

**Restart Services**:

```bash
# Linux (systemd)
sudo systemctl restart sentineld

# macOS (launchd)
sudo launchctl unload /Library/LaunchDaemons/com.sentineld.agent.plist
sudo launchctl load /Library/LaunchDaemons/com.sentineld.agent.plist

# Windows (Service)
sc stop "SentinelD Agent"
sc start "SentinelD Agent"
```

## Basic Operations

### Querying Process Data

**List Recent Processes**:

```bash
# Last 10 processes
sentinelcli query "SELECT pid, name, executable_path, collection_time FROM processes ORDER BY collection_time DESC LIMIT 10"

# Processes by name
sentinelcli query "SELECT * FROM processes WHERE name = 'chrome'"

# High CPU processes
sentinelcli query "SELECT pid, name, cpu_usage FROM processes WHERE cpu_usage > 50.0 ORDER BY cpu_usage DESC"
```

**Process Tree Analysis**:

```bash
# Find child processes of a specific parent
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

# Process hierarchy depth
sentinelcli query "
WITH RECURSIVE process_tree AS (
    SELECT pid, ppid, name, 0 as depth
    FROM processes
    WHERE ppid IS NULL
    UNION ALL
    SELECT p.pid, p.ppid, p.name, pt.depth + 1
    FROM processes p
    JOIN process_tree pt ON p.ppid = pt.pid
)
SELECT pid, name, depth FROM process_tree ORDER BY depth, pid
"
```

**Suspicious Process Patterns**:

```bash
# Processes with suspicious names
sentinelcli query "
SELECT pid, name, executable_path, command_line
FROM processes
WHERE name IN ('malware.exe', 'backdoor.exe', 'trojan.exe')
   OR name LIKE '%suspicious%'
   OR executable_path LIKE '%temp%'
"

# Processes with unusual parent-child relationships
sentinelcli query "
SELECT
    p1.pid as parent_pid,
    p1.name as parent_name,
    p2.pid as child_pid,
    p2.name as child_name
FROM processes p1
JOIN processes p2 ON p1.pid = p2.ppid
WHERE p1.name = 'explorer.exe'
  AND p2.name NOT IN ('chrome.exe', 'firefox.exe', 'notepad.exe')
"
```

### Data Export

**Export to Different Formats**:

```bash
# JSON export
sentinelcli query "SELECT * FROM processes WHERE cpu_usage > 10.0" --format json > high_cpu_processes.json

# CSV export
sentinelcli query "SELECT pid, name, cpu_usage, memory_usage FROM processes" --format csv > process_metrics.csv

# Table format (default)
sentinelcli query "SELECT * FROM processes LIMIT 5" --format table
```

**Export with Filters**:

```bash
# Export processes from last hour
sentinelcli query "
SELECT * FROM processes
WHERE collection_time > (strftime('%s', 'now') - 3600) * 1000
" --format json > recent_processes.json

# Export by user
sentinelcli query "
SELECT * FROM processes
WHERE user_id = '1000'
" --format csv > user_processes.csv
```

## Process Monitoring

### Real-time Monitoring

**Watch Process Activity**:

```bash
# Monitor new processes in real-time
sentinelcli watch processes --filter "name LIKE '%chrome%'"

# Monitor high CPU processes
sentinelcli watch processes --filter "cpu_usage > 50.0"

# Monitor specific user processes
sentinelcli watch processes --filter "user_id = '1000'"
```

**Process Statistics**:

```bash
# Process count by name
sentinelcli query "
SELECT name, COUNT(*) as count
FROM processes
GROUP BY name
ORDER BY count DESC
LIMIT 10
"

# CPU usage distribution
sentinelcli query "
SELECT
    CASE
        WHEN cpu_usage IS NULL THEN 'Unknown'
        WHEN cpu_usage = 0 THEN '0%'
        WHEN cpu_usage < 10 THEN '1-9%'
        WHEN cpu_usage < 50 THEN '10-49%'
        WHEN cpu_usage < 100 THEN '50-99%'
        ELSE '100%+'
    END as cpu_range,
    COUNT(*) as process_count
FROM processes
GROUP BY cpu_range
ORDER BY process_count DESC
"

# Memory usage statistics
sentinelcli query "
SELECT
    AVG(memory_usage) as avg_memory,
    MIN(memory_usage) as min_memory,
    MAX(memory_usage) as max_memory,
    COUNT(*) as process_count
FROM processes
WHERE memory_usage IS NOT NULL
"
```

### Process Investigation

**Deep Process Analysis**:

```bash
# Get detailed information about a specific process
sentinelcli query "
SELECT
    pid,
    name,
    executable_path,
    command_line,
    start_time,
    cpu_usage,
    memory_usage,
    executable_hash,
    user_id,
    collection_time
FROM processes
WHERE pid = 1234
"

# Find processes with the same executable
sentinelcli query "
SELECT
    executable_path,
    COUNT(*) as instance_count,
    GROUP_CONCAT(pid) as pids
FROM processes
WHERE executable_path IS NOT NULL
GROUP BY executable_path
HAVING COUNT(*) > 1
ORDER BY instance_count DESC
"

# Process execution timeline
sentinelcli query "
SELECT
    pid,
    name,
    collection_time,
    cpu_usage,
    memory_usage
FROM processes
WHERE name = 'chrome'
ORDER BY collection_time DESC
LIMIT 20
"
```

## Alert Management

### Viewing Alerts

**List Recent Alerts**:

```bash
# Last 10 alerts
sentinelcli alerts list --limit 10

# Alerts by severity
sentinelcli alerts list --severity high,critical

# Alerts by rule
sentinelcli alerts list --rule "suspicious-processes"

# Alerts from specific time range
sentinelcli alerts list --since "2024-01-15 10:00:00" --until "2024-01-15 18:00:00"
```

**Alert Details**:

```bash
# Get detailed information about a specific alert
sentinelcli alerts show <alert-id>

# Export alerts to file
sentinelcli alerts export --format json --output alerts.json

# Export alerts with filters
sentinelcli alerts export --severity high,critical --format csv --output critical_alerts.csv
```

### Alert Filtering and Search

**Advanced Alert Queries**:

```bash
# Alerts affecting specific processes
sentinelcli query "
SELECT
    a.id,
    a.title,
    a.severity,
    a.alert_time,
    a.affected_processes
FROM alerts a
WHERE JSON_EXTRACT(a.alert_data, '$.pid') = 1234
ORDER BY a.alert_time DESC
"

# Alerts by hostname
sentinelcli query "
SELECT
    a.id,
    a.title,
    a.severity,
    a.alert_time,
    JSON_EXTRACT(a.alert_data, '$.hostname') as hostname
FROM alerts a
WHERE JSON_EXTRACT(a.alert_data, '$.hostname') = 'server-01'
ORDER BY a.alert_time DESC
"

# Alert frequency by rule
sentinelcli query "
SELECT
    rule_id,
    COUNT(*) as alert_count,
    MAX(alert_time) as last_alert
FROM alerts
GROUP BY rule_id
ORDER BY alert_count DESC
"
```

### Alert Response

**Acknowledge Alerts**:

```bash
# Acknowledge a specific alert
sentinelcli alerts acknowledge <alert-id> --comment "Investigating"

# Acknowledge multiple alerts
sentinelcli alerts acknowledge --rule "suspicious-processes" --comment "False positive"

# List acknowledged alerts
sentinelcli alerts list --status acknowledged
```

**Alert Suppression**:

```bash
# Suppress alerts for a specific rule
sentinelcli alerts suppress --rule "suspicious-processes" --duration "1h" --reason "Maintenance"

# Suppress alerts for specific processes
sentinelcli alerts suppress --process 1234 --duration "30m" --reason "Known good process"

# List active suppressions
sentinelcli alerts suppressions list
```

## Rule Management

### Rule Operations

**List Rules**:

```bash
# List all rules
sentinelcli rules list

# List enabled rules only
sentinelcli rules list --enabled

# List rules by category
sentinelcli rules list --category "malware"

# List rules by severity
sentinelcli rules list --severity high,critical
```

**Rule Validation**:

```bash
# Validate a rule file
sentinelcli rules validate /path/to/rule.sql

# Validate all rules
sentinelcli rules validate --all

# Test a rule with sample data
sentinelcli rules test /path/to/rule.sql --sample-data
```

**Rule Management**:

```bash
# Enable a rule
sentinelcli rules enable suspicious-processes

# Disable a rule
sentinelcli rules disable suspicious-processes

# Update a rule
sentinelcli rules update suspicious-processes --file /path/to/new-rule.sql

# Delete a rule
sentinelcli rules delete suspicious-processes
```

### Rule Development

**Create a New Rule**:

```bash
# Create a new rule file
cat > /etc/sentineld/rules/custom-rule.sql << 'EOF'
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
EOF

# Validate the rule
sentinelcli rules validate /etc/sentineld/rules/custom-rule.sql

# Enable the rule
sentinelcli rules enable custom-rule
```

**Rule Testing**:

```bash
# Test rule against current data
sentinelcli rules test custom-rule --live

# Test rule with specific time range
sentinelcli rules test custom-rule --since "2024-01-15 00:00:00" --until "2024-01-15 23:59:59"

# Test rule performance
sentinelcli rules test custom-rule --benchmark
```

### Rule Import/Export

**Export Rules**:

```bash
# Export all rules
sentinelcli rules export --output rules-backup.tar.gz

# Export specific rules
sentinelcli rules export --rules "suspicious-processes,high-cpu" --output selected-rules.tar.gz

# Export rules by category
sentinelcli rules export --category "malware" --output malware-rules.tar.gz
```

**Import Rules**:

```bash
# Import rules from file
sentinelcli rules import rules-backup.tar.gz

# Import rules with validation
sentinelcli rules import rules-backup.tar.gz --validate

# Import rules with conflict resolution
sentinelcli rules import rules-backup.tar.gz --resolve-conflicts
```

## System Health Monitoring

### Performance Metrics

**System Performance**:

```bash
# View system metrics
sentinelcli metrics

# CPU usage over time
sentinelcli metrics --metric cpu_usage --duration 1h

# Memory usage over time
sentinelcli metrics --metric memory_usage --duration 1h

# Process collection rate
sentinelcli metrics --metric collection_rate --duration 1h
```

**Database Performance**:

```bash
# Database status
sentinelcli database status

# Database size
sentinelcli database size

# Database performance metrics
sentinelcli database metrics

# Database maintenance
sentinelcli database maintenance --vacuum
```

### Log Analysis

**View Logs**:

```bash
# Recent logs
sentinelcli logs --tail 50

# Logs by level
sentinelcli logs --level error

# Logs by component
sentinelcli logs --component procmond

# Logs with filters
sentinelcli logs --filter "error" --tail 100
```

**Log Analysis**:

```bash
# Error frequency
sentinelcli logs --analyze --level error

# Performance issues
sentinelcli logs --analyze --filter "slow"

# Security events
sentinelcli logs --analyze --filter "security"
```

## Configuration Management

### Configuration Files

**View Configuration**:

```bash
# Show current configuration
sentinelcli config show

# Show specific configuration section
sentinelcli config show alerting

# Show configuration with defaults
sentinelcli config show --include-defaults
```

**Update Configuration**:

```bash
# Update configuration value
sentinelcli config set app.scan_interval_ms 60000

# Update multiple values
sentinelcli config set alerting.sinks[0].enabled true

# Reload configuration
sentinelcli config reload
```

**Configuration Validation**:

```bash
# Validate configuration file
sentinelcli config validate /etc/sentineld/config.yaml

# Validate current configuration
sentinelcli config validate

# Check configuration for issues
sentinelcli config check
```

### Environment Management

**Environment Variables**:

```bash
# Set environment variables
export SENTINELD_LOG_LEVEL=debug
export SENTINELD_DATABASE_PATH=/var/lib/sentineld/events.redb

# View environment configuration
sentinelcli config show --environment
```

**Service Configuration**:

```bash
# Update service configuration
sudo systemctl edit sentineld

# Reload service configuration
sudo systemctl daemon-reload
sudo systemctl restart sentineld
```

## Troubleshooting

### Common Issues

**Service Won't Start**:

```bash
# Check service status
sudo systemctl status sentineld

# Check logs for errors
sudo journalctl -u sentineld -f

# Check configuration
sentinelcli config validate

# Check permissions
ls -la /var/lib/sentineld/
```

**Database Issues**:

```bash
# Check database status
sentinelcli database status

# Check database integrity
sentinelcli database integrity-check

# Repair database
sentinelcli database repair

# Rebuild database
sentinelcli database rebuild
```

**Alert Delivery Issues**:

```bash
# Check alert sink status
sentinelcli alerts sinks status

# Test alert delivery
sentinelcli alerts test-delivery

# Check network connectivity
sentinelcli network test

# View delivery logs
sentinelcli logs --filter "delivery"
```

### Debug Mode

**Enable Debug Logging**:

```bash
# Set debug log level
sentinelcli config set app.log_level debug

# Restart service
sudo systemctl restart sentineld

# Monitor debug logs
sentinelcli logs --level debug --tail 100
```

**Component Debugging**:

```bash
# Debug procmond
sudo sentinelcli debug procmond --verbose

# Debug sentinelagent
sentinelcli debug sentinelagent --verbose

# Debug database
sentinelcli debug database --verbose
```

### Performance Issues

**High CPU Usage**:

```bash
# Check process collection rate
sentinelcli metrics --metric collection_rate

# Reduce scan interval
sentinelcli config set app.scan_interval_ms 60000

# Check for problematic rules
sentinelcli rules list --performance
```

**High Memory Usage**:

```bash
# Check memory usage
sentinelcli metrics --metric memory_usage

# Reduce batch size
sentinelcli config set app.batch_size 500

# Check database size
sentinelcli database size
```

**Slow Queries**:

```bash
# Check query performance
sentinelcli database query-stats

# Optimize database
sentinelcli database optimize

# Check for slow rules
sentinelcli rules list --slow
```

## Best Practices

### Security

1. **Regular Updates**: Keep SentinelD updated to the latest version
2. **Access Control**: Limit access to SentinelD configuration and data
3. **Audit Logging**: Enable comprehensive audit logging
4. **Network Security**: Use secure connections for remote management
5. **Backup**: Regularly backup configuration and database

### Performance

1. **Resource Monitoring**: Monitor CPU, memory, and disk usage
2. **Rule Optimization**: Optimize detection rules for performance
3. **Database Maintenance**: Regular database maintenance and cleanup
4. **Alert Tuning**: Tune alert thresholds to reduce noise
5. **Capacity Planning**: Plan for growth in process count and data volume

### Operations

1. **Documentation**: Document custom rules and configurations
2. **Testing**: Test rules and configurations in non-production environments
3. **Monitoring**: Set up comprehensive monitoring and alerting
4. **Incident Response**: Develop procedures for security incidents
5. **Training**: Train operators on SentinelD features and best practices

### Maintenance

1. **Regular Backups**: Backup configuration and database regularly
2. **Log Rotation**: Implement log rotation to prevent disk space issues
3. **Database Cleanup**: Regular cleanup of old data
4. **Rule Review**: Regular review and update of detection rules
5. **Performance Tuning**: Regular performance analysis and tuning

---

*This operator guide provides comprehensive instructions for managing SentinelD in production environments. For additional help, consult the troubleshooting section or contact support.*
