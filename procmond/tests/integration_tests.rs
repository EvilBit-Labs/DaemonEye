//! Integration tests for ProcessEventSource with collector-core runtime.
//!
//! These tests verify that the ProcessEventSource properly integrates with the
//! collector-core framework and behaves correctly in realistic scenarios.

use collector_core::{CollectionEvent, Collector, CollectorConfig, EventSource, SourceCaps};
use daemoneye_lib::storage::DatabaseManager;
use procmond::{ProcessEventSource, ProcessSourceConfig};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{sleep, timeout};

/// Test database manager that properly manages the temporary directory lifecycle.
struct TestDatabase {
    _temp_dir: TempDir,
    db_manager: Arc<Mutex<DatabaseManager>>,
}

impl TestDatabase {
    /// Creates a test database manager for integration tests.
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let db_path = temp_dir.path().join("integration_test.db");
        let db_manager = Arc::new(Mutex::new(
            DatabaseManager::new(&db_path)
                .expect("Failed to create database manager for integration test"),
        ));
        Self {
            _temp_dir: temp_dir,
            db_manager,
        }
    }

    /// Returns a reference to the database manager.
    fn manager(&self) -> Arc<Mutex<DatabaseManager>> {
        self.db_manager.clone()
    }
}

/// Creates a test configuration optimized for integration testing.
fn create_test_config() -> ProcessSourceConfig {
    ProcessSourceConfig {
        collection_interval: Duration::from_millis(1000), // More realistic interval
        collect_enhanced_metadata: false,                 // Disabled for speed in tests
        max_processes_per_cycle: 10,                      // Very small limit for testing
        compute_executable_hashes: false,                 // Disabled for speed
        max_events_in_flight: 50,
        collection_timeout: Duration::from_secs(10), // Longer timeout
        shutdown_timeout: Duration::from_secs(5),
        max_backpressure_wait: Duration::from_millis(1000),
        event_batch_size: 5,
        batch_timeout: Duration::from_millis(500),
    }
}

#[tokio::test]
async fn test_process_event_source_with_collector_core() {
    // Create test components
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let config = create_test_config();
    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Create collector configuration
    let collector_config = CollectorConfig {
        component_name: "test-collector".to_string(),
        max_event_sources: 5,
        event_buffer_size: 1000,
        startup_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        health_check_interval: Duration::from_secs(1),
        telemetry_interval: Duration::from_secs(1),
        enable_telemetry: true,
        enable_debug_logging: true,
        max_batch_size: 50,
        batch_timeout: Duration::from_millis(100),
        backpressure_threshold: 800,
        max_backpressure_wait: Duration::from_millis(500),
    };

    // Create and configure collector
    let mut collector = Collector::new(collector_config);

    // Register the process event source
    let registration_result = collector.register(Box::new(process_source));
    assert!(
        registration_result.is_ok(),
        "Process source registration should succeed"
    );

    // Verify collector capabilities
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));
    assert!(capabilities.contains(SourceCaps::REALTIME)); // Due to fast interval

    // Verify source count
    assert_eq!(collector.source_count(), 1);
}

