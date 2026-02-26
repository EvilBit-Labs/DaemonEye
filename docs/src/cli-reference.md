# CLI Reference

This document provides comprehensive reference information for all DaemonEye command-line interfaces.

---

## Table of Contents

[TOC]

---

## Overview

DaemonEye provides three main command-line tools:

- **procmond**: Privileged process collector daemon
- **daemoneye-agent**: Detection orchestrator and lifecycle manager
- **daemoneye-cli**: Command-line interface for queries and management

## procmond

The privileged process monitoring daemon that collects process information with minimal attack surface.

### Usage

```bash
procmond [OPTIONS]
```

### Options

| Option                | Short | Default                           | Description                                 |
| --------------------- | ----- | --------------------------------- | ------------------------------------------- |
| `--database`          | `-d`  | `/var/lib/daemoneye/processes.db` | Database path for storing process data      |
| `--log-level`         | `-l`  | `info`                            | Log level (debug, info, warn, error)        |
| `--interval`          | `-i`  | `30`                              | Collection interval in seconds (5-3600)     |
| `--max-processes`     |       | `0`                               | Maximum processes per cycle (0 = unlimited) |
| `--enhanced-metadata` |       |                                   | Enable enhanced metadata collection         |
| `--compute-hashes`    |       |                                   | Enable executable hashing for integrity     |
| `--help`              | `-h`  |                                   | Print help information                      |
| `--version`           | `-V`  |                                   | Print version information                   |

### Examples

```bash
# Basic process monitoring with 30-second intervals
procmond --database /var/lib/daemoneye/processes.db --interval 30

# Enhanced collection with metadata and hashing
procmond --enhanced-metadata --compute-hashes --interval 60

# Debug mode with verbose logging
procmond --log-level debug --interval 10

# Limited collection for testing
procmond --max-processes 100 --interval 5
```

### Configuration

`procmond` is orchestrated by `daemoneye-agent`; collectors do not consume component-specific configuration files. When the binary is launched directly (for example during development or troubleshooting) it honours the following sources:

1. Command-line flags (highest precedence)
2. Environment variables (`PROCMOND_*`) typically injected by the agent
3. System DaemonEye configuration file (`/etc/daemoneye/config.toml`)
4. Embedded defaults (lowest precedence)

Per-user configuration is not supported for collectors; only the operator-facing CLI honours user-scoped overrides when invoked directly.

Operators should configure collection behaviour through the agent, which materialises these settings when spawning the collector.

### Exit Codes

| Code | Description                                                                              |
| ---- | ---------------------------------------------------------------------------------------- |
| 0    | Success                                                                                  |
| 1    | Unhandled error returned from the runtime (includes configuration, permission, database) |
| 2    | CLI argument parsing failure reported by `clap`                                          |

## daemoneye-agent

The detection orchestrator that manages procmond lifecycle, executes detection rules, and handles alerting.

### Usage

```bash
daemoneye-agent [OPTIONS]
```

### Options

| Option        | Short | Default                           | Description                          |
| ------------- | ----- | --------------------------------- | ------------------------------------ |
| `--database`  | `-d`  | `/var/lib/daemoneye/processes.db` | Database path for process data       |
| `--log-level` | `-l`  | `info`                            | Log level (debug, info, warn, error) |
| `--help`      | `-h`  |                                   | Print help information               |
| `--version`   | `-V`  |                                   | Print version information            |

### Examples

```bash
# Start orchestrator with default settings
daemoneye-agent

# Use custom database location
daemoneye-agent --database /custom/path/processes.db

# Enable debug logging
daemoneye-agent --log-level debug

# Test mode (exits immediately for integration tests)
DAEMONEYE_AGENT_TEST_MODE=1 daemoneye-agent
```

### Environment Variables

| Variable                    | Description                                     |
| --------------------------- | ----------------------------------------------- |
| `DAEMONEYE_AGENT_TEST_MODE` | Set to `1` to enable test mode (immediate exit) |

### Features

