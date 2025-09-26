# Inter-Process Communication (IPC)

## Overview

DaemonEye uses a secure, high-performance IPC system to coordinate between its three components (`procmond`, `daemoneye-agent`, and `daemoneye-cli`). The IPC layer provides reliable cross-platform communication using native OS primitives while maintaining strict security boundaries.

## Transport Architecture

The IPC system is built on the `interprocess` crate, which provides a unified interface for local inter-process communication across different operating systems:

- **Transport**: Cross-platform local sockets via `interprocess::local_socket`
- **Protocol**: Protocol Buffers with CRC32 integrity validation
- **Framing**: Length-delimited frames with error detection
- **Security**: Platform-appropriate permissions and connection limiting
- **Integration**: Full async/await support with Tokio runtime

## Platform-Specific Transports

DaemonEye automatically selects the optimal IPC transport for each platform:

- **Linux/macOS**: Unix domain sockets for maximum performance and security
- **Windows**: Named pipes with appropriate security descriptors
- **All Platforms**: Identical API and behavior regardless of underlying transport

## Protocol Design

### Message Framing

Each IPC message uses a simple, robust framing protocol:

```text
[Length: u32][CRC32: u32][Protobuf Message: N bytes]
```

- **Length Field**: Message size in bytes (little-endian)
- **CRC32 Checksum**: Integrity validation of message content
- **Message Payload**: Protocol Buffer-encoded data

### Security Features

- **Local-only**: No network exposure - all communication is local to the host
- **Permission Control**: Unix sockets use 0600 permissions (owner-only access)
- **Connection Limits**: Configurable maximum concurrent connections
- **Input Validation**: All messages validated before processing
- **Resource Limits**: Configurable frame size limits prevent DoS attacks

## Configuration

The IPC system is configured through the `IpcConfig` structure:

```rust
pub struct IpcConfig {
    pub endpoint_path: String,
    pub max_frame_bytes: usize,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub max_connections: usize,
    pub crc32_variant: Crc32Variant,
}
```

### Configuration Parameters

- **`endpoint_path`**: Platform-specific IPC endpoint location
- **`max_frame_bytes`**: Maximum message size (default: 1MB)
- **`read_timeout_ms`**: Read operation timeout (default: 30 seconds)
- **`write_timeout_ms`**: Write operation timeout (default: 10 seconds)
- **`max_connections`**: Maximum concurrent connections (default: 16)
- **`crc32_variant`**: Checksum algorithm variant (default: IEEE 802.3)

### Endpoint Configuration

#### Default Paths

- **Unix/macOS**: `/var/run/daemoneye/procmond.sock`
- **Windows**: `\\.\pipe\daemoneye\procmond`

#### Custom Endpoints

You can specify custom endpoints in the configuration:

```yaml
# Example configuration (choose one based on platform)
ipc:
  # Unix/macOS
  endpoint_path: /tmp/custom-daemoneye.sock
  # Windows (alternative)
  # endpoint_path: "\\\\.\\pipe\\custom-daemoneye"
```

#### Endpoint Permissions

- Unix sockets: Directory permissions `0700`, socket permissions `0600`
- Windows named pipes: Restricted to the current user by default
- All platforms: No network access - local communication only

## Error Handling and Reliability

### Connection Management

- **Automatic Reconnection**: Clients automatically reconnect with exponential backoff
- **Graceful Degradation**: Components handle IPC failures without crashing
- **Timeout Handling**: Configurable timeouts prevent hanging operations
- **Connection Limiting**: Server-side connection limits prevent resource exhaustion

### Error Types

The IPC layer provides detailed error information:

```rust
use std::io;

pub enum IpcError {
    Timeout,                                    // Operation timed out
    TooLarge { size: usize, max_size: usize },  // Message exceeds size limit
    CrcMismatch { expected: u32, actual: u32 }, // Data corruption detected
    Io(io::Error),                              // Underlying I/O error
    Decode(prost::DecodeError),                 // Protobuf decoding error
    Encode(String),                             // Protobuf encoding error
    PeerClosed,                                 // Connection closed by peer
    InvalidLength { length: usize },            // Invalid message length
}
```

## Performance Characteristics

- **Low Latency**: Direct process-to-process communication without network stack
- **High Throughput**: Efficient binary protocol with minimal overhead
- **Memory Efficient**: Zero-copy deserialization where possible
- **Async/Await**: Non-blocking operations with Tokio integration

## Troubleshooting

### Common Issues

1. **Permission Denied**

   - Check socket file/directory permissions
   - Ensure processes run as same user or with appropriate privileges

2. **Connection Refused**

   - Verify the server component (`procmond`) is running
   - Check endpoint path configuration matches between components

3. **Timeout Errors**

   - Increase timeout values in configuration if needed
   - Check system load and process responsiveness

### Diagnostic Commands

```bash
# Check IPC status
daemoneye-cli health-check --verbose

# Verify endpoint accessibility
ls -la /var/run/daemoneye/  # Unix/macOS

# Test IPC connectivity
daemoneye-cli ipc test
```
