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
wget https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-linux-x86_64.tar.gz
tar -xzf daemoneye-linux-x86_64.tar.gz

# Linux ARM64
wget https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-linux-aarch64.tar.gz
tar -xzf daemoneye-linux-aarch64.tar.gz

# macOS x86_64
curl -L https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-macos-x86_64.tar.gz | tar -xz

# macOS ARM64 (Apple Silicon)
curl -L https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-macos-aarch64.tar.gz | tar -xz

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

### Method 2: Package Managers

**Homebrew (macOS)**:

```bash
# Add DaemonEye tap
brew tap daemoneye/daemoneye

# Install DaemonEye
brew install daemoneye

# Start service
brew services start daemoneye
```

**APT (Ubuntu/Debian)**:

```bash
# Add repository key
wget -qO - https://packages.daemoneye.com/apt/key.gpg | sudo apt-key add -

# Add repository
echo "deb https://packages.daemoneye.com/apt stable main" | sudo tee /etc/apt/sources.list.d/daemoneye.list

# Update package list
sudo apt update

# Install DaemonEye
sudo apt install daemoneye

# Start service
sudo systemctl start daemoneye
sudo systemctl enable daemoneye
```

**YUM/DNF (RHEL/CentOS/Fedora)**:

```bash
# Add repository
sudo tee /etc/yum.repos.d/daemoneye.repo << 'EOF'
[daemoneye]
name=DaemonEye
baseurl=https://packages.daemoneye.com/yum/stable/
enabled=1
gpgcheck=1
gpgkey=https://packages.daemoneye.com/apt/key.gpg
EOF

# Install DaemonEye
sudo yum install daemoneye  # RHEL/CentOS
# or
sudo dnf install daemoneye  # Fedora

# Start service
sudo systemctl start daemoneye
sudo systemctl enable daemoneye
```

**Chocolatey (Windows)**:

```powershell
# Install DaemonEye
choco install daemoneye

# Start service
Start-Service DaemonEye
```

### Method 3: From Source

**Install Rust** (1.87+):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup update
```

**Clone and Build**:

```bash
# Clone repository
git clone https://github.com/daemoneye/daemoneye.git
cd daemoneye

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

### Method 4: Using cargo-dist (Release Tooling)

DaemonEye uses `cargo-dist` and `cargo-release` for automated building, packaging, and releasing. This is the recommended method for developers and contributors who want to build release-quality binaries.

**Install cargo-dist and cargo-release**:

```bash
# Install cargo-dist for cross-platform binary distribution
cargo install cargo-dist

# Install cargo-release for automated versioning and releasing
cargo install cargo-release
```

**Build with cargo-dist**:

```bash
# Build and package for all supported platforms
cargo dist build

# Build for specific platforms only
cargo dist build --targets x86_64-unknown-linux-gnu,aarch64-apple-darwin

# Build and create installers
cargo dist build --artifacts=all
```

**Release with cargo-release**:

```bash
# Prepare a new release (updates version, changelog, etc.)
cargo release --execute

# Dry run to see what would be changed
cargo release --dry-run

# Release with specific version
cargo release 1.0.0 --execute
```

**cargo-dist Configuration**:

The project includes a `Cargo.toml` configuration for cargo-dist that defines:

- **Supported platforms**: Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64)
- **Package formats**: Tarballs, ZIP files, and platform-specific installers
- **Asset inclusion**: Binaries, documentation, and configuration templates
- **Signing**: GPG signing for release artifacts

**Benefits of cargo-dist**:

- **Cross-platform builds**: Single command builds for all supported platforms
- **Consistent packaging**: Standardized package formats across platforms
- **Automated signing**: GPG signing of release artifacts
- **Installation scripts**: Platform-specific installation helpers
- **Checksum generation**: Automatic generation of SHA-256 checksums
- **CI/CD integration**: Designed for automated release pipelines

**Release Workflow**:

```bash
# 1. Update version and changelog
cargo release --execute

# 2. Build and package all platforms
cargo dist build --artifacts=all

# 3. Test packages locally
cargo dist test

# 4. Publish to GitHub releases
cargo dist publish
```

> [!NOTE]
> **For Contributors**: If you're contributing to DaemonEye and need to test your changes, use `cargo dist build` to create release-quality binaries that match the official distribution format.

## Platform-Specific Installation

### Linux Installation

**Ubuntu/Debian**:

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install dependencies
sudo apt install -y ca-certificates curl wget gnupg lsb-release

# Add DaemonEye repository
wget -qO - https://packages.daemoneye.com/apt/key.gpg | sudo apt-key add -
echo "deb https://packages.daemoneye.com/apt stable main" | sudo tee /etc/apt/sources.list.d/daemoneye.list

# Install DaemonEye
sudo apt update
sudo apt install daemoneye

# Configure service
sudo systemctl enable daemoneye
sudo systemctl start daemoneye
```

**RHEL/CentOS**:

```bash
# Update system
sudo yum update -y

# Install dependencies
sudo yum install -y ca-certificates curl wget

# Add DaemonEye repository
sudo tee /etc/yum.repos.d/daemoneye.repo << 'EOF'
[daemoneye]
name=DaemonEye
baseurl=https://packages.daemoneye.com/yum/stable/
enabled=1
gpgcheck=1
gpgkey=https://packages.daemoneye.com/apt/key.gpg
EOF

# Install DaemonEye
sudo yum install daemoneye

# Configure service
sudo systemctl enable daemoneye
sudo systemctl start daemoneye
```

**Arch Linux**:

```bash
# Install from AUR
yay -S daemoneye

# Or build from source
git clone https://aur.archlinux.org/daemoneye.git
cd daemoneye
makepkg -si
```

### macOS Installation

**Using Homebrew**:

```bash
# Install Homebrew if not already installed
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Add DaemonEye tap
brew tap daemoneye/daemoneye

# Install DaemonEye
brew install daemoneye

# Start service
brew services start daemoneye
```

**Manual Installation**:

```bash
# Download and extract
curl -L https://github.com/daemoneye/daemoneye/releases/latest/download/daemoneye-macos-x86_64.tar.gz | tar -xz

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

**Using Chocolatey**:

```powershell
# Install Chocolatey if not already installed
Set-ExecutionPolicy Bypass -Scope Process -Force; [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072; iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

# Install DaemonEye
choco install daemoneye

# Start service
Start-Service DaemonEye
```

**Manual Installation**:

```powershell
# Download from GitHub releases
# Extract to C:\Program Files\DaemonEye\

# Add to PATH
$env:PATH += ";C:\Program Files\DaemonEye"

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
