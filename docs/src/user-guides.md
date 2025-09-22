# User Guides

This section contains comprehensive user guides for DaemonEye, covering everything from basic usage to advanced configuration and troubleshooting.

---

## Table of Contents

[TOC]

---

## Operator Guide

The operator guide provides comprehensive information for system administrators and security operators who need to deploy, configure, and maintain DaemonEye in production environments.

[Read Operator Guide →](./user-guides/operator-guide.md)

## Configuration Guide

The configuration guide covers all aspects of DaemonEye configuration, from basic settings to advanced tuning and security hardening.

[Read Configuration Guide →](./user-guides/configuration.md)

## Quick Start

### Installation

Install DaemonEye using your preferred method:

**Using Package Managers**:

```bash
# Ubuntu/Debian
sudo apt install daemoneye

# RHEL/CentOS
sudo yum install daemoneye

# macOS
brew install daemoneye

# Windows
choco install daemoneye
```

**Using Docker**:

```bash
docker run -d --privileged \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  daemoneye/daemoneye:latest
```

**Using Kubernetes**:

```bash
kubectl apply -f https://raw.githubusercontent.com/EvilBit-Labs/daemoneye/main/deploy/kubernetes/daemoneye.yaml
```

### Basic Configuration

Create a basic configuration file:

```yaml
# /etc/daemoneye/config.yaml
app:
  scan_interval_ms: 30000
  batch_size: 1000
  log_level: info
  data_dir: /var/lib/daemoneye
  log_dir: /var/log/daemoneye

database:
  path: /var/lib/daemoneye/processes.db
  retention_days: 30

alerting:
  enabled: true
  sinks:
    - type: syslog
      enabled: true
      facility: daemon
```

### Starting DaemonEye

**Linux (systemd)**:

```bash
sudo systemctl start daemoneye
sudo systemctl enable daemoneye
```

**macOS (launchd)**:

```bash
sudo launchctl load /Library/LaunchDaemons/com.daemoneye.agent.plist
```

**Windows (Service)**:

```powershell
Start-Service DaemonEye
```

**Docker**:

```bash
docker run -d --name daemoneye \
  --privileged \
  -v /etc/daemoneye:/config:ro \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  daemoneye/daemoneye:latest
```

### Basic Usage

**Check Status**:

```bash
daemoneye-cli health
```

**Query Processes**:

```bash
daemoneye-cli query "SELECT pid, name, executable_path FROM processes LIMIT 10"
```

**List Alerts**:

```bash
daemoneye-cli alerts list
```

**View Logs**:

```bash
daemoneye-cli logs --tail 100
```

## Common Tasks

### Process Monitoring

**Monitor Specific Processes**:

```bash
# Monitor processes by name
daemoneye-cli watch processes --filter "name LIKE '%apache%'"

# Monitor processes by CPU usage
daemoneye-cli watch processes --filter "cpu_usage > 10.0"

# Monitor processes by memory usage
daemoneye-cli watch processes --filter "memory_usage > 1000000"
```

**Query Process Information**:

```bash
# Get all processes
daemoneye-cli query "SELECT * FROM processes"

# Get processes by PID
daemoneye-cli query "SELECT * FROM processes WHERE pid = 1234"

# Get processes by name pattern
daemoneye-cli query "SELECT * FROM processes WHERE name LIKE '%nginx%'"

# Get processes by executable path
daemoneye-cli query "SELECT * FROM processes WHERE executable_path LIKE '%/usr/bin/%'"
```

### Alert Management

**Configure Alerting**:

```bash
# Enable syslog alerts
daemoneye-cli config set alerting.sinks[0].enabled true
daemoneye-cli config set alerting.sinks[0].type syslog
daemoneye-cli config set alerting.sinks[0].facility daemon

# Enable webhook alerts
daemoneye-cli config set alerting.sinks[1].enabled true
daemoneye-cli config set alerting.sinks[1].type webhook
daemoneye-cli config set alerting.sinks[1].url "https://alerts.example.com/webhook"
```

**View Alerts**:

```bash
# List recent alerts
daemoneye-cli alerts list

# List alerts by severity
daemoneye-cli alerts list --severity high

# List alerts by rule
daemoneye-cli alerts list --rule "suspicious_processes"

# Get alert details
daemoneye-cli alerts show <alert-id>
```

### Rule Management

**Create Detection Rules**:

```bash
# Create a rule file
cat > /etc/daemoneye/rules/suspicious-processes.sql << 'EOF'
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
daemoneye-cli rules validate /etc/daemoneye/rules/suspicious-processes.sql

# Test the rule
daemoneye-cli rules test suspicious-processes
```

**Manage Rules**:

```bash
# List all rules
daemoneye-cli rules list

# Enable/disable rules
daemoneye-cli rules enable suspicious-processes
daemoneye-cli rules disable suspicious-processes

# Reload rules
daemoneye-cli rules reload
```

### Configuration Management

**View Configuration**:

```bash
# Show current configuration
daemoneye-cli config show

# Show specific setting
daemoneye-cli config get app.scan_interval_ms

# Show all settings with defaults
daemoneye-cli config show --include-defaults
```

**Update Configuration**:

```bash
# Set a single value
daemoneye-cli config set app.scan_interval_ms 60000

# Set multiple values
daemoneye-cli config set app.scan_interval_ms 60000 app.batch_size 500

# Update from file
daemoneye-cli config load /path/to/config.yaml
```

**Validate Configuration**:

```bash
# Validate configuration
daemoneye-cli config validate

# Check configuration syntax
daemoneye-cli config check
```

