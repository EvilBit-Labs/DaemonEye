# macOS Process Collector User Manual

This document describes the capabilities and features of the macOS Process Collector for DaemonEye.

## Overview

The macOS Process Collector provides comprehensive process monitoring capabilities specifically designed for macOS systems. It offers enhanced security analysis, detailed metadata collection, and macOS-specific features that go beyond standard process monitoring.

## Core Capabilities

### Process Discovery and Monitoring

The collector can enumerate and monitor all running processes on macOS systems, including:

- **System Processes**: Kernel tasks, system daemons, and Apple system components
- **User Applications**: GUI applications, command-line tools, and background services
- **Sandboxed Processes**: Applications running in sandboxed environments
- **Containerized Processes**: Docker containers, virtual machines, and other isolated environments

### Enhanced Security Analysis

#### Code Signing Detection

- **Signature Validation**: Verifies the authenticity of signed applications
- **Certificate Chain Analysis**: Extracts and validates developer certificates
- **Team ID Identification**: Identifies Apple-signed vs. third-party applications
- **Notarization Status**: Detects notarized applications and their validation status
- **Signature Expiration**: Identifies expired or invalid signatures

#### Entitlements Analysis

- **Sandbox Entitlements**: Detects sandboxed applications and their restrictions
- **System Access**: Identifies processes with elevated system privileges
- **Network Permissions**: Determines network access capabilities
- **File System Access**: Analyzes file system access permissions
- **Hardened Runtime**: Detects applications using hardened runtime features

#### System Integrity Protection (SIP) Awareness

- **SIP Status Detection**: Automatically detects if SIP is enabled
- **Protected Path Recognition**: Identifies processes running from SIP-protected locations
- **Graceful Handling**: Adapts monitoring behavior based on SIP restrictions

### Application Bundle Analysis

#### Bundle Information Extraction

- **Bundle Identifiers**: Extracts unique application bundle IDs
- **Version Information**: Retrieves application version and build numbers
- **Localized Names**: Gets application names in different languages
- **Team Identifiers**: Identifies developer teams and organizations
- **Application Paths**: Analyzes `.app` bundle structures

#### System Process Identification

- **Apple System Components**: Recognizes `com.apple.*` system processes
- **System Daemons**: Identifies system daemons and services
- **Kernel Extensions**: Detects kernel extensions and drivers
- **Launch Agents**: Monitors launch agents and daemons

### Comprehensive Process Metadata

#### Memory and Resource Information

- **Memory Usage**: Virtual memory, resident memory, and memory footprint
- **CPU Statistics**: CPU usage, thread count, and priority information
- **File Descriptors**: Open file and socket information
- **Process Hierarchy**: Parent-child process relationships

#### System Information

- **Architecture Detection**: Process and system architecture (Intel/Apple Silicon)
- **User Information**: User ID, group ID, and user name resolution
- **Start Time**: Accurate process start time and duration
- **Command Line**: Full command line arguments and parameters

#### Security Context

- **Privilege Level**: User vs. system vs. root processes
- **Sandbox Status**: Sandboxed vs. unsandboxed processes
- **Code Signing**: Signed vs. unsigned applications
- **Entitlements**: Available system capabilities and restrictions

## Configuration Options

### Basic Configuration

```rust
let config = MacOSCollectorConfig {
    // Enable entitlements analysis
    collect_entitlements: true,
    
    // Check SIP protection status
    check_sip_protection: true,
    
    // Analyze code signing
    collect_code_signing: true,
    
    // Extract bundle information
    collect_bundle_info: true,
    
    // Handle sandboxed processes gracefully
    handle_sandboxed_processes: true,
    
    // Use Security framework for enhanced analysis
    use_security_framework: true,
    
    // Collect additional system information
    collect_system_info: true,
};
```

### Advanced Features

#### Entitlements Analysis

When `collect_entitlements` is enabled, the collector can detect:

- **Sandbox Entitlements**: `com.apple.security.sandbox`, `com.apple.security.app-sandbox`
- **Network Access**: `com.apple.security.network.client`, `com.apple.security.network.server`
- **File System Access**: `com.apple.security.files.user-selected.read-only`
- **Hardware Access**: `com.apple.security.device.camera`, `com.apple.security.device.microphone`
- **System Integration**: `com.apple.security.automation.apple-events`

#### Code Signing Analysis

When `collect_code_signing` is enabled, the collector provides:

- **Signature Status**: Valid, invalid, expired, or unsigned
- **Certificate Information**: Issuer, subject, and validity dates
- **Team ID**: Apple or third-party developer identification
- **Notarization**: Notarized application status
- **Hardened Runtime**: Library validation and runtime protection

#### Bundle Information

When `collect_bundle_info` is enabled, the collector extracts:

- **Bundle ID**: Unique application identifier (e.g., `com.apple.Safari`)
- **Version**: Application version and build numbers
- **Display Name**: Localized application names
- **Team ID**: Developer team identifier
- **Application Path**: Full path to the application bundle

## Performance Characteristics

### Collection Speed

- **Process Enumeration**: < 5 seconds for 10,000+ processes
- **Collection Rate**: > 1,000 processes per second
- **Memory Usage**: < 100MB during collection
- **CPU Overhead**: < 5% sustained during monitoring

### Scalability

