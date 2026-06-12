//! Integration tests for the resilient IPC client
//!
//! These tests validate the advanced client features including connection management,
//! automatic reconnection, circuit breaker functionality, and resilience patterns.
//!
//! # Test Coverage
//!
//! - Client creation and initialization
//! - Connection state tracking and health checks
//! - Circuit breaker functionality under failure conditions
//! - Automatic reconnection with exponential backoff
//! - Server error handling and error propagation
//! - Concurrent request handling
//! - Force reconnection scenarios
//! - Client statistics and metrics collection

#![allow(clippy::print_stdout)] // Test output is intentional
#![allow(
    clippy::expect_used,
    clippy::str_to_string,
    clippy::as_conversions,
    clippy::uninlined_format_args,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::let_underscore_must_use
)]

use daemoneye_lib::ipc::client::{
    CircuitBreakerState, CollectorEndpoint, LoadBalancingStrategy, ResilientIpcClient,
};
use daemoneye_lib::ipc::codec::IpcError;
use daemoneye_lib::ipc::interprocess_transport::InterprocessServer;
use daemoneye_lib::ipc::{IpcConfig, TransportType};
use daemoneye_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

/// Small, fast backoff settings so retry-driven tests do not wait on the
/// production defaults (10 attempts, 100ms base). Each helper that sends to a
/// missing server applies these to keep the suite quick.
const FAST_BASE_DELAY: Duration = Duration::from_millis(1);
const FAST_MAX_DELAY: Duration = Duration::from_millis(5);

/// Build a server whose handler echoes every task back as a success.
fn echo_server(config: &IpcConfig) -> InterprocessServer {
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });
    server
}

/// Creates a test IPC configuration with a temporary directory for socket files.
///
/// Returns a tuple of (`IpcConfig`, `TempDir`) where the `TempDir` must be kept alive
/// for the duration of the test to prevent cleanup of the socket path.
fn create_test_config() -> (IpcConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let endpoint_path = create_test_endpoint(&temp_dir);

    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path,
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 4,
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    (config, temp_dir)
}

fn create_test_endpoint(temp_dir: &TempDir) -> String {
    #[cfg(unix)]
    {
        temp_dir
            .path()
            .join("test-resilient.sock")
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        let dir_name = temp_dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("test");
        format!(r"\\.\pipe\daemoneye\test-resilient-{}", dir_name)
    }
}

fn create_test_task(task_id: &str) -> DetectionTask {
    DetectionTask {
        task_id: task_id.to_owned(),
        task_type: TaskType::EnumerateProcesses.into(),
        process_filter: None,
        hash_check: None,
        metadata: Some("resilient client test".to_owned()),
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    }
}

#[tokio::test]
async fn test_resilient_client_creation() {
    let (config, _temp_dir) = create_test_config();
    let _client = ResilientIpcClient::new(&config);
    // Client creation always succeeds
}

#[tokio::test]
async fn test_connection_state_tracking() {
    let (config, _temp_dir) = create_test_config();
    let _client = ResilientIpcClient::new(&config);

    // Test that client can be created successfully
    // Health check functionality not yet implemented
}