## Troubleshooting

### Common Issues

**Service Won't Start**:

```bash
# Check service status
sudo systemctl status daemoneye

# Check logs
sudo journalctl -u daemoneye -f

# Check configuration
daemoneye-cli config validate
```

**Permission Denied**:

```bash
# Check file permissions
ls -la /var/lib/daemoneye/
ls -la /var/log/daemoneye/

# Fix permissions
sudo chown -R daemoneye:daemoneye /var/lib/daemoneye
sudo chown -R daemoneye:daemoneye /var/log/daemoneye
```

**Database Issues**:

```bash
# Check database status
daemoneye-cli database status

# Check database integrity
daemoneye-cli database integrity-check

# Repair database
daemoneye-cli database repair
```

**Performance Issues**:

```bash
# Check system metrics
daemoneye-cli metrics

# Check resource usage
daemoneye-cli system status

# Optimize configuration
daemoneye-cli config optimize
```

### Debug Mode

**Enable Debug Logging**:

```bash
# Set debug level
daemoneye-cli config set app.log_level debug

# Restart service
sudo systemctl restart daemoneye

# Monitor debug logs
daemoneye-cli logs --level debug --tail 100
```

**Debug Specific Components**:

```bash
# Debug process collection
daemoneye-cli debug collector

# Debug alert delivery
daemoneye-cli debug alerts

# Debug database operations
daemoneye-cli debug database
```

### Health Checks

**System Health**:

```bash
# Overall health
daemoneye-cli health

# Component health
daemoneye-cli health --component procmond
daemoneye-cli health --component daemoneye-agent
daemoneye-cli health --component database

# Detailed health report
daemoneye-cli health --verbose
```

**Performance Health**:

```bash
# Performance metrics
daemoneye-cli metrics

# Resource usage
daemoneye-cli system resources

# Performance analysis
daemoneye-cli system analyze
```

## Advanced Usage

### Custom Integrations

**SIEM Integration**:

```yaml
# Splunk HEC
integrations:
  siem:
    splunk:
      enabled: true
      hec_url: https://splunk.example.com:8088/services/collector
      hec_token: ${SPLUNK_HEC_TOKEN}
      index: daemoneye

# Elasticsearch
integrations:
  siem:
    elasticsearch:
      enabled: true
      url: https://elasticsearch.example.com:9200
      username: ${ELASTIC_USERNAME}
      password: ${ELASTIC_PASSWORD}
      index: daemoneye-processes
```

**Export Formats**:

```yaml
# CEF Export
integrations:
  export:
    cef:
      enabled: true
      output_file: /var/log/daemoneye/cef.log
      cef_version: "1.0"
      device_vendor: "DaemonEye"
      device_product: "Process Monitor"

# STIX Export
integrations:
  export:
    stix:
      enabled: true
      output_file: /var/log/daemoneye/stix.json
      stix_version: "2.1"
```

### Performance Tuning

**Optimize for High Load**:

```yaml
app:
  scan_interval_ms: 60000  # Reduce scan frequency
  batch_size: 500          # Smaller batches
  max_memory_mb: 256       # Limit memory usage
  max_cpu_percent: 3.0     # Limit CPU usage

database:
  cache_size: -128000      # 128MB cache
  temp_store: MEMORY       # Use memory for temp tables
  synchronous: NORMAL      # Balance safety and performance
```

**Optimize for Low Latency**:

```yaml
app:
  scan_interval_ms: 10000  # Increase scan frequency
  batch_size: 100          # Smaller batches
  max_memory_mb: 512       # More memory for caching

detection:
  enable_rule_caching: true
  cache_ttl_seconds: 300
  max_concurrent_rules: 5
```

### Security Hardening

**Enable Security Features**:

```yaml
security:
  enable_privilege_dropping: true
  drop_to_user: daemoneye
  drop_to_group: daemoneye
  enable_audit_logging: true
  enable_integrity_checking: true
  hash_algorithm: blake3
  enable_signature_verification: true
```

**Network Security**:

```yaml
security:
  network:
    enable_tls: true
    cert_file: /etc/daemoneye/cert.pem
    key_file: /etc/daemoneye/key.pem
    ca_file: /etc/daemoneye/ca.pem
    verify_peer: true
```

## Best Practices

### Deployment

1. **Start Small**: Begin with basic monitoring and gradually add features
2. **Test Configuration**: Always validate configuration before deployment
3. **Monitor Resources**: Keep an eye on CPU and memory usage
4. **Regular Updates**: Keep DaemonEye updated with latest releases
5. **Backup Data**: Regularly backup configuration and data

### Configuration

1. **Use Hierarchical Config**: Leverage multiple configuration sources
2. **Environment Variables**: Use environment variables for secrets
3. **Validation**: Always validate configuration changes
4. **Documentation**: Document custom configurations
5. **Version Control**: Keep configuration files in version control

### Monitoring

1. **Set Up Alerting**: Configure appropriate alert thresholds
2. **Monitor Performance**: Track system performance metrics
3. **Log Analysis**: Regularly review logs for issues
4. **Health Checks**: Implement automated health monitoring
5. **Incident Response**: Have a plan for handling alerts

### Security

1. **Principle of Least Privilege**: Run with minimal required privileges
2. **Network Security**: Use TLS for all network communications
3. **Access Control**: Implement proper authentication and authorization
4. **Audit Logging**: Enable comprehensive audit logging
5. **Regular Updates**: Keep security patches current

---

*This user guide provides comprehensive information for using DaemonEye. For additional help, consult the specific user guides or contact support.*
