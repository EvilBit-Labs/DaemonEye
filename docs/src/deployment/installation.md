# DaemonEye Installation Guide

This guide provides comprehensive installation instructions for DaemonEye across different platforms and deployment scenarios.

---

## Table of Contents

[TOC]

---

## System Requirements

### Minimum Requirements

**Operating System**:

- Linux: Kernel 3.10+ (Ubuntu 16.04+, RHEL 7.6+, Debian 9+)
- macOS: 10.14+ (Mojave or later)
- Windows: Windows 10+ or Windows Server 2016+

**Hardware**:

- CPU: x86_64 or ARM64 processor
- RAM: 512MB available memory
- Disk: 1GB free space
- Network: Internet access for initial setup (optional)

**Privileges**:

- Linux: `CAP_SYS_PTRACE` capability or root access
- Windows: `SeDebugPrivilege` or Administrator access
- macOS: Appropriate entitlements or root access

### Recommended Requirements

**Operating System**:

- Linux: Kernel 4.15+ (Ubuntu 18.04+, RHEL 8+, Debian 10+)
- macOS: 11+ (Big Sur or later)
- Windows: Windows 11+ or Windows Server 2019+

**Hardware**:

- CPU: 2+ cores
- RAM: 2GB+ available memory
- Disk: 10GB+ free space
- Network: Stable internet connection

**Enhanced Features** (Enterprise Tier):

- Linux: Kernel 4.7+ for eBPF support
- Windows: Windows 7+ for ETW support
- macOS: 10.15+ for EndpointSecurity support

## Installation Methods

### Method 1: Pre-built Binaries (Recommended)

**Download Latest Release**:

```bash
# Linux x86_64
wget https://github.com/EvilBit-Labs/DaemonEye/releases/latest/download/daemoneye-linux-x86_64.tar.gz
tar -xzf daemoneye-linux-x86_64.tar.gz

# Linux ARM64
wget https://github.com/EvilBit-Labs/DaemonEye/releases/latest/download/daemoneye-linux-aarch64.tar.gz
tar -xzf daemoneye-linux-aarch64.tar.gz

# macOS x86_64
curl -L https://github.com/EvilBit-Labs/DaemonEye/releases/latest/download/daemoneye-macos-x86_64.tar.gz | tar -xz

# macOS ARM64 (Apple Silicon)
curl -L https://github.com/EvilBit-Labs/DaemonEye/releases/latest/download/daemoneye-macos-aarch64.tar.gz | tar -xz

# Windows x86_64
# Download from GitHub releases and extract
```

**Install to System Directories**:

```bash
# Linux/macOS
sudo cp procmond daemoneye-agent daemoneye-cli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli

# Create system directories
sudo mkdir -p /etc/daemoneye
sudo mkdir -p /var/lib/daemoneye
sudo mkdir -p /var/log/daemoneye

# Set ownership
sudo chown -R $USER:$USER /etc/daemoneye
sudo chown -R $USER:$USER /var/lib/daemoneye
sudo chown -R $USER:$USER /var/log/daemoneye

# Windows
# Copy to C:\Program Files\DaemonEye\
# Add to PATH environment variable
```

### Method 2: Package Managers (Planned)

Package manager support (Homebrew, APT, YUM/DNF, Chocolatey) is currently under development. Installers will be available in future releases.

For now, use one of the following installation methods:

- **Pre-built Binaries** (Method 1) - Recommended for most users
- **Build from Source** (Method 3) - For developers and advanced users

### Method 3: Build from Source

**Install Rust** (1.87+):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup update
```

**Clone and Build**:

```bash
# Clone repository
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye

# Build in release mode
cargo build --release

# Install built binaries
sudo cp target/release/procmond target/release/daemoneye-agent target/release/daemoneye-cli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli
```

**Cross-Platform Building**:

```bash
# Install cross-compilation toolchain
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin

# Build for different targets
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
```

### Method 4: Using GoReleaser (Release Tooling)

DaemonEye uses [GoReleaser](https://goreleaser.com/) for automated cross-platform building, packaging, and releasing. This is the recommended method for developers and contributors who want to build release-quality binaries.

**Local build with GoReleaser**:

```bash
# Validate configuration
just goreleaser-check

# Build binaries locally (snapshot, no publish)
just goreleaser-build

# Run a full snapshot release (build + package, no publish)
just goreleaser-snapshot
```

**Release with cargo-release**:

```bash
# Dry run to see what would be changed
cargo release --dry-run

