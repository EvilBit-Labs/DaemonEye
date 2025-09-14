# SentinelD Installation Guide

This guide provides comprehensive installation instructions for SentinelD across different platforms and deployment scenarios.

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
wget https://github.com/sentineld/sentineld/releases/latest/download/sentineld-linux-x86_64.tar.gz
tar -xzf sentineld-linux-x86_64.tar.gz

# Linux ARM64
wget https://github.com/sentineld/sentineld/releases/latest/download/sentineld-linux-aarch64.tar.gz
tar -xzf sentineld-linux-aarch64.tar.gz

# macOS x86_64
curl -L https://github.com/sentineld/sentineld/releases/latest/download/sentineld-macos-x86_64.tar.gz | tar -xz

# macOS ARM64 (Apple Silicon)
curl -L https://github.com/sentineld/sentineld/releases/latest/download/sentineld-macos-aarch64.tar.gz | tar -xz

# Windows x86_64
# Download from GitHub releases and extract
```

**Install to System Directories**:

```bash
# Linux/macOS
sudo cp procmond sentinelagent sentinelcli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/sentinelagent /usr/local/bin/sentinelcli

# Create system directories
sudo mkdir -p /etc/sentineld
sudo mkdir -p /var/lib/sentineld
sudo mkdir -p /var/log/sentineld

# Set ownership
sudo chown -R $USER:$USER /etc/sentineld
sudo chown -R $USER:$USER /var/lib/sentineld
sudo chown -R $USER:$USER /var/log/sentineld

# Windows
# Copy to C:\Program Files\SentinelD\
# Add to PATH environment variable
```

### Method 2: Package Managers

**Homebrew (macOS)**:

```bash
# Add SentinelD tap
brew tap sentineld/sentineld

# Install SentinelD
brew install sentineld

# Start service
brew services start sentineld
```

**APT (Ubuntu/Debian)**:

```bash
# Add repository key
wget -qO - https://packages.sentineld.com/apt/key.gpg | sudo apt-key add -

# Add repository
echo "deb https://packages.sentineld.com/apt stable main" | sudo tee /etc/apt/sources.list.d/sentineld.list

# Update package list
sudo apt update

# Install SentinelD
sudo apt install sentineld

# Start service
sudo systemctl start sentineld
sudo systemctl enable sentineld
```

**YUM/DNF (RHEL/CentOS/Fedora)**:

```bash
# Add repository
sudo tee /etc/yum.repos.d/sentineld.repo << 'EOF'
[sentineld]
name=SentinelD
baseurl=https://packages.sentineld.com/yum/stable/
enabled=1
gpgcheck=1
gpgkey=https://packages.sentineld.com/apt/key.gpg
EOF

# Install SentinelD
sudo yum install sentineld  # RHEL/CentOS
# or
sudo dnf install sentineld  # Fedora

# Start service
sudo systemctl start sentineld
sudo systemctl enable sentineld
```

**Chocolatey (Windows)**:

```powershell
# Install SentinelD
choco install sentineld

# Start service
Start-Service SentinelD
```

### Method 3: From Source

**Install Rust** (1.85+):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup update
```

**Clone and Build**:

```bash
# Clone repository
git clone https://github.com/sentineld/sentineld.git
cd sentineld

# Build in release mode
cargo build --release

# Install built binaries
sudo cp target/release/procmond target/release/sentinelagent target/release/sentinelcli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/sentinelagent /usr/local/bin/sentinelcli
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

SentinelD uses `cargo-dist` and `cargo-release` for automated building, packaging, and releasing. This is the recommended method for developers and contributors who want to build release-quality binaries.

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
> **For Contributors**: If you're contributing to SentinelD and need to test your changes, use `cargo dist build` to create release-quality binaries that match the official distribution format.

## Platform-Specific Installation

### Linux Installation

**Ubuntu/Debian**:

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install dependencies
sudo apt install -y ca-certificates curl wget gnupg lsb-release

# Add SentinelD repository
wget -qO - https://packages.sentineld.com/apt/key.gpg | sudo apt-key add -
echo "deb https://packages.sentineld.com/apt stable main" | sudo tee /etc/apt/sources.list.d/sentineld.list

# Install SentinelD
sudo apt update
sudo apt install sentineld

# Configure service
sudo systemctl enable sentineld
sudo systemctl start sentineld
```

**RHEL/CentOS**:

