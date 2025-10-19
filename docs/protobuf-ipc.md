# Protobuf IPC Implementation

This document describes the Protocol Buffer implementation for Inter-Process Communication (IPC) between the `procmond` and `daemoneye-agent` components in DaemonEye.

## Overview

The protobuf schema defines type-safe message contracts that enable efficient and reliable communication between:

- **procmond**: Privileged process collector
- **daemoneye-agent**: Detection orchestrator

## Message Types

### Core Messages

#### DetectionTask

Sent from `daemoneye-agent` to `procmond` to request process data collection.

```protobuf
message DetectionTask {
  string task_id = 1;                    // Unique correlation ID
  TaskType task_type = 2;                // Type of operation
  optional ProcessFilter process_filter = 3;  // Optional filtering
  optional HashCheck hash_check = 4;     // Optional hash verification
  optional string metadata = 5;          // Optional task metadata
}
```

#### DetectionResult

Returned from `procmond` to `daemoneye-agent` with collection results.

```protobuf
message DetectionResult {
  string task_id = 1;                      // Matching correlation ID
  bool success = 2;                        // Operation success status
  optional string error_message = 3;      // Error details if failed
  repeated ProcessRecord processes = 4;    // Collected process data
  optional HashResult hash_result = 5;    // Hash verification result
}
```

### Data Types

#### ProcessRecord

Comprehensive process information structure.

```protobuf
message ProcessRecord {
  uint32 pid = 1;                        // Process ID
  optional uint32 ppid = 2;              // Parent process ID
  string name = 3;                       // Process name
  optional string executable_path = 4;   // Full executable path
  repeated string command_line = 5;      // Command line arguments
  optional int64 start_time = 6;         // Start time (Unix timestamp)
  optional double cpu_usage = 7;         // CPU usage percentage
  optional uint64 memory_usage = 8;      // Memory usage in bytes
  optional string executable_hash = 9;   // File hash (hex-encoded)
  optional string hash_algorithm = 10;   // Hash algorithm name
  optional string user_id = 11;          // User ID
  bool accessible = 12;                  // Data accessibility flag
  bool file_exists = 13;                 // Executable file existence
  int64 collection_time = 14;            // Collection timestamp (millis)
}
```

#### TaskType Enum

Defines the types of operations that can be requested.

```protobuf
enum TaskType {
  ENUMERATE_PROCESSES = 0;    // List all accessible processes
  CHECK_PROCESS_HASH = 1;     // Verify executable hash
  MONITOR_PROCESS_TREE = 2;   // Monitor process hierarchy
  VERIFY_EXECUTABLE = 3;      // Verify executable integrity
}
```

#### ProcessFilter

Criteria for filtering process collection.

```protobuf
message ProcessFilter {
  repeated string process_names = 1;      // Filter by process names
  repeated uint32 pids = 2;               // Filter by process IDs
  optional string executable_pattern = 3; // Filter by path pattern
}
```

#### HashCheck/HashResult

Hash verification request and response.

```protobuf
message HashCheck {
  string expected_hash = 1;      // Expected hash value
  string hash_algorithm = 2;     // Algorithm (e.g., "sha256")
  string executable_path = 3;    // Path to verify
}

message HashResult {
  string hash_value = 1;         // Computed hash
  string algorithm = 2;          // Algorithm used
  string file_path = 3;          // File path processed
  bool success = 4;              // Computation success
  optional string error_message = 5; // Error details if failed
}
```

## Rust Integration

### Module Structure

The protobuf types are available in the `daemoneye_lib::proto` module:

```rust
use daemoneye_lib::proto::{
    DetectionResult, DetectionTask, ProtoHashCheck, ProtoHashResult, ProtoProcessFilter,
    ProtoProcessRecord, ProtoTaskType,
};
```

### Type Conversions

Automatic conversions between native and protobuf types:

```rust
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::proto::ProtoProcessRecord;

fn convert_process_record() {
    // Convert native to protobuf
    let native_process = ProcessRecord::new(1234, "firefox".to_string());
    let proto_process: ProtoProcessRecord = native_process.into();

    // Convert protobuf to native
    let converted_back: ProcessRecord = proto_process.into();
}
```

### Helper Methods

Convenient constructors for common operations:

```rust
fn create_tasks_and_results() {
    // Create enumeration task
    let task = DetectionTask::new_enumerate_processes("task-123", None);

    // Create hash check task
    let hash_check = ProtoHashCheck {
        expected_hash: "abc123...".to_string(),
        hash_algorithm: "sha256".to_string(),
        executable_path: "/usr/bin/firefox".to_string(),
    };
    let task = DetectionTask::new_hash_check("task-456", hash_check);

    // Create results
    let processes = vec![]; // Empty vector for example
    let success = DetectionResult::success("task-123", processes);
    let failure = DetectionResult::failure("task-123", "Permission denied");
}
```

