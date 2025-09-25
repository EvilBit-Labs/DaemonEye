# macOS-Specific Process Collection Features

This document describes the macOS-specific optimizations and features implemented in the `MacOSProcessCollector`.

## Overview

The `MacOSProcessCollector` provides enhanced process monitoring capabilities on macOS systems by utilizing native libproc and sysctl APIs. This collector offers superior performance and metadata collection compared to the cross-platform `SysinfoProcessCollector`.

## Key Features

### 1. Native libproc API Integration

- **Direct Process Enumeration**: Uses `proc_listpids()` for efficient process discovery
- **Enhanced Metadata**: Leverages `proc_pidinfo()` with `PROC_PIDTASKALLINFO` for detailed process information
- **Executable Path Resolution**: Uses `proc_pidpath()` for accurate executable path detection
- **Performance**: Significantly faster than parsing `/proc` or using higher-level APIs

### 2. macOS Entitlements Detection

The collector provides basic entitlements detection capabilities:

- **Privilege Detection**: Identifies processes running with elevated privileges (UID 0)
- **Sandbox Detection**: Detects sandboxed processes using process flags
- **System Access**: Determines if processes have system-level access capabilities
- **Network/Filesystem Access**: Infers access permissions based on sandbox status

### 3. System Integrity Protection (SIP) Awareness

- **SIP Status Detection**: Automatically detects if SIP is enabled on the system
- **Protected Path Recognition**: Identifies processes running from SIP-protected locations
- **Graceful Handling**: Adapts collection behavior based on SIP restrictions

### 4. Code Signing Detection

Basic code signature validation using heuristic analysis:

- **System Binary Detection**: Identifies likely signed system binaries
- **Path-Based Analysis**: Uses executable paths to infer signing status
- **User Binary Handling**: Distinguishes between system and user-installed applications

### 5. Bundle Information Extraction

- **Bundle ID Generation**: Attempts to extract or generate bundle identifiers for applications
- **Team ID Detection**: Identifies Apple-signed processes and system components
- **Application Path Analysis**: Parses `.app` bundle structures for metadata

### 6. Enhanced Process Metadata

The collector provides comprehensive process information:

- **Memory Statistics**: Virtual memory, resident memory, and memory footprint
- **Thread Information**: Thread count and priority information
- **Architecture Detection**: Process and system architecture identification
- **Command Line Arguments**: Full command line parsing using `KERN_PROCARGS2`
- **Start Time**: Accurate process start time from BSD process info

### 7. Advanced System Process Detection

Comprehensive system process identification:

- **macOS-Specific Processes**: Recognizes common macOS system processes and daemons
- **Pattern Matching**: Identifies system processes by naming conventions
- **Apple Process Detection**: Recognizes `com.apple.*` bundle identifiers
- **Daemon Detection**: Identifies daemon processes by naming patterns

## Configuration Options

The `MacOSCollectorConfig` provides fine-grained control over collection behavior:

```rust
pub struct MacOSCollectorConfig {
    /// Whether to collect process entitlements information
    pub collect_entitlements: bool,
    /// Whether to check SIP protection status
    pub check_sip_protection: bool,
    /// Whether to collect code signing information
    pub collect_code_signing: bool,
    /// Whether to collect bundle information
    pub collect_bundle_info: bool,
    /// Whether to handle sandboxed processes gracefully
    pub handle_sandboxed_processes: bool,
}
```

## Performance Characteristics

### Benchmarks

- **Process Enumeration**: < 5 seconds for 10,000+ processes
- **Memory Usage**: < 100MB during collection
- **CPU Overhead**: < 5% sustained during continuous monitoring
- **Collection Rate**: > 1,000 processes/second

### Optimizations

- **Blocking Task Isolation**: CPU-intensive operations run in blocking tasks
- **Efficient Memory Management**: Minimal allocations during collection
- **Error Resilience**: Individual process failures don't stop collection
- **Graceful Degradation**: Continues operation when advanced features are unavailable

## Security Considerations

### Privilege Management