- **Embedded EventBus Broker**: Runs daemoneye-eventbus broker for multi-collector coordination
- **IPC Server**: Provides IPC server for CLI communication via protobuf over Unix sockets/named pipes
- **IPC Client**: Communicates with procmond via protobuf over Unix sockets/named pipes
- **Detection Engine**: Executes SQL-based detection rules against collected data
- **Alert Management**: Multi-channel alert delivery (stdout, syslog, webhooks, email)
- **Graceful Shutdown**: Handles SIGTERM/SIGINT for clean shutdown

### Configuration

daemoneye-agent supports hierarchical configuration loading:

1. Command-line flags (highest precedence)
2. Environment variables (`DAEMONEYE_AGENT_*`)
3. User configuration file (`~/.config/daemoneye-agent/config.yaml`)
4. System configuration file (`/etc/daemoneye-agent/config.yaml`)
5. Embedded defaults (lowest precedence)

## daemoneye-cli

The command-line interface for querying database statistics, health checks, and system management.

### Usage

```bash
daemoneye-cli [OPTIONS]
```

### Options

| Option       | Short | Default                           | Description                 |
| ------------ | ----- | --------------------------------- | --------------------------- |
| `--database` | `-d`  | `/var/lib/daemoneye/processes.db` | Database path for queries   |
| `--format`   | `-f`  | `human`                           | Output format (human, json) |
| `--help`     | `-h`  |                                   | Print help information      |
| `--version`  | `-V`  |                                   | Print version information   |

### Examples

```bash
# View database statistics in human-readable format
daemoneye-cli --database /var/lib/daemoneye/processes.db --format human

# Get statistics in JSON format for scripting
daemoneye-cli --database /var/lib/daemoneye/processes.db --format json

# Use default database location
daemoneye-cli --format json
```

### Output Formats

#### Human Format

```text
DaemonEye Database Statistics
============================
Processes: 1234
Rules: 5
Alerts: 42
System Info: 1
Scans: 100
Health status: Healthy
```

#### JSON Format

```json
{
  "processes": 1234,
  "rules": 5,
  "alerts": 42,
  "system_info": 1,
  "scans": 100,
  "health_status": "Healthy"
}
```

### Configuration

daemoneye-cli supports hierarchical configuration loading:

1. Command-line flags (highest precedence)
2. Environment variables (`DAEMONEYE_CLI_*`)
3. User configuration file (`~/.config/daemoneye-cli/config.yaml`)
4. System configuration file (`/etc/daemoneye-cli/config.yaml`)
5. Embedded defaults (lowest precedence)

## Common Patterns

### Basic Monitoring Setup

```bash
# Terminal 1: Start the orchestrator
daemoneye-agent --log-level info

# Terminal 2: Monitor database statistics
watch -n 5 'daemoneye-cli --format json'

# Terminal 3: Run procmond directly (optional)
procmond --enhanced-metadata --compute-hashes
```

### Testing and Development

```bash
# Test procmond collection
procmond --interval 5 --max-processes 10 --log-level debug

# Test agent in test mode
DAEMONEYE_AGENT_TEST_MODE=1 daemoneye-agent

# Check database growth
daemoneye-cli --format json | jq '.processes'
```

### Production Deployment

```bash
# Start agent as service
systemctl start daemoneye-agent

# Monitor health
daemoneye-cli --format json | jq '.processes, .alerts'

# Check logs
journalctl -u daemoneye-agent -f
```

## Shell Completions

All DaemonEye CLI tools support shell completions for bash, zsh, fish, and PowerShell.

### Generate Completions

```bash
# Bash
daemoneye-cli --generate-completion bash > /etc/bash_completion.d/daemoneye-cli

# Zsh
daemoneye-cli --generate-completion zsh > ~/.zsh/completions/_daemoneye-cli

# Fish
daemoneye-cli --generate-completion fish > ~/.config/fish/completions/daemoneye-cli.fish

# PowerShell
daemoneye-cli --generate-completion powershell > DaemonEye.ps1
```