#[tokio::test]
async fn test_circuit_breaker_functionality() {
    let (config, _temp_dir) = create_test_config();
    // Keep retries small/fast: the no-server sends below now run a bounded
    // backoff loop, so the production defaults would make this test crawl.
    let client =
        ResilientIpcClient::new(&config).with_reconnect_config(2, FAST_BASE_DELAY, FAST_MAX_DELAY);

    // Get initial stats
    let stats = client.get_stats().await;
    let metrics = client.metrics();
    assert_eq!(
        metrics
            .tasks_sent_total
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert!(!stats.endpoint_stats.is_empty());
    assert_eq!(
        stats
            .endpoint_stats
            .first()
            .expect("Expected at least one endpoint")
            .circuit_breaker_state,
        CircuitBreakerState::Closed
    );

    // Try to send a task without a server (should fail)
    let task = create_test_task("circuit-breaker-test");
    let result = client.send_task(task).await;
    assert!(result.is_err(), "Expected task to fail without server");

    // Perform multiple task attempts to trigger the circuit breaker
    for i in 0..5 {
        println!("Attempt {}: sending task", i + 1);
        let task = create_test_task(&format!("circuit-breaker-test-{}", i));
        let result = client.send_task_to_endpoint(task, "default").await;
        assert!(result.is_err(), "Expected task to fail without server");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Give time for circuit breaker metrics to update
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Check metrics to verify failures were recorded
    let metrics = client.metrics();
    let tasks_failed = metrics
        .tasks_failed_total
        .load(std::sync::atomic::Ordering::Relaxed);
    let circuit_breaker_activations = metrics
        .circuit_breaker_activations_total
        .load(std::sync::atomic::Ordering::Relaxed);

    println!("Tasks failed: {}", tasks_failed);
    println!(
        "Circuit breaker activations: {}",
        circuit_breaker_activations
    );

    // Assert that failures were recorded
    assert!(tasks_failed > 0, "Expected failed tasks to be recorded");
    assert!(
        circuit_breaker_activations > 0,
        "Expected circuit breaker to be activated at least once"
    );
}

#[tokio::test]
async fn test_automatic_reconnection() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    // Set up handler to process enumeration tasks
    server.set_handler(|task: DetectionTask| async move {
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await; // Give server time to start

    // Create client and send task
    let client = ResilientIpcClient::new(&config);
    let task = create_test_task("reconnection-test");

    let result = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(task, "default"),
    )
    .await
    .expect("Client request timed out")
    .expect("Client request failed");

    assert!(result.success);
    assert_eq!(result.task_id, "reconnection-test");

    // Health check functionality not yet implemented
    // assert!(client.health_check().await);

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_retry_with_exponential_backoff() {
    let (mut config, _temp_dir) = create_test_config();
    // Use a unique endpoint path to avoid interference from other tests
    config.endpoint_path = format!("{}-backoff", config.endpoint_path);
    // Four attempts with a 20ms base so the backoff sleeps are observable but
    // the test still finishes quickly.
    let client = ResilientIpcClient::new(&config).with_reconnect_config(
        4,
        Duration::from_millis(20),
        Duration::from_millis(100),
    );

    // No server: ServerNotFound is retryable, so the bounded loop exhausts all
    // attempts before surfacing the last error.
    let task = create_test_task("backoff-test");
    let start_time = std::time::Instant::now();

    let result = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(task, "default"),
    )
    .await
    .expect("send should not hang");

    assert!(result.is_err(), "expected failure against a missing server");

    // Retries actually happened: the connection was attempted exactly N times
    // (once per bounded attempt) — proving the loop ran rather than failing
    // fast as the pre-backoff implementation did.
    let attempts = client
        .metrics()
        .connection_attempts_total
        .load(Ordering::Relaxed);
    assert_eq!(attempts, 4, "expected exactly 4 connection attempts");

    // Elapsed time covers at least the first three backoff sleeps (20 + 40 + 80
    // = 140ms, before jitter) — well above the pre-backoff "<500ms quick fail".
    let elapsed = start_time.elapsed();
    assert!(
        elapsed >= Duration::from_millis(140),
        "expected backoff sleeps to elapse, took {elapsed:?}"
    );
}

#[tokio::test]
async fn test_server_error_handling() {
    let (config, _temp_dir) = create_test_config();

    // Start server that always returns errors
    let mut server = InterprocessServer::new(config.clone());
    server.set_handler(|_task: DetectionTask| async move {
        Err(daemoneye_lib::ipc::codec::IpcError::Encode(
            "Test server error".to_owned(),
        ))
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Create client and send task
    let client = ResilientIpcClient::new(&config);
    let task = create_test_task("error-handling-test");

    let result = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(task, "default"),
    )
    .await
    .expect("Client request timed out")
    .expect("Client request failed");

    // Should receive error response
    assert!(!result.success);
    assert!(result.error_message.is_some());
    assert!(
        result
            .error_message
            .expect("Expected error message")
            .contains("Test server error")
    );

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_concurrent_requests() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Check if this is a capability negotiation request
        if task.metadata.as_deref() == Some("capability_negotiation") {
            // Return capability information in the result
            return Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![], // Empty for capability negotiation
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            });
        }

        // Normal task processing
        sleep(Duration::from_millis(50)).await;

        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Create a client and configure endpoint
    let client = Arc::new(ResilientIpcClient::new(&config));

    // Add endpoint to client
    let endpoint = CollectorEndpoint::new("default".to_string(), config.endpoint_path.clone(), 1);
    client.add_endpoint(endpoint).await;

    // Negotiate capabilities
    client
        .negotiate_capabilities("default")
        .await
        .expect("Capability negotiation failed");

    let dummy_task = create_test_task("health-check");
    let _ = client.send_task_to_endpoint(dummy_task, "default").await; // This will make the endpoint healthy

    // Send multiple concurrent requests
    let mut handles = vec![];

    for i in 0..3 {
        let client_clone = Arc::clone(&client);
        let handle = tokio::spawn(async move {
            let task = create_test_task(&format!("concurrent-test-{i}"));

            timeout(Duration::from_secs(3), client_clone.send_task(task))
                .await
                .expect("Client request timed out")
        });

        handles.push(handle);
    }

    // Wait for all requests to complete
    let mut successful_requests = 0;
    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(result)) => {
                assert_eq!(result.task_id, format!("concurrent-test-{i}"));
                assert!(result.success);
                successful_requests += 1;
            }
            Ok(Err(e)) => {
                eprintln!("Request {i} failed: {e}");
            }
            Err(e) => {
                eprintln!("Request {i} panicked: {e}");
            }
        }
    }

    // At least some requests should succeed
    assert!(successful_requests > 0, "No requests succeeded");

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_force_reconnection() {
    let (config, _temp_dir) = create_test_config();

    // Start server
    let mut server = InterprocessServer::new(config.clone());

    server.set_handler(|task: DetectionTask| async move {
        // Check if this is a capability negotiation request
        if task.metadata.as_deref() == Some("capability_negotiation") {
            // Return capability information in the result
            return Ok(DetectionResult {
                task_id: task.task_id,
                success: true,
                error_message: None,
                processes: vec![], // Empty for capability negotiation
                hash_result: None,
                network_events: vec![],
                filesystem_events: vec![],
                performance_events: vec![],
            });
        }

        // Normal task processing
        Ok(DetectionResult {
            task_id: task.task_id,
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
            network_events: vec![],
            filesystem_events: vec![],
            performance_events: vec![],
        })
    });

    server.start().await.expect("Failed to start server");
    sleep(Duration::from_millis(200)).await;

    // Create client and configure endpoint
    let client = ResilientIpcClient::new(&config);

    // Add endpoint to client
    let endpoint = CollectorEndpoint::new("default".to_string(), config.endpoint_path.clone(), 1);
    client.add_endpoint(endpoint).await;

    // Negotiate capabilities
    client
        .negotiate_capabilities("default")
        .await
        .expect("Capability negotiation failed");

    let dummy_task = create_test_task("health-check");
    let _ = client.send_task_to_endpoint(dummy_task, "default").await; // Make endpoint healthy

    let task = create_test_task("force-reconnect-test");

    // First request should succeed
    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result.success);

    // Send another request to test reconnection capability
    let task2 = create_test_task("force-reconnect-test-2");
    let result2 = timeout(Duration::from_secs(5), client.send_task(task2))
        .await
        .expect("Client request timed out")
        .expect("Client request failed");

    assert!(result2.success);

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_client_statistics() {
    let (config, _temp_dir) = create_test_config();
    let client =
        ResilientIpcClient::new(&config).with_reconnect_config(2, FAST_BASE_DELAY, FAST_MAX_DELAY);

    // Get initial stats
    let stats = client.get_stats().await;
    assert!(!stats.endpoint_stats.is_empty());
    let metrics = client.metrics();
    assert_eq!(
        metrics
            .tasks_sent_total
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        stats
            .endpoint_stats
            .first()
            .expect("Expected at least one endpoint")
            .circuit_breaker_state,
        CircuitBreakerState::Closed
    );

    // Try to send a task without server to generate failures
    let task = create_test_task("stats-test");
    let result = timeout(
        Duration::from_secs(2),
        client.send_task_to_endpoint(task, "default"),
    )
    .await;

    // Should timeout or fail
    let task_completed = result.is_ok_and(|task_result| task_result.is_ok());
    // We expect either a timeout or a task failure, not success
    assert!(
        !task_completed,
        "Expected task to timeout or fail, but it succeeded"
    );

    // Check updated stats - task should have been attempted or failed due to circuit breaker
    let _final_stats = client.get_stats().await;
    let final_metrics = client.metrics();

    // Either tasks_sent_total should be > 0 (if task was attempted)
    // OR tasks_failed_total should be > 0 (if circuit breaker was open)
    let tasks_sent = final_metrics
        .tasks_sent_total
        .load(std::sync::atomic::Ordering::Relaxed);
    let tasks_failed = final_metrics
        .tasks_failed_total
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        tasks_sent > 0 || tasks_failed > 0,
        "Expected at least one task to be sent or failed for statistics tracking. Sent: {}, Failed: {}",
        tasks_sent,
        tasks_failed
    );
}

