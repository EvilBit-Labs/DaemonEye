# Testing Documentation

This document provides comprehensive testing strategies and guidelines for DaemonEye, covering unit testing, integration testing, performance testing, and security testing.

---

## Table of Contents

[TOC]

---

## Testing Philosophy

DaemonEye follows a comprehensive testing strategy that ensures:

- **Reliability**: Robust error handling and edge case coverage
- **Performance**: Meets performance requirements under load
- **Security**: Validates security controls and prevents vulnerabilities
- **Maintainability**: Easy to understand and modify tests
- **Coverage**: High test coverage across all components

## Testing Strategy

### Three-Tier Testing Architecture

1. **Unit Tests**: Test individual components in isolation
2. **Integration Tests**: Test component interactions and data flow
3. **End-to-End Tests**: Test complete workflows and user scenarios

### Testing Pyramid

```text
        ┌─────────────────┐
        │   E2E Tests     │  ← Few, slow, expensive
        │   (Manual)      │
        ├─────────────────┤
        │ Integration     │  ← Some, medium speed
        │ Tests           │
        ├─────────────────┤
        │   Unit Tests    │  ← Many, fast, cheap
        │   (Automated)   │
        └─────────────────┘
```

## Unit Testing

### Core Testing Framework

DaemonEye uses a comprehensive unit testing framework:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_process_collection() {
        let collector = ProcessCollector::new();
        let processes = collector.collect_processes().await.unwrap();

        assert!(!processes.is_empty());
        assert!(processes.iter().any(|p| p.pid > 0));
    }

    #[tokio::test]
    async fn test_database_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db = Database::new(&db_path).await.unwrap();
        let process = ProcessInfo {
            pid: 1234,
            name: "test_process".to_string(),
            // ... other fields
        };

        db.insert_process(&process).await.unwrap();
        let retrieved = db.get_process(1234).await.unwrap();

        assert_eq!(process.pid, retrieved.pid);
        assert_eq!(process.name, retrieved.name);
    }
}
```

### Mocking and Test Doubles

Use mocks for external dependencies:

```rust
use mockall::mock;

mock! {
    pub ProcessCollector {}

    #[async_trait]
    impl ProcessCollectionService for ProcessCollector {
        async fn collect_processes(&self) -> Result<CollectionResult, CollectionError>;
        async fn get_system_info(&self) -> Result<SystemInfo, CollectionError>;
    }
}

#[tokio::test]
async fn test_agent_with_mock_collector() {
    let mut mock_collector = MockProcessCollector::new();
    mock_collector
        .expect_collect_processes()
        .times(1)
        .returning(|| Ok(CollectionResult::default()));

    let agent = SentinelAgent::new(Box::new(mock_collector));
    let result = agent.run_collection_cycle().await;

    assert!(result.is_ok());
}
```

### Property-Based Testing

Use property-based testing for complex logic:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_process_info_serialization(process in any::<ProcessInfo>()) {
        let serialized = serde_json::to_string(&process).unwrap();
        let deserialized: ProcessInfo = serde_json::from_str(&serialized).unwrap();
        assert_eq!(process, deserialized);
    }

    #[test]
    fn test_sql_query_validation(query in "[a-zA-Z0-9_\\s]+") {
        let result = validate_sql_query(&query);
        // Property: validation should not panic
        let _ = result;
    }
}
```

## Integration Testing

### Database Integration Tests

Test database operations with real SQLite:

```rust
#[tokio::test]
async fn test_database_integration() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("integration_test.db");

    let db = Database::new(&db_path).await.unwrap();

    // Test schema creation
    db.create_schema().await.unwrap();

    // Test data insertion
    let process = ProcessInfo {
        pid: 1234,
        name: "test_process".to_string(),
        executable_path: Some("/usr/bin/test".to_string()),
        command_line: Some("test --arg value".to_string()),
        start_time: Some(Utc::now()),
        cpu_usage: Some(0.5),
        memory_usage: Some(1024),
        status: ProcessStatus::Running,
        executable_hash: Some("abc123".to_string()),
        collection_time: Utc::now(),
    };

    db.insert_process(&process).await.unwrap();

    // Test data retrieval
    let retrieved = db.get_process(1234).await.unwrap();
    assert_eq!(process.pid, retrieved.pid);

    // Test query execution
    let results = db.query_processes("SELECT * FROM processes WHERE pid = ?", &[1234]).await.unwrap();
    assert_eq!(results.len(), 1);
}
```