```bash
# Update system
sudo yum update -y

# Install dependencies
sudo yum install -y ca-certificates curl wget

# Add SentinelD repository
sudo tee /etc/yum.repos.d/sentineld.repo << 'EOF'
[sentineld]
name=SentinelD
baseurl=https://packages.sentineld.com/yum/stable/
enabled=1
gpgcheck=1
gpgkey=https://packages.sentineld.com/apt/key.gpg
EOF

# Install SentinelD
sudo yum install sentineld

# Configure service
sudo systemctl enable sentineld
sudo systemctl start sentineld
```

**Arch Linux**:

```bash
# Install from AUR
yay -S sentineld

# Or build from source
git clone https://aur.archlinux.org/sentineld.git
cd sentineld
makepkg -si
```

### macOS Installation

**Using Homebrew**:

```bash
# Install Homebrew if not already installed
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Add SentinelD tap
brew tap sentineld/sentineld

# Install SentinelD
brew install sentineld

# Start service
brew services start sentineld
```

**Manual Installation**:

```bash
# Download and extract
curl -L https://github.com/sentineld/sentineld/releases/latest/download/sentineld-macos-x86_64.tar.gz | tar -xz

# Install to system directories
sudo cp procmond sentinelagent sentinelcli /usr/local/bin/
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/sentinelagent /usr/local/bin/sentinelcli

# Create directories
sudo mkdir -p /Library/Application\ Support/SentinelD
sudo mkdir -p /var/lib/sentineld
sudo mkdir -p /var/log/sentineld

# Set ownership
sudo chown -R $(whoami):staff /Library/Application\ Support/SentinelD
sudo chown -R $(whoami):staff /var/lib/sentineld
sudo chown -R $(whoami):staff /var/log/sentineld
```

### Windows Installation

**Using Chocolatey**:

```powershell
# Install Chocolatey if not already installed
Set-ExecutionPolicy Bypass -Scope Process -Force; [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072; iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))

# Install SentinelD
choco install sentineld

# Start service
Start-Service SentinelD
```

**Manual Installation**:

```powershell
# Download from GitHub releases
# Extract to C:\Program Files\SentinelD\

# Add to PATH
$env:PATH += ";C:\Program Files\SentinelD"

# Create data directories
New-Item -ItemType Directory -Path "C:\ProgramData\SentinelD" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\SentinelD\data" -Force
New-Item -ItemType Directory -Path "C:\ProgramData\SentinelD\logs" -Force
```

## Service Configuration

### Linux (systemd)

**Create Service File**:

```bash
sudo tee /etc/systemd/system/sentineld.service << 'EOF'
[Unit]
Description=SentinelD Security Monitoring Agent
Documentation=https://docs.sentineld.com
After=network.target
Wants=network.target

[Service]
Type=notify
User=sentineld
Group=sentineld
ExecStart=/usr/local/bin/sentinelagent --config /etc/sentineld/config.yaml
ExecReload=/bin/kill -HUP $MAINPID
KillMode=mixed
KillSignal=SIGTERM
TimeoutStopSec=30
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=sentineld

# Security settings
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/sentineld /var/log/sentineld
CapabilityBoundingSet=CAP_SYS_PTRACE
AmbientCapabilities=CAP_SYS_PTRACE

[Install]
WantedBy=multi-user.target
EOF
```

**Create User and Directories**:

```bash
# Create sentineld user
sudo useradd -r -s /bin/false -d /var/lib/sentineld sentineld

# Set ownership
sudo chown -R sentineld:sentineld /var/lib/sentineld
sudo chown -R sentineld:sentineld /var/log/sentineld
sudo chown -R sentineld:sentineld /etc/sentineld

# Reload systemd and start service
sudo systemctl daemon-reload
sudo systemctl enable sentineld
sudo systemctl start sentineld
```

### macOS (launchd)

**Create LaunchDaemon**:

```bash
sudo tee /Library/LaunchDaemons/com.sentineld.agent.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sentineld.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/sentinelagent</string>
        <string>--config</string>
        <string>/Library/Application Support/SentinelD/config.yaml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/sentineld/agent.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/sentineld/agent.error.log</string>
    <key>UserName</key>
    <string>sentineld</string>
    <key>GroupName</key>
    <string>staff</string>
</dict>
</plist>
EOF
```

**Load and Start Service**:

```bash
# Load service
sudo launchctl load /Library/LaunchDaemons/com.sentineld.agent.plist

# Check status
sudo launchctl list | grep sentineld
```

### Windows (Service)

**Create Service**:

```powershell
# Create service
New-Service -Name "SentinelD Agent" -BinaryPathName "C:\Program Files\SentinelD\sentinelagent.exe --config C:\ProgramData\SentinelD\config.yaml" -DisplayName "SentinelD Security Monitoring Agent" -StartupType Automatic

# Start service
Start-Service "SentinelD Agent"

# Check status
Get-Service "SentinelD Agent"
```

## Post-Installation Setup

### Generate Initial Configuration

```bash
# Generate default configuration
sentinelcli config init --output /etc/sentineld/config.yaml

# Or for user-specific configuration
sentinelcli config init --output ~/.config/sentineld/config.yaml
```

### Create Data Directories

```bash
# Linux/macOS
sudo mkdir -p /var/lib/sentineld
sudo mkdir -p /var/log/sentineld
sudo chown -R $USER:$USER /var/lib/sentineld
sudo chown -R $USER:$USER /var/log/sentineld

# Windows
mkdir "C:\ProgramData\SentinelD\data"
mkdir "C:\ProgramData\SentinelD\logs"
```

### Set Up Basic Rules

```bash
# Create rules directory
mkdir -p /etc/sentineld/rules

# Create a basic rule
cat > /etc/sentineld/rules/suspicious-processes.sql << 'EOF'
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
sentinelcli rules validate /etc/sentineld/rules/suspicious-processes.sql
```

### Configure Alerting

```bash
# Enable syslog alerts
sentinelcli config set alerting.sinks[0].enabled true
sentinelcli config set alerting.sinks[0].type syslog
sentinelcli config set alerting.sinks[0].facility daemon

# Enable webhook alerts (if SIEM is available)
sentinelcli config set alerting.sinks[1].enabled true
sentinelcli config set alerting.sinks[1].type webhook
sentinelcli config set alerting.sinks[1].url "https://your-siem.com/webhook"
sentinelcli config set alerting.sinks[1].headers.Authorization "Bearer ${WEBHOOK_TOKEN}"
```

## Verification

### Check Installation

```bash
# Check binary versions
procmond --version
sentinelagent --version
sentinelcli --version

# Check service status
# Linux
sudo systemctl status sentineld

# macOS
sudo launchctl list | grep sentineld

# Windows
Get-Service "SentinelD Agent"
```

### Test Basic Functionality

```bash
# Check system health
sentinelcli health

# List recent processes
sentinelcli query "SELECT pid, name, executable_path FROM processes LIMIT 10"

# Check alerts
sentinelcli alerts list

# Test rule execution
sentinelcli rules test suspicious-processes
```

### Performance Verification

```bash
# Check system metrics
sentinelcli metrics

# Monitor process collection
sentinelcli watch processes --filter "cpu_usage > 10.0"

# Check database status
sentinelcli database status
```

## Troubleshooting

### Common Installation Issues

**Permission Denied**:

```bash
# Check file permissions
ls -la /usr/local/bin/procmond
ls -la /usr/local/bin/sentinelagent
ls -la /usr/local/bin/sentinelcli

# Fix permissions
sudo chmod +x /usr/local/bin/procmond /usr/local/bin/sentinelagent /usr/local/bin/sentinelcli
```

**Service Won't Start**:

```bash
# Check service logs
# Linux
sudo journalctl -u sentineld -f

# macOS
sudo log show --predicate 'process == "sentinelagent"' --last 1h

# Windows
Get-EventLog -LogName Application -Source "SentinelD" -Newest 10
```

**Configuration Errors**:

```bash
# Validate configuration
sentinelcli config validate

# Check configuration syntax
sentinelcli config check

# Show effective configuration
sentinelcli config show --include-defaults
```

**Database Issues**:

```bash
# Check database status
sentinelcli database status

# Check database integrity
sentinelcli database integrity-check

# Repair database
sentinelcli database repair
```

### Debug Mode

```bash
# Enable debug logging
sentinelcli config set app.log_level debug

# Restart service
# Linux
sudo systemctl restart sentineld

# macOS
sudo launchctl unload /Library/LaunchDaemons/com.sentineld.agent.plist
sudo launchctl load /Library/LaunchDaemons/com.sentineld.agent.plist

# Windows
Restart-Service "SentinelD Agent"

# Monitor debug logs
sentinelcli logs --level debug --tail 100
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

### Getting Help

- **Documentation**: Check the full documentation in `docs/`
- **Logs**: Review logs with `sentinelcli logs`
- **Health Checks**: Use `sentinelcli health` for system status
- **Community**: Join discussions on GitHub or community forums
- **Support**: Contact support for commercial assistance

---

*This installation guide provides comprehensive instructions for installing SentinelD across different platforms. For additional help, consult the troubleshooting section or contact support.*