- **Minimal Privileges**: Operates with standard user privileges when possible
- **Enhanced Access**: Can optionally request debugging entitlements for additional metadata
- **Immediate Dropping**: Drops elevated privileges immediately after initialization
- **Audit Logging**: All privilege operations are logged for security auditing

### Sandboxed Process Handling

- **Graceful Failures**: Handles sandboxed process access restrictions
- **Partial Data Collection**: Collects available metadata when full access is denied
- **Error Classification**: Distinguishes between access denied and process not found

### System Integrity Protection

- **SIP Compliance**: Respects SIP restrictions on system process access
- **Protected Path Awareness**: Identifies and handles SIP-protected processes appropriately
- **Fallback Mechanisms**: Provides alternative collection methods when SIP blocks access

## Error Handling

The collector implements comprehensive error handling:

### Error Types

- **`LibprocError`**: Native libproc API failures
- **`SysctlError`**: sysctl API call failures
- **`EntitlementsError`**: Entitlements detection failures
- **`SipError`**: SIP status detection failures
- **`SandboxError`**: Sandboxed process access issues

### Recovery Strategies

- **Graceful Degradation**: Continues with reduced functionality when errors occur
- **Detailed Logging**: Provides comprehensive error information for debugging
- **Fallback Methods**: Uses alternative collection methods when primary methods fail
- **Statistics Tracking**: Maintains detailed statistics about collection success/failure rates

## Integration with collector-core

The `MacOSProcessCollector` integrates seamlessly with the collector-core framework:

- **EventSource Trait**: Implements the standard `EventSource` interface
- **Capability Advertisement**: Reports available features to the orchestrator
- **Lifecycle Management**: Supports start/stop operations and health checks
- **Event Generation**: Produces standardized `ProcessEvent` structures

## Testing

Comprehensive integration tests verify macOS-specific functionality:

- **Basic Collection**: Verifies process enumeration and metadata extraction
- **Enhanced Features**: Tests entitlements, SIP, and code signing detection
- **Error Handling**: Validates graceful handling of various error conditions
- **Performance**: Ensures collection meets performance requirements
- **Security**: Verifies proper privilege handling and security boundaries

## Future Enhancements

Planned improvements for future versions:

1. **Real-time Monitoring**: Integration with macOS kernel event notifications
2. **Enhanced Code Signing**: Full Security framework integration for signature validation
3. **Detailed Entitlements**: Complete entitlements parsing using Security framework
4. **Network Correlation**: Integration with network monitoring for process-to-connection mapping
5. **Container Detection**: Enhanced detection of containerized and virtualized processes

## Compatibility

- **macOS Versions**: Tested on macOS 10.15+ (Catalina and later)
- **Architectures**: Supports both Intel (x86_64) and Apple Silicon (arm64)
- **System Requirements**: No additional dependencies beyond standard macOS APIs
- **Permissions**: Operates with standard user permissions, enhanced features require debugging entitlements

## Usage Examples

### Basic Usage

```rust
use procmond::macos_collector::{MacOSProcessCollector, MacOSCollectorConfig};
use procmond::process_collector::ProcessCollectionConfig;

let base_config = ProcessCollectionConfig::default();
let macos_config = MacOSCollectorConfig::default();
let collector = MacOSProcessCollector::new(base_config, macos_config)?;

let (events, stats) = collector.collect_processes().await?;
println!("Collected {} processes", events.len());
```

### Enhanced Configuration

```rust
let base_config = ProcessCollectionConfig {
    collect_enhanced_metadata: true,
    max_processes: 1000,
    skip_system_processes: false,
    ..Default::default()
};

let macos_config = MacOSCollectorConfig {
    collect_entitlements: true,
    check_sip_protection: true,
    collect_code_signing: true,
    collect_bundle_info: true,
    handle_sandboxed_processes: true,
};

let collector = MacOSProcessCollector::new(base_config, macos_config)?;
```

This implementation provides a solid foundation for macOS-specific process monitoring while maintaining compatibility with the broader DaemonEye architecture.