- **Large Systems**: Handles systems with 10,000+ processes efficiently
- **High-Frequency Monitoring**: Supports continuous monitoring with minimal impact
- **Resource Management**: Bounded memory usage and CPU consumption
- **Error Resilience**: Individual process failures don't stop collection

## Security Considerations

### Privilege Requirements

- **Standard User**: Basic process monitoring with standard user privileges
- **Enhanced Features**: Some advanced features may require debugging entitlements
- **Sandboxed Processes**: Gracefully handles processes that cannot be fully analyzed
- **SIP Compliance**: Respects System Integrity Protection restrictions

### Data Privacy

- **Command Line Redaction**: Optional masking of sensitive command line arguments
- **User Data Protection**: No collection of personal user data
- **System Information Only**: Focuses on system and security-relevant information
- **Audit Trail**: All collection activities are logged for security auditing

## Use Cases

### Security Monitoring

- **Malware Detection**: Identify unsigned or suspicious applications
- **Privilege Escalation**: Monitor for processes with unexpected privileges
- **Sandbox Bypass**: Detect applications attempting to escape sandbox restrictions
- **Code Injection**: Identify processes with code signing violations

### Compliance and Auditing

- **Software Inventory**: Maintain accurate inventory of installed applications
- **License Compliance**: Track software usage and licensing
- **Security Posture**: Assess system security configuration
- **Incident Response**: Provide detailed process information for forensic analysis

### System Administration

- **Performance Monitoring**: Track resource usage and system performance
- **Application Management**: Monitor application lifecycle and dependencies
- **System Health**: Detect system issues and anomalies
- **Capacity Planning**: Analyze system resource utilization

## Integration

### DaemonEye Integration

The macOS Process Collector integrates seamlessly with the DaemonEye architecture:

- **Event Generation**: Produces standardized `ProcessEvent` structures
- **Real-time Monitoring**: Supports continuous process monitoring
- **Alert Integration**: Integrates with DaemonEye's alerting system
- **Database Storage**: Stores process information in the event store
- **Query Interface**: Accessible through the CLI and API

### Detection Rules

The collector supports SQL-based detection rules for:

- **Process Patterns**: Identify processes by name, path, or attributes
- **Security Anomalies**: Detect unusual privilege or entitlement patterns
- **Resource Usage**: Monitor CPU, memory, and file system usage
- **Network Activity**: Correlate processes with network connections
- **System Changes**: Detect new or modified applications

## Troubleshooting

### Common Issues

#### Permission Denied Errors

- **Cause**: Insufficient privileges for process access
- **Solution**: Run with appropriate privileges or enable debugging entitlements
- **Workaround**: Collector continues with available information

#### Sandboxed Process Access

- **Cause**: Process is running in a sandboxed environment
- **Solution**: Enable `handle_sandboxed_processes` option
- **Behavior**: Collector provides partial information when possible

#### SIP Protection

- **Cause**: System Integrity Protection blocking access
- **Solution**: Collector automatically adapts to SIP restrictions
- **Behavior**: Provides alternative collection methods when available

### Performance Optimization

#### Large Process Counts

- **Optimization**: Use process filtering to focus on relevant processes
- **Configuration**: Adjust `max_processes` limit for your system
- **Monitoring**: Use collection statistics to identify bottlenecks

#### Memory Usage

- **Optimization**: Enable process filtering to reduce memory usage
- **Configuration**: Adjust batch sizes for your system resources
- **Monitoring**: Monitor memory usage during collection

## Examples

### Basic Process Monitoring

```rust
use procmond::macos_collector::{EnhancedMacOSCollector, MacOSCollectorConfig};
use procmond::process_collector::ProcessCollectionConfig;

// Create collector with default configuration
let base_config = ProcessCollectionConfig::default();
let macos_config = MacOSCollectorConfig::default();
let collector = EnhancedMacOSCollector::new(base_config, macos_config)?;

// Collect all processes
let (events, stats) = collector.collect_processes().await?;
println!("Collected {} processes", events.len());
```

### Security-Focused Monitoring

```rust
// Configure for security analysis
let macos_config = MacOSCollectorConfig {
    collect_entitlements: true,
    collect_code_signing: true,
    collect_bundle_info: true,
    use_security_framework: true,
    ..Default::default()
};

let collector = EnhancedMacOSCollector::new(base_config, macos_config)?;
let (events, stats) = collector.collect_processes().await?;

// Analyze security-relevant processes
for event in events {
    if let Some(entitlements) = &event.entitlements {
        println!("Process {} has entitlements: {:?}", event.name, entitlements);
    }
}
```

### Performance Monitoring

```rust
// Configure for performance monitoring
let base_config = ProcessCollectionConfig {
    collect_enhanced_metadata: true,
    max_processes: 1000,
    ..Default::default()
};

let collector = EnhancedMacOSCollector::new(base_config, macos_config)?;
let (events, stats) = collector.collect_processes().await?;

// Analyze resource usage
for event in events {
    if event.cpu_usage > 10.0 {
        println!("High CPU usage: {} ({}%)", event.name, event.cpu_usage);
    }
}
```

This macOS Process Collector provides comprehensive process monitoring capabilities specifically designed for macOS systems, offering enhanced security analysis, detailed metadata collection, and seamless integration with the DaemonEye platform.