# Prepare a new release (updates version, creates tag)
cargo release --execute

# Release with specific version
cargo release 1.0.0 --execute
```

**GoReleaser Configuration**:

The project includes platform-specific GoReleaser configs (`.goreleaser-linux.yaml`, `.goreleaser-macos.yaml`, `.goreleaser-windows.yaml`) that define:

- **Supported platforms**: Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64, aarch64)
- **Package formats**: `.tar.gz` for Unix, `.zip` for Windows
- **Binaries**: procmond, daemoneye-agent, daemoneye-cli
- **Signing**: Cosign keyless signing via GitHub Actions OIDC

**Release Workflow**:

```bash
# 1. Update version and create tag
cargo release --execute

# 2. Push tag to trigger CI release
git push --tags

# 3. GoReleaser builds, packages, signs, and publishes to GitHub Releases
```

> [!NOTE]
> **For Contributors**: Use `just goreleaser-build` to create release-quality binaries that match the official distribution format.

## Platform-Specific Installation

### Linux Installation

**Ubuntu/Debian - Build from Source**:

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install dependencies
sudo apt install -y ca-certificates curl wget build-essential

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone and build
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye
cargo build --release

# Install binaries
sudo cp target/release/procmond target/release/daemoneye-agent target/release/daemoneye-cli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli

# Create system directories
sudo mkdir -p /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye
sudo chown -R $USER:$USER /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye

# Configure service
sudo systemctl enable daemoneye
sudo systemctl start daemoneye
```

**RHEL/CentOS - Build from Source**:

```bash
# Update system
sudo yum update -y

# Install dependencies
sudo yum install -y ca-certificates curl wget gcc g++ make

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone and build
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye
cargo build --release

# Install binaries
sudo cp target/release/procmond target/release/daemoneye-agent target/release/daemoneye-cli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli

# Create system directories
sudo mkdir -p /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye
sudo chown -R $USER:$USER /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye

# Configure service
sudo systemctl enable daemoneye
sudo systemctl start daemoneye
```

**Arch Linux - Build from Source**:

```bash
# Install dependencies
sudo pacman -S --needed base-devel rust

# Clone and build
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye
cargo build --release

# Install binaries
sudo install -Dm755 target/release/procmond /usr/local/bin/procmond
sudo install -Dm755 target/release/daemoneye-agent /usr/local/bin/daemoneye-agent
sudo install -Dm755 target/release/daemoneye-cli /usr/local/bin/daemoneye-cli

# Create system directories
sudo mkdir -p /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye
```

### macOS Installation

**Using Homebrew** *(Planned)*:

Homebrew package support for DaemonEye is coming soon. For now, please use the build from source or manual installation methods below.

**Build from Source**:

```bash
# Clone the repository
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye

# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Build DaemonEye
cargo build --release

# Install binaries
sudo install -Dm755 target/release/procmond /usr/local/bin/procmond
sudo install -Dm755 target/release/daemoneye-agent /usr/local/bin/daemoneye-agent
sudo install -Dm755 target/release/daemoneye-cli /usr/local/bin/daemoneye-cli

# Create system directories
sudo mkdir -p /etc/daemoneye /var/lib/daemoneye /var/log/daemoneye
```

**Manual Installation**:

```bash
# Download and extract
curl -L https://github.com/EvilBit-Labs/DaemonEye/releases/latest/download/daemoneye-macos-x86_64.tar.gz | tar -xz

# Install to system directories
sudo cp procmond daemoneye-agent daemoneye-cli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli

# Create directories
sudo mkdir -p /Library/Application\ Support/DaemonEye
sudo mkdir -p /var/lib/daemoneye
sudo mkdir -p /var/log/daemoneye

# Set ownership
sudo chown -R $(whoami):staff /Library/Application\ Support/DaemonEye
sudo chown -R $(whoami):staff /var/lib/daemoneye
sudo chown -R $(whoami):staff /var/log/daemoneye
```

### Windows Installation

**Using Chocolatey** *(Planned)*:

Chocolatey package support for DaemonEye is coming soon. For now, please use the build from source or manual installation methods below.

**Build from Source**:

```powershell
# Install Rust (from https://rustup.rs/)
# Download and run rustup-init.exe, or use:
# iwr https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe -OutFile rustup-init.exe
# .\rustup-init.exe -y

# Clone the repository
git clone https://github.com/EvilBit-Labs/DaemonEye.git
cd DaemonEye

# Build DaemonEye
cargo build --release

# Create installation directory
New-Item -ItemType Directory -Path "C:\Program Files\DaemonEye" -Force

# Install binaries
Copy-Item "target\release\procmond.exe" "C:\Program Files\DaemonEye\"
Copy-Item "target\release\daemoneye-agent.exe" "C:\Program Files\DaemonEye\"
Copy-Item "target\release\daemoneye-cli.exe" "C:\Program Files\DaemonEye\"

# Add to PATH (run as Administrator)
[Environment]::SetEnvironmentVariable("PATH", "$env:PATH;C:\Program Files\DaemonEye", [EnvironmentVariableTarget]::Machine)

# Create data directories
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye\data" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye\logs" -Force
```

**Manual Installation**:

```powershell
# Download from GitHub releases
# https://github.com/EvilBit-Labs/DaemonEye/releases
# Extract to C:\Program Files\DaemonEye\

# Add to PATH (run as Administrator)
[Environment]::SetEnvironmentVariable("PATH", "$env:PATH;C:\Program Files\DaemonEye", [EnvironmentVariableTarget]::Machine)

# Create data directories
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye\data" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\DaemonEye\logs" -Force
```

## Service Configuration

### Linux (systemd)

**Create Service File**:

```bash
sudo tee /etc/systemd/system/daemoneye.service << 'EOF'
[Unit]
Description=DaemonEye Security Monitoring Agent
Documentation=https://docs.daemoneye.com
After=network.target
Wants=network.target

[Service]
Type=notify
User=daemoneye
Group=daemoneye
ExecStart=/usr/local/bin/daemoneye-agent --config /etc/daemoneye/config.yaml
ExecReload=/bin/kill -HUP $MAINPID
KillMode=mixed
KillSignal=SIGTERM
TimeoutStopSec=30
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=daemoneye

# Security settings
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/daemoneye /var/log/daemoneye
CapabilityBoundingSet=CAP_SYS_PTRACE
AmbientCapabilities=CAP_SYS_PTRACE

[Install]
WantedBy=multi-user.target
EOF
```

**Create User and Directories**:

```bash
# Create daemoneye user
sudo useradd -r -s /bin/false -d /var/lib/daemoneye daemoneye

# Set ownership
sudo chown -R daemoneye:daemoneye /var/lib/daemoneye
sudo chown -R daemoneye:daemoneye /var/log/daemoneye
sudo chown -R daemoneye:daemoneye /etc/daemoneye

# Reload systemd and start service
sudo systemctl daemon-reload
sudo systemctl enable daemoneye
sudo systemctl start daemoneye
```

### macOS (launchd)

**Create LaunchDaemon**:

```bash
sudo tee /Library/LaunchDaemons/com.daemoneye.agent.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.daemoneye.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/daemoneye-agent</string>
        <string>--config</string>
        <string>/Library/Application Support/DaemonEye/config.yaml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/daemoneye/agent.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/daemoneye/agent.error.log</string>
    <key>UserName</key>
    <string>daemoneye</string>
    <key>GroupName</key>
    <string>staff</string>
</dict>
</plist>
EOF
```

**Load and Start Service**:

```bash
# Load service
sudo launchctl load /Library/LaunchDaemons/com.daemoneye.agent.plist

# Check status
sudo launchctl list | grep daemoneye
```

### Windows (Service)

**Create Service**:

```powershell
# Create service
New-Service -Name "DaemonEye Agent" -BinaryPathName "C:\Program Files\DaemonEye\daemoneye-agent.exe --config C:\ProgramData\DaemonEye\config.yaml" -DisplayName "DaemonEye Security Monitoring Agent" -StartupType Automatic

# Start service
Start-Service "DaemonEye Agent"

# Check status
Get-Service "DaemonEye Agent"
```

## Post-Installation Setup

### Generate Initial Configuration

```bash
# Generate default configuration
daemoneye-cli config init --output /etc/daemoneye/config.yaml

# Or for user-specific configuration
daemoneye-cli config init --output ~/.config/daemoneye/config.yaml
```

### Create Data Directories