### IPC Integration Tests

Test inter-process communication:

```rust
#[tokio::test]
async fn test_ipc_communication() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    // Start server
    let server = IpcServer::new(&socket_path).await.unwrap();
    let server_handle = tokio::spawn(async move {
        server.run().await
    });

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect client
    let client = IpcClient::new(&socket_path).await.unwrap();

    // Test request/response
    let request = IpcRequest::CollectProcesses;
    let response = client.send_request(request).await.unwrap();

    assert!(matches!(response, IpcResponse::Processes(_)));

    // Cleanup
    server_handle.abort();
}
```

### Alert Delivery Integration Tests

Test alert delivery mechanisms:

```rust
#[tokio::test]
async fn test_alert_delivery() {
    let mut alert_manager = AlertManager::new();

    // Add test sinks
    let syslog_sink = SyslogSink::new("daemon").unwrap();
    let webhook_sink = WebhookSink::new("http://localhost:8080/webhook").unwrap();

    alert_manager.add_sink(Box::new(syslog_sink));
    alert_manager.add_sink(Box::new(webhook_sink));

    // Create test alert
    let alert = Alert {
        id: Uuid::new_v4(),
        rule_name: "test_rule".to_string(),
        severity: AlertSeverity::High,
        message: "Test alert".to_string(),
        process: ProcessInfo::default(),
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    };

    // Send alert
    let result = alert_manager.send_alert(alert).await;
    assert!(result.is_ok());
}
```

## End-to-End Testing

### CLI Testing

Test command-line interface:

```rust
use insta::assert_snapshot;
use std::process::Command;

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("sentinelcli").unwrap();
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("DaemonEye CLI"));
}

#[test]
fn test_cli_query() {
    let mut cmd = Command::cargo_bin("sentinelcli").unwrap();
    cmd.args(&["query", "SELECT * FROM processes LIMIT 1"])
        .assert()
        .success();
}

#[test]
fn test_cli_config() {
    let mut cmd = Command::cargo_bin("sentinelcli").unwrap();
    cmd.args(&["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("app:"));
}
```

### Full System Testing

Test complete system workflows:

```rust
#[tokio::test]
async fn test_full_system_workflow() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    // Create test configuration
    let config = Config::default();
    config.save_to_file(&config_path).unwrap();

    // Start procmond
    let procmond_handle = tokio::spawn(async move {
        let procmond = ProcMonD::new(&config_path).await.unwrap();
        procmond.run().await
    });

    // Start daemoneye-agent
    let agent_handle = tokio::spawn(async move {
        let agent = SentinelAgent::new(&config_path).await.unwrap();
        agent.run().await
    });

    // Wait for services to start
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Test CLI operations
    let mut cmd = Command::cargo_bin("sentinelcli").unwrap();
    cmd.args(&["--config", config_path.to_str().unwrap(), "query", "SELECT COUNT(*) FROM processes"])
        .assert()
        .success();

    // Cleanup
    procmond_handle.abort();
    agent_handle.abort();
}
```

## Performance Testing

### Load Testing

Test system performance under load:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_process_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_collection");

    group.bench_function("collect_processes", |b| {
        b.iter(|| {
            let collector = ProcessCollector::new();
            black_box(collector.collect_processes())
        })
    });

    group.bench_function("collect_processes_parallel", |b| {
        b.iter(|| {
            let collector = ProcessCollector::new();
            black_box(collector.collect_processes_parallel())
        })
    });

    group.finish();
}