#[tokio::test]
async fn test_backoff_retry_succeeds_when_server_starts_mid_retry() {
    // The "comes up on a later attempt" case: the client begins retrying
    // against an absent socket (ServerNotFound), the server binds mid-retry,
    // and a subsequent attempt connects and succeeds.
    let (config, _temp_dir) = create_test_config();
    let client = ResilientIpcClient::new(&config).with_reconnect_config(
        10,
        Duration::from_millis(50),
        Duration::from_secs(1),
    );

    let server_config = config.clone();
    let server_handle = tokio::spawn(async move {
        // Start the server shortly after the client begins retrying.
        sleep(Duration::from_millis(120)).await;
        let mut server = echo_server(&server_config);
        server.start().await.expect("server should start");
        // Keep it alive long enough for the client to connect and finish.
        sleep(Duration::from_secs(2)).await;
        let _result = server.graceful_shutdown().await;
    });

    let task = create_test_task("warmup-test");
    let result = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(task, "default"),
    )
    .await
    .expect("send should not hang")
    .expect("send should succeed once the server comes up");

    assert!(result.success);
    assert_eq!(result.task_id, "warmup-test");

    let _result = server_handle.await;
}

#[tokio::test]
async fn test_backoff_bound_stops_after_max_attempts() {
    // Regression guard: with max_reconnect_attempts above the circuit-breaker
    // failure threshold (5), the breaker must NOT trip mid-retry and cut the
    // loop short. Exactly N connection attempts must occur.
    let (mut config, _temp_dir) = create_test_config();
    config.endpoint_path = format!("{}-bound", config.endpoint_path);
    let client =
        ResilientIpcClient::new(&config).with_reconnect_config(8, FAST_BASE_DELAY, FAST_MAX_DELAY);

    let task = create_test_task("bound-test");
    let result = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(task, "default"),
    )
    .await
    .expect("send should not hang");

    assert!(result.is_err(), "expected failure against a missing server");

    let attempts = client
        .metrics()
        .connection_attempts_total
        .load(Ordering::Relaxed);
    assert_eq!(
        attempts, 8,
        "expected exactly 8 attempts (breaker must not short-circuit the loop), got {attempts}"
    );
}