## Error Handling

All CLI tools follow consistent error handling patterns:

- **Exit Code 0**: Success
- **Exit Code 1**: General error
- **Exit Code 2**: CLI argument parsing failure
- **Exit Code 3**: Permission denied
- **Exit Code 4**: Database error

### Common Error Messages

| Error                 | Cause                                 | Solution                                                  |
| --------------------- | ------------------------------------- | --------------------------------------------------------- |
| `Permission denied`   | Insufficient privileges               | Run with appropriate privileges or check file permissions |
| `Database locked`     | Another process is using the database | Stop other DaemonEye processes or check for stale locks   |
| `Invalid interval`    | Interval outside 5-3600 range         | Use interval between 5 and 3600 seconds                   |
| `Configuration error` | Invalid configuration file            | Check configuration syntax and values                     |

## Environment Variables

### Global Environment Variables

| Variable         | Description                             | Default        |
| ---------------- | --------------------------------------- | -------------- |
| `NO_COLOR`       | Disable colored output                  | Not set        |
| `TERM`           | Terminal type (affects color detection) | System default |
| `RUST_LOG`       | Rust logging configuration              | Not set        |
| `RUST_BACKTRACE` | Enable Rust backtraces                  | Not set        |

### Component-Specific Variables

#### procmond

| Variable             | Description         | Default                           |
| -------------------- | ------------------- | --------------------------------- |
| `PROCMOND_DATABASE`  | Database path       | `/var/lib/daemoneye/processes.db` |
| `PROCMOND_LOG_LEVEL` | Log level           | `info`                            |
| `PROCMOND_INTERVAL`  | Collection interval | `30`                              |

#### daemoneye-agent

| Variable                    | Description      | Default                           |
| --------------------------- | ---------------- | --------------------------------- |
| `DAEMONEYE_AGENT_DATABASE`  | Database path    | `/var/lib/daemoneye/processes.db` |
| `DAEMONEYE_AGENT_LOG_LEVEL` | Log level        | `info`                            |
| `DAEMONEYE_AGENT_TEST_MODE` | Enable test mode | Not set                           |

#### daemoneye-cli

| Variable                 | Description   | Default                           |
| ------------------------ | ------------- | --------------------------------- |
| `DAEMONEYE_CLI_DATABASE` | Database path | `/var/lib/daemoneye/processes.db` |
| `DAEMONEYE_CLI_FORMAT`   | Output format | `human`                           |

## Integration Examples

### Systemd Service

```ini
[Unit]
Description=DaemonEye Agent
After=network.target

[Service]
Type=simple
User=daemoneye
Group=daemoneye
ExecStart=/usr/local/bin/daemoneye-agent --database /var/lib/daemoneye/processes.db
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Docker Deployment

```dockerfile
FROM rust:1.91-slim as builder
COPY . /app
WORKDIR /app
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/daemoneye-agent /usr/local/bin/
COPY --from=builder /app/target/release/daemoneye-cli /usr/local/bin/
COPY --from=builder /app/target/release/procmond /usr/local/bin/

VOLUME ["/data"]

CMD ["daemoneye-agent", "--database", "/data/processes.db"]
```

### Kubernetes DaemonSet

```yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: daemoneye
spec:
  selector:
    matchLabels:
      app: daemoneye
  template:
    metadata:
      labels:
        app: daemoneye
    spec:
      hostPID: true
      containers:
        - name: daemoneye-agent
          image: daemoneye/daemoneye:latest
          securityContext:
            privileged: true
          volumeMounts:
            - name: data
              mountPath: /data
            - name: proc
              mountPath: /host/proc
              readOnly: true
      volumes:
        - name: data
          hostPath:
            path: /var/lib/daemoneye
        - name: proc
          hostPath:
            path: /proc
```

---

*This CLI reference provides comprehensive information for using DaemonEye command-line tools. For additional help, use the `--help` flag with any command or consult the user guides.*