## Build System Integration

### Dependencies

The protobuf implementation requires:

```toml
[dependencies]
prost = "0.13.5"
prost-types = "0.13.5"
serde = { version = "1.0", features = ["derive"] }

[build-dependencies]
prost-build = "0.13.5"
```

### Build Script

Automatic code generation via `build.rs`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::Config::new()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["proto/ipc.proto"], &["proto/"])?;
    Ok(())
}
```

### Generated Code

The build system generates type-safe Rust structs with:

- Automatic serialization/deserialization support
- JSON compatibility via serde
- Optional field handling
- Type safety with strong typing

## Security Considerations

### Message Validation

- All messages include correlation IDs for request/response matching
- Optional fields provide graceful degradation
- Error messages are structured and actionable

### Size Limits

- Individual messages limited to prevent DoS attacks
- Streaming support for large result sets
- Backpressure mechanisms in transport layer

### Type Safety

- Strongly typed enums prevent invalid operations
- Required fields enforce data integrity
- Optional fields provide extensibility

## Usage Examples

### Basic Process Enumeration

```rust
fn basic_process_enumeration() -> Result<(), Box<dyn std::error::Error>> {
    // Create enumeration request
    let task = DetectionTask::new_enumerate_processes("enum-001", None);

    // Serialize for transport
    let bytes = prost::Message::encode_to_vec(&task);

    // Send via IPC transport...

    // Deserialize response
    // Create a mock DetectionResult for demonstration
    let mock_result = DetectionResult {
        task_id: "enum-001".to_string(),
        success: true,
        processes: vec![ProtoProcessRecord {
            pid: 1234,
            name: "example_process".to_string(),
            executable_path: Some("/usr/bin/example".to_string()),
            ppid: Some(1),
            command_line: vec!["example_process --arg".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };

    // Serialize the mock result to demonstrate proper encoding/decoding
    let response_bytes = prost::Message::encode_to_vec(&mock_result);
    let result = DetectionResult::decode(&response_bytes[..])?;

    // Process results
    if result.success {
        for process in result.processes {
            println!("Process: {} (PID: {})", process.name, process.pid);
        }
    }

    Ok(())
}
```

### Filtered Process Collection

```rust
use daemoneye_lib::proto::ProtoProcessFilter;

fn filtered_process_collection() {
    // Create filter for specific processes
    let filter = ProtoProcessFilter {
        process_names: vec!["firefox".to_string(), "chrome".to_string()],
        pids: vec![],
        executable_pattern: Some("/usr/bin/*".to_string()),
    };

    let task = DetectionTask::new_enumerate_processes("filtered-001", Some(filter));
}
```

### Hash Verification

```rust
use daemoneye_lib::proto::ProtoHashCheck;

fn hash_verification() {
    let hash_check = ProtoHashCheck {
        expected_hash: "a1b2c3d4...".to_string(),
        hash_algorithm: "sha256".to_string(),
        executable_path: "/usr/bin/suspicious".to_string(),
    };

    let task = DetectionTask::new_hash_check("verify-001", hash_check);
}
```

## Error Handling

### Structured Errors

Error responses include specific error codes and messages:

```rust
fn handle_error() {
    let result = DetectionResult::failure("task-123", "Permission denied accessing PID 1234");
}
```

### Validation

All messages are validated during deserialization:

```rust
fn validate_message(bytes: &[u8]) -> Result<(), prost::DecodeError> {
    match DetectionTask::decode(bytes) {
        Ok(task) => {
            // Use task here - validate task fields, log task details, etc.
            println!("Successfully decoded task: {:?}", task);
            Ok(())
        }
        Err(e) => {
            eprintln!("Invalid message format: {}", e);
            return Err(e);
        }
    }
}
```

## Performance Characteristics

### Serialization Efficiency

- Binary protobuf format provides compact serialization
- Typical message sizes: 50-500 bytes for tasks, 1-10KB for results
- Zero-copy deserialization where possible

### Memory Usage

- Streaming support prevents unbounded memory growth
- Optional fields reduce memory footprint
- Efficient string handling with Rust's ownership model

## Testing

The implementation includes comprehensive tests for:

- Message creation and validation
- Type conversions between native and protobuf formats
- Serialization/deserialization round-trips
- Error handling and edge cases

Run tests with:

```bash
just test
```

## Future Extensions

The protobuf schema is designed for evolution:

- Optional fields enable backward compatibility
- Reserved field numbers prevent conflicts
- Extensible message types support new features
- Versioned schemas for major changes