```bash
# Linux/macOS
sudo mkdir -p /var/lib/daemoneye
sudo mkdir -p /var/log/daemoneye
sudo chown -R $USER:$USER /var/lib/daemoneye
sudo chown -R $USER:$USER /var/log/daemoneye

# Windows
mkdir "C:\ProgramData\DaemonEye\data"
mkdir "C:\ProgramData\DaemonEye\logs"
```

### Set Up Basic Rules

```bash
# Create rules directory
mkdir -p /etc/daemoneye/rules

# Create a basic rule
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
```

### Configure Alerting

```bash
# Enable syslog alerts
daemoneye-cli config set alerting.sinks[0].enabled true
daemoneye-cli config set alerting.sinks[0].type syslog
daemoneye-cli config set alerting.sinks[0].facility daemon

# Enable webhook alerts (if SIEM is available)
daemoneye-cli config set alerting.sinks[1].enabled true
daemoneye-cli config set alerting.sinks[1].type webhook
daemoneye-cli config set alerting.sinks[1].url "https://your-siem.com/webhook"
daemoneye-cli config set alerting.sinks[1].headers.Authorization "Bearer ${WEBHOOK_TOKEN}"
```

## Verification

### Check Installation

```bash
# Check binary versions
procmond --version
daemoneye-agent --version
daemoneye-cli --version

# Check service status
# Linux
sudo systemctl status daemoneye

# macOS
sudo launchctl list | grep daemoneye

# Windows
Get-Service "DaemonEye Agent"
```

### Test Basic Functionality

```bash
# Check system health
daemoneye-cli health

# List recent processes
daemoneye-cli query "SELECT pid, name, executable_path FROM processes LIMIT 10"

# Check alerts
daemoneye-cli alerts list

# Test rule execution
daemoneye-cli rules test suspicious-processes
```

### Performance Verification

```bash
# Check system metrics
daemoneye-cli metrics

# Monitor process collection
daemoneye-cli watch processes --filter "cpu_usage > 10.0"

# Check database status
daemoneye-cli database status
```

## Troubleshooting

### Common Installation Issues

**Permission Denied**:

```bash
# Check file permissions
ls -la /usr/local/bin/procmond
ls -la /usr/local/bin/daemoneye-agent
ls -la /usr/local/bin/daemoneye-cli

# Fix permissions
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/daemoneye-agent /usr/local/bin/daemoneye-cli
```

**Service Won't Start**:

```bash
# Check service logs
# Linux
sudo journalctl -u daemoneye -f

# macOS
sudo log show --predicate 'process == "daemoneye-agent"' --last 1h

# Windows
Get-EventLog -LogName Application -Source "DaemonEye" -Newest 10
```

**Configuration Errors**:

```bash
# Validate configuration
daemoneye-cli config validate

# Check configuration syntax
daemoneye-cli config check

# Show effective configuration
daemoneye-cli config show --include-defaults
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

### Debug Mode

```bash
# Enable debug logging
daemoneye-cli config set app.log_level debug

# Restart service
# Linux
sudo systemctl restart daemoneye

# macOS
sudo launchctl unload /Library/LaunchDaemons/com.daemoneye.agent.plist
sudo launchctl load /Library/LaunchDaemons/com.daemoneye.agent.plist

# Windows
Restart-Service "DaemonEye Agent"

# Monitor debug logs
daemoneye-cli logs --level debug --tail 100
```

### Performance Issues

**High CPU Usage**:

```bash
# Check process collection rate
daemoneye-cli metrics --metric collection_rate

# Reduce scan interval
daemoneye-cli config set app.scan_interval_ms 60000

# Check for problematic rules
daemoneye-cli rules list --performance
```

**High Memory Usage**:

```bash
# Check memory usage
daemoneye-cli metrics --metric memory_usage

# Reduce batch size
daemoneye-cli config set app.batch_size 500

# Check database size
daemoneye-cli database size
```

**Slow Queries**:

```bash
# Check query performance
daemoneye-cli database query-stats

# Optimize database
daemoneye-cli database optimize

# Check for slow rules
daemoneye-cli rules list --slow
```

### Getting Help

- **Documentation**: Check the full documentation in `docs/`
- **Logs**: Review logs with `daemoneye-cli logs`
- **Health Checks**: Use `daemoneye-cli health` for system status
- **Community**: Join discussions on GitHub or community forums
- **Support**: Contact support for commercial assistance

---

*This installation guide provides comprehensive instructions for installing DaemonEye across different platforms. For additional help, consult the troubleshooting section or contact support.*