#[tokio::test]
async fn test_circuit_breaker_open_short_circuits_without_retry() {
    // Once the breaker is open, a further send is rejected immediately with
    // CircuitBreakerOpen and makes no new connection attempt — the open breaker
    // is classified fatal and not retried.
    let (mut config, _temp_dir) = create_test_config();
    config.endpoint_path = format!("{}-cb-open", config.endpoint_path);
    // One attempt per send and the default breaker threshold (5): five failing
    // sends trip the breaker.
    let client =
        ResilientIpcClient::new(&config).with_reconnect_config(1, FAST_BASE_DELAY, FAST_MAX_DELAY);

    for i in 0..5 {
        let task = create_test_task(&format!("cb-trip-{i}"));
        let result = client.send_task_to_endpoint(task, "default").await;
        assert!(result.is_err(), "send {i} should fail without a server");
    }

    let attempts_before = client
        .metrics()
        .connection_attempts_total
        .load(Ordering::Relaxed);

    let task = create_test_task("cb-after-open");
    let result = client.send_task_to_endpoint(task, "default").await;
    assert!(
        matches!(result, Err(IpcError::CircuitBreakerOpen)),
        "expected CircuitBreakerOpen, got {result:?}"
    );

    let attempts_after = client
        .metrics()
        .connection_attempts_total
        .load(Ordering::Relaxed);
    assert_eq!(
        attempts_before, attempts_after,
        "an open breaker must not attempt a new connection"
    );
}