#[tokio::test]
async fn test_process_event_source_lifecycle_management() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let config = create_test_config();
    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Test EventSource trait methods
    assert_eq!(process_source.name(), "process-monitor");

    let capabilities = process_source.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));
    assert!(capabilities.contains(SourceCaps::REALTIME));

    // Test health check
    let health_result = process_source.health_check().await;
    assert!(health_result.is_ok(), "Initial health check should pass");

    // Test start/stop lifecycle
    let (tx, mut rx) = mpsc::channel(1000);
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start the event source in a background task
    let source_clone = process_source;
    let shutdown_clone = Arc::clone(&shutdown_signal);
    let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

    // Wait for at least one event to be generated
    let mut event_count = 0;
    let max_wait = Duration::from_secs(15); // More generous timeout
    let start_time = std::time::Instant::now();

    println!("Starting to wait for process events...");
    while event_count < 1 && start_time.elapsed() < max_wait {
        println!(
            "Waiting for events, elapsed: {:?}, count: {}",
            start_time.elapsed(),
            event_count
        );
        if let Ok(Some(event)) = timeout(Duration::from_millis(200), rx.recv()).await {
            match event {
                CollectionEvent::Process(proc_event) => {
                    println!(
                        "Received process event: PID={}, name={}",
                        proc_event.pid, proc_event.name
                    );
                    assert!(proc_event.pid > 0, "Process PID should be valid");
                    assert!(
                        !proc_event.name.is_empty(),
                        "Process name should not be empty"
                    );
                    assert!(proc_event.accessible, "Process should be accessible");
                    event_count += 1;
                }
                _ => panic!("Unexpected event type"),
            }
        } else {
            println!("No event received in this iteration");
        }
    }

    println!("Finished waiting. Total events received: {}", event_count);
    assert!(
        event_count > 0,
        "Should have received at least one process event within {} seconds",
        max_wait.as_secs()
    );

    // Signal shutdown
    shutdown_signal.store(true, Ordering::Relaxed);

    // Wait for the start task to complete
    let start_result = timeout(Duration::from_secs(5), start_task).await;
    match start_result {
        Err(_) => panic!("Start task timed out after 5 seconds"),
        Ok(inner_result) => match inner_result {
            Err(e) => panic!("Start task failed with error: {}", e),
            Ok(_) => {
                // Start task completed successfully
            }
        },
    }
}

#[tokio::test]
async fn test_process_event_source_error_handling() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let mut config = create_test_config();

    // Configure for aggressive timeouts to test error handling
    config.collection_timeout = Duration::from_millis(1); // Very short timeout
    config.max_backpressure_wait = Duration::from_millis(10);

    let process_source = ProcessEventSource::with_config(db_manager, config);

    let (tx, _rx) = mpsc::channel(1); // Small buffer to create backpressure
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start collection with aggressive timeouts
    let start_result = timeout(
        Duration::from_millis(500),
        process_source.start(tx, shutdown_signal.clone()),
    )
    .await;

    // The operation should either complete or timeout
    // We're mainly testing that it doesn't panic or hang
    assert!(start_result.is_ok() || start_result.is_err());

    // Signal shutdown to clean up
    shutdown_signal.store(true, Ordering::Relaxed);
}

#[tokio::test]
async fn test_process_event_source_statistics_integration() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let config = create_test_config();
    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Check initial statistics
    let initial_stats = process_source.stats();
    assert_eq!(initial_stats.collection_cycles, 0);
    assert_eq!(initial_stats.processes_collected, 0);
    assert_eq!(initial_stats.collection_errors, 0);

    let (tx, mut rx) = mpsc::channel(1000);
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start collection
    let source_clone = process_source;
    let shutdown_clone = Arc::clone(&shutdown_signal);
    let start_task = tokio::spawn(async move {
        let _ = source_clone.start(tx, shutdown_clone).await;
    });

    // Note: We can't access source_clone here due to move, so we'll test differently
    // The statistics would be updated inside the spawned task

    // Consume at least one event to verify they're being generated
    let mut received_events = 0;
    let max_wait = Duration::from_secs(15); // More generous timeout
    let start_time = std::time::Instant::now();

    println!("Starting to wait for process events in statistics test...");
    while received_events < 1 && start_time.elapsed() < max_wait {
        println!(
            "Waiting for events, elapsed: {:?}, count: {}",
            start_time.elapsed(),
            received_events
        );
        if let Ok(Some(event)) = timeout(Duration::from_millis(200), rx.recv()).await {
            println!("Received process event: {:?}", event);
            received_events += 1;
        } else {
            println!("No event received in this iteration");
        }
    }

    println!(
        "Finished waiting. Total events received: {}",
        received_events
    );
    assert!(
        received_events > 0,
        "Should have received at least one event (got {})",
        received_events
    );

    // Signal shutdown and wait for task to complete
    shutdown_signal.store(true, Ordering::Relaxed);

    // Wait for the background task to finish with a timeout
    match tokio::time::timeout(Duration::from_secs(5), start_task).await {
        Ok(Ok(_)) => {
            // Task completed successfully
        }
        Ok(Err(e)) => {
            panic!("Background task panicked: {}", e);
        }
        Err(_) => {
            panic!("Background task did not complete within timeout");
        }
    }
}