fn benchmark_database_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("database_operations");

    group.bench_function("insert_process", |b| {
        let db = Database::new(":memory:").unwrap();
        let process = ProcessInfo::default();

        b.iter(|| black_box(db.insert_process(&process)))
    });

    group.bench_function("query_processes", |b| {
        let db = Database::new(":memory:").unwrap();
        // Insert test data
        for i in 0..1000 {
            let process = ProcessInfo {
                pid: i,
                ..Default::default()
            };
            db.insert_process(&process).unwrap();
        }

        b.iter(|| black_box(db.query_processes("SELECT * FROM processes WHERE pid > ?", &[500])))
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_process_collection,
    benchmark_database_operations
);
criterion_main!(benches);
```

### Memory Testing

Test memory usage and leaks:

```rust
#[tokio::test]
async fn test_memory_usage() {
    let initial_memory = get_memory_usage();

    // Run operations that should not leak memory
    for _ in 0..1000 {
        let collector = ProcessCollector::new();
        let _processes = collector.collect_processes().await.unwrap();
        drop(collector);
    }

    // Force garbage collection
    tokio::task::yield_now().await;

    let final_memory = get_memory_usage();
    let memory_increase = final_memory - initial_memory;

    // Memory increase should be minimal
    assert!(memory_increase < 10 * 1024 * 1024); // 10MB
}

fn get_memory_usage() -> usize {
    // Platform-specific memory usage detection
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                return parts[1].parse::<usize>().unwrap() * 1024; // Convert to bytes
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Fallback for other platforms
        0
    }
}
```

### Stress Testing

Test system behavior under stress:

```rust
#[tokio::test]
async fn test_stress_collection() {
    let collector = ProcessCollector::new();

    // Run collection continuously for 60 seconds
    let start = Instant::now();
    let mut count = 0;

    while start.elapsed() < Duration::from_secs(60) {
        let processes = collector.collect_processes().await.unwrap();
        count += processes.len();

        // Small delay to prevent overwhelming the system
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Should have collected a reasonable number of processes
    assert!(count > 0);
    println!("Collected {} processes in 60 seconds", count);
}
```

## Security Testing

### Fuzz Testing

Test with random inputs:

```rust
use cargo_fuzz;

#[no_mangle]
pub extern "C" fn fuzz_process_info(data: &[u8]) {
    if let Ok(process_info) = ProcessInfo::from_bytes(data) {
        // Test that deserialization doesn't panic
        let _ = process_info.pid;
        let _ = process_info.name;
    }
}

#[no_mangle]
pub extern "C" fn fuzz_sql_query(data: &[u8]) {
    if let Ok(query) = std::str::from_utf8(data) {
        // Test SQL query validation
        let _ = validate_sql_query(query);
    }
}
```

### Security Boundary Testing

Test security boundaries:

```rust
#[tokio::test]
async fn test_privilege_dropping() {
    let collector = ProcessCollector::new();

    // Should start with elevated privileges
    assert!(collector.has_privileges());

    // Drop privileges
    collector.drop_privileges().await.unwrap();

    // Should no longer have privileges
    assert!(!collector.has_privileges());

    // Should still be able to collect processes (with reduced capabilities)
    let processes = collector.collect_processes().await.unwrap();
    assert!(!processes.is_empty());
}

#[tokio::test]
async fn test_sql_injection_prevention() {
    let db = Database::new(":memory:").unwrap();

    // Test various SQL injection attempts
    let malicious_queries = vec![
        "'; DROP TABLE processes; --",
        "1' OR '1'='1",
        "'; INSERT INTO processes VALUES (9999, 'hacker', '/bin/evil'); --",
    ];

    for query in malicious_queries {
        let result = db.execute_query(query).await;
        // Should either reject the query or sanitize it safely
        match result {
            Ok(_) => {
                // If query succeeds, verify no damage was done
                let count = db.count_processes().await.unwrap();
                assert_eq!(count, 0); // No processes should exist
            }
            Err(_) => {
                // Query was rejected, which is also acceptable
            }
        }
    }
}
```

### Input Validation Testing

Test input validation:

```rust
#[test]
fn test_input_validation() {
    // Test valid inputs
    let valid_process = ProcessInfo {
        pid: 1234,
        name: "valid_process".to_string(),
        executable_path: Some("/usr/bin/valid".to_string()),
        command_line: Some("valid --arg value".to_string()),
        start_time: Some(Utc::now()),
        cpu_usage: Some(0.5),
        memory_usage: Some(1024),
        status: ProcessStatus::Running,
        executable_hash: Some("abc123".to_string()),
        collection_time: Utc::now(),
    };

    assert!(valid_process.validate().is_ok());

    // Test invalid inputs
    let invalid_process = ProcessInfo {
        pid: 0,                                            // Invalid PID
        name: "".to_string(),                              // Empty name
        executable_path: Some("".to_string()),             // Empty path
        command_line: Some("a".repeat(10000).to_string()), // Too long
        start_time: Some(Utc::now()),
        cpu_usage: Some(-1.0), // Negative CPU usage
        memory_usage: Some(0),
        status: ProcessStatus::Running,
        executable_hash: Some("invalid_hash".to_string()),
        collection_time: Utc::now(),
    };

    assert!(invalid_process.validate().is_err());
}
```

## Test Configuration

### Test Environment Setup

```yaml
# test-config.yaml
app:
  log_level: debug
  scan_interval_ms: 1000
  batch_size: 10

database:
  path: ':memory:'
  max_connections: 5
  retention_days: 1

alerting:
  enabled: false

testing:
  enable_mocks: true
  mock_external_services: true
  test_data_dir: /tmp/daemoneye-test
  cleanup_after_tests: true
```

### Test Data Management

```rust
pub struct TestDataManager {
    temp_dir: TempDir,
    test_data: HashMap<String, Vec<u8>>,
}

impl TestDataManager {
    pub fn new() -> Self {
        Self {
            temp_dir: TempDir::new().unwrap(),
            test_data: HashMap::new(),
        }
    }

    pub fn add_test_data(&mut self, name: &str, data: &[u8]) {
        self.test_data.insert(name.to_string(), data.to_vec());
    }

    pub fn get_test_data(&self, name: &str) -> Option<&[u8]> {
        self.test_data.get(name).map(|v| v.as_slice())
    }

    pub fn create_test_database(&self) -> PathBuf {
        let db_path = self.temp_dir.path().join("test.db");
        let db = Database::new(&db_path).unwrap();
        db.create_schema().unwrap();
        db_path
    }
}
```

## Continuous Integration

### GitHub Actions Workflow

```yaml
name: Tests

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        rust: [1.85, stable, beta]
        os: [ubuntu-latest, macos-latest, windows-latest]

    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: ${{ matrix.rust }}
          override: true

      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libsqlite3-dev

      - name: Run tests
        run: |
          cargo test --verbose
          cargo test --verbose --features integration-tests

      - name: Run benchmarks
        run: cargo bench --verbose

      - name: Run fuzz tests
        run: |
          cargo install cargo-fuzz
          cargo fuzz build
          cargo fuzz run process_info
          cargo fuzz run sql_query

      - name: Generate coverage report
        run: |
          cargo install cargo-tarpaulin
          cargo tarpaulin --out Html --output-dir coverage
```

### Test Reporting

```rust
use insta::assert_snapshot;

#[test]
fn test_config_serialization() {
    let config = Config::default();
    let serialized = serde_yaml::to_string(&config).unwrap();

    // Snapshot testing for configuration
    assert_snapshot!(serialized);
}

#[test]
fn test_alert_format() {
    let alert = Alert {
        id: Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap(),
        rule_name: "test_rule".to_string(),
        severity: AlertSeverity::High,
        message: "Test alert message".to_string(),
        process: ProcessInfo::default(),
        timestamp: Utc::now(),
        metadata: HashMap::new(),
    };

    let formatted = alert.format_json().unwrap();
    assert_snapshot!(formatted);
}
```

## Test Maintenance

### Test Organization

```text
// tests/
// ├── unit/
// │   ├── collector_tests.rs
// │   ├── database_tests.rs
// │   └── alert_tests.rs
// ├── integration/
// │   ├── ipc_tests.rs
// │   ├── database_tests.rs
// │   └── alert_delivery_tests.rs
// ├── e2e/
// │   ├── cli_tests.rs
// │   └── system_tests.rs
// └── common/
//     ├── test_helpers.rs
//     └── test_data.rs
```

### Test Utilities

```rust
// tests/common/test_helpers.rs
pub struct TestHelper {
    temp_dir: TempDir,
    config: Config,
}

impl TestHelper {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::default();

        Self { temp_dir, config }
    }

    pub fn create_test_database(&self) -> Database {
        let db_path = self.temp_dir.path().join("test.db");
        Database::new(&db_path).unwrap()
    }

    pub fn create_test_config(&self) -> PathBuf {
        let config_path = self.temp_dir.path().join("config.yaml");
        self.config.save_to_file(&config_path).unwrap();
        config_path
    }

    pub fn cleanup(&self) {
        // Cleanup test resources
    }
}

impl Drop for TestHelper {
    fn drop(&mut self) {
        self.cleanup();
    }
}
```

---

*This testing documentation provides comprehensive guidance for testing DaemonEye. For additional testing information, consult the specific test files or contact the development team.*