#[tokio::test]
async fn test_circuit_breaker_trips_blocks_and_recovers() {
    // Covers AE2: the breaker trips after threshold failures (reported as Open
    // in stats), blocks sends while open, then allows attempts again once the
    // cooldown elapses and closes after a successful recovery. A short recovery
    // timeout keeps the test fast.
    //
    // NOTE: this asserts the observable Open -> blocked -> Closed lifecycle. It
    // does NOT assert the intermediate HalfOpen state or the multi-success
    // half-open accumulation: a pre-existing quirk means the breaker's
    // half-open transition is computed on a throwaway clone in
    // `should_attempt_connection`, so the stored breaker recovers on the first
    // success and HalfOpen is never surfaced. Tracked as a follow-up; see the
    // PR's residual review findings.
    let (config, _temp_dir) = create_test_config();
    let client = ResilientIpcClient::new(&config)
        .with_reconnect_config(1, FAST_BASE_DELAY, FAST_MAX_DELAY)
        .with_breaker_config(3, Duration::from_millis(200));

    // Phase 1: three failing sends (no server) trip the breaker.
    for i in 0..3 {
        let task = create_test_task(&format!("trip-{i}"));
        let _result = client.send_task_to_endpoint(task, "default").await;
    }
    let stats = client.get_stats().await;
    let state = stats
        .endpoint_stats
        .iter()
        .find(|e| e.endpoint_id == "default")
        .expect("default endpoint present")
        .circuit_breaker_state
        .clone();
    assert_eq!(
        state,
        CircuitBreakerState::Open,
        "breaker should report Open in stats after threshold failures"
    );

    // Phase 2: while open and within the cooldown, sends short-circuit.
    let task = create_test_task("blocked");
    let result = client.send_task_to_endpoint(task, "default").await;
    assert!(
        matches!(result, Err(IpcError::CircuitBreakerOpen)),
        "send while open should be blocked, got {result:?}"
    );

    // Phase 3: bring a server up and wait out the cooldown; recovery succeeds
    // and the breaker closes.
    let mut server = echo_server(&config);
    server.start().await.expect("server should start");
    sleep(Duration::from_millis(250)).await; // exceed the 200ms cooldown

    for i in 0..3 {
        let task = create_test_task(&format!("recover-{i}"));
        let result = timeout(
            Duration::from_secs(5),
            client.send_task_to_endpoint(task, "default"),
        )
        .await
        .expect("recovery send should not hang");
        assert!(
            result.is_ok(),
            "recovery send {i} should succeed: {result:?}"
        );
    }

    let stats = client.get_stats().await;
    let state = stats
        .endpoint_stats
        .iter()
        .find(|e| e.endpoint_id == "default")
        .expect("default endpoint present")
        .circuit_breaker_state
        .clone();
    assert_eq!(
        state,
        CircuitBreakerState::Closed,
        "breaker should close in stats after successful recovery"
    );

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_connection_pool_reuse() {
    // Two sequential tasks to the same endpoint: the first establishes and
    // pools a connection, the second reuses it without establishing a new one.
    let (config, _temp_dir) = create_test_config();
    let mut server = echo_server(&config);
    server.start().await.expect("server should start");
    sleep(Duration::from_millis(200)).await;

    let client = ResilientIpcClient::new(&config);

    let first = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(create_test_task("pool-1"), "default"),
    )
    .await
    .expect("no hang")
    .expect("first send should succeed");
    assert!(first.success);

    let metrics = client.metrics();
    assert_eq!(
        metrics
            .connections_established_total
            .load(Ordering::Relaxed),
        1,
        "first task should establish exactly one connection"
    );
    assert_eq!(
        metrics.pooled_connections_current.load(Ordering::Relaxed),
        1,
        "the connection should be returned to the pool after the send"
    );

    let second = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(create_test_task("pool-2"), "default"),
    )
    .await
    .expect("no hang")
    .expect("second send should succeed");
    assert!(second.success);

    assert_eq!(
        metrics
            .connections_established_total
            .load(Ordering::Relaxed),
        1,
        "second task should reuse the pooled connection, not establish a new one"
    );
    assert_eq!(
        metrics.pooled_connections_current.load(Ordering::Relaxed),
        1,
        "the reused connection should be returned to the pool after the second send"
    );

    let _result = server.graceful_shutdown().await;
}