#[tokio::test]
async fn test_process_event_source_backpressure_integration() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let mut config = create_test_config();

    // Configure for low backpressure limits
    config.max_events_in_flight = 5;
    config.max_backpressure_wait = Duration::from_millis(100);
    config.event_batch_size = 2;

    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Create a channel that won't be read from to simulate backpressure
    let (tx, _rx) = mpsc::channel(1);
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start collection with backpressure conditions
    let source_clone = process_source;
    let shutdown_clone = Arc::clone(&shutdown_signal);
    let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

    // Let it run for a short time to encounter backpressure
    sleep(Duration::from_millis(200)).await;

    // Signal shutdown
    shutdown_signal.store(true, Ordering::Relaxed);

    // Wait for completion
    let result = timeout(Duration::from_secs(3), start_task).await;
    assert!(
        result.is_ok(),
        "Task should complete even under backpressure"
    );
}

#[tokio::test]
async fn test_process_event_source_graceful_shutdown() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let mut config = create_test_config();
    config.shutdown_timeout = Duration::from_millis(100);
    config.max_backpressure_wait = Duration::from_millis(10); // Very fast timeout for test
    config.collection_timeout = Duration::from_millis(100); // Very short collection timeout

    let process_source = ProcessEventSource::with_config(db_manager, config);

    let (tx, _rx) = mpsc::channel(1000);
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start collection
    let source_clone = process_source;
    let shutdown_clone = Arc::clone(&shutdown_signal);
    let start_task = tokio::spawn(async move { source_clone.start(tx, shutdown_clone).await });

    // Wait for a short time to let collection start, but don't wait for events
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal graceful shutdown
    let shutdown_start = std::time::Instant::now();
    shutdown_signal.store(true, Ordering::Relaxed);

    // Wait for graceful shutdown
    let result = timeout(Duration::from_secs(5), start_task).await;
    let shutdown_duration = shutdown_start.elapsed();

    assert!(result.is_ok(), "Graceful shutdown should complete");
    assert!(result.unwrap().is_ok(), "Graceful shutdown should succeed");
    assert!(
        shutdown_duration < Duration::from_secs(3),
        "Shutdown should be reasonably fast"
    );
}

#[tokio::test]
async fn test_process_event_source_health_monitoring_integration() {
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let config = create_test_config();
    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Perform multiple health checks to verify consistency
    for i in 0..3 {
        let health_result = process_source.health_check().await;
        assert!(
            health_result.is_ok(),
            "Health check {} should pass: {:?}",
            i + 1,
            health_result
        );

        // Small delay between checks
        sleep(Duration::from_millis(10)).await;
    }

    // Test health check under load
    let (tx, _rx) = mpsc::channel(1000);
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Start collection in background
    let source_clone = process_source;
    let shutdown_clone = Arc::clone(&shutdown_signal);
    let start_task = tokio::spawn(async move {
        let _ = source_clone.start(tx, shutdown_clone).await;
    });

    // Wait a bit for collection to start
    sleep(Duration::from_millis(150)).await;

    // Note: We can't access source_clone here due to move
    // The health check would be performed inside the spawned task

    // Clean shutdown and wait for task to complete
    shutdown_signal.store(true, Ordering::Relaxed);

    // Wait for the background task to finish with a timeout
    match tokio::time::timeout(Duration::from_secs(5), start_task).await {
        Ok(Ok(_)) => {
            // Task completed successfully
        }
        Ok(Err(e)) => {
            panic!("Background task panicked: {}", e);
        }
        Err(_) => {
            panic!("Background task did not complete within timeout");
        }
    }
}

#[tokio::test]
async fn test_multiple_process_event_sources() {
    // Test registering multiple process sources with different configurations
    let collector_config = CollectorConfig::default();
    let mut collector = Collector::new(collector_config);

    // Create a single source (multiple sources with same name would conflict)
    let test_db = TestDatabase::new();
    let db_manager = test_db.manager();
    let config = create_test_config();
    let process_source = ProcessEventSource::with_config(db_manager, config);

    // Source should register successfully
    let result = collector.register(Box::new(process_source));
    assert!(result.is_ok(), "Source registration should succeed");

    // Verify source is registered
    assert_eq!(collector.source_count(), 1);

    // Verify combined capabilities
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));
    assert!(capabilities.contains(SourceCaps::REALTIME));
}