// Unix-only: this test simulates a stale pooled connection by tearing down a
// server and rebinding a new one on the same endpoint. That simulation relies
// on Unix-domain-socket teardown/rebind timing; on Windows named pipes the
// stale-handle and rebind semantics differ and make the simulation unreliable
// (GOTCHAS §1.1 — Windows IPC behavior only validates in CI). The production
// fallback it exercises (`send_task_to_endpoint` re-dialing on a failed pooled
// send) is platform-agnostic, and cross-platform pooling is covered by
// `test_connection_pool_reuse`.
#[cfg(unix)]
#[tokio::test]
async fn test_pooled_connection_recovers_after_server_restart() {
    // A connection pooled against one server becomes stale when that server
    // restarts. The next send reuses the stale connection, detects the failure,
    // and recovers by establishing a fresh connection — server restarts must
    // not be made worse by connection pooling.
    let (config, _temp_dir) = create_test_config();
    let mut server = echo_server(&config);
    server.start().await.expect("server should start");
    sleep(Duration::from_millis(200)).await;

    let client =
        ResilientIpcClient::new(&config).with_reconnect_config(3, FAST_BASE_DELAY, FAST_MAX_DELAY);

    // First send establishes and pools a connection.
    let first = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(create_test_task("restart-1"), "default"),
    )
    .await
    .expect("no hang")
    .expect("first send should succeed");
    assert!(first.success);
    assert_eq!(
        client
            .metrics()
            .pooled_connections_current
            .load(Ordering::Relaxed),
        1,
        "first send should pool its connection"
    );

    // Restart the server: the pooled connection is now stale.
    let _result = server.graceful_shutdown().await;
    sleep(Duration::from_millis(150)).await;
    let mut server = echo_server(&config);
    server.start().await.expect("server should restart");
    sleep(Duration::from_millis(200)).await;

    // The next send reuses the stale pooled connection, the send over it fails,
    // and the client recovers transparently with a fresh connection.
    let second = timeout(
        Duration::from_secs(5),
        client.send_task_to_endpoint(create_test_task("restart-2"), "default"),
    )
    .await
    .expect("no hang")
    .expect("send should recover via a fresh connection after the restart");
    assert!(second.success);
    assert_eq!(second.task_id, "restart-2");

    let _result = server.graceful_shutdown().await;
}

#[tokio::test]
async fn test_failover_routes_around_downed_endpoint() {
    // Covers AE2: routing selects a primary endpoint that is down, the send
    // fails, and the client fails over to a healthy alternate that succeeds.
    let (down_config, _down_dir) = create_test_config();
    let (up_config, _up_dir) = create_test_config();

    // Only the "up" endpoint has a server listening.
    let mut server = echo_server(&up_config);
    server.start().await.expect("server should start");
    sleep(Duration::from_millis(200)).await;

    // Priority strategy selects the lowest-priority-number healthy endpoint
    // first; "down" (priority 1) is tried before "up" (priority 2).
    let down = CollectorEndpoint::new("down".to_string(), down_config.endpoint_path.clone(), 1);
    let up = CollectorEndpoint::new("up".to_string(), up_config.endpoint_path.clone(), 2);

    let client = ResilientIpcClient::new_with_endpoints(
        &down_config,
        vec![down, up],
        LoadBalancingStrategy::Priority,
    )
    .with_reconnect_config(2, FAST_BASE_DELAY, FAST_MAX_DELAY);

    let task = create_test_task("failover-test");
    let result = timeout(Duration::from_secs(5), client.send_task(task))
        .await
        .expect("send should not hang")
        .expect("failover should succeed on the healthy endpoint");

    assert!(result.success);
    assert_eq!(result.task_id, "failover-test");

    let _result = server.graceful_shutdown().await;
}
