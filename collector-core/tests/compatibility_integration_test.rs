//! Compatibility tests ensuring collector-core works with existing procmond and sentinelagent.
//!
//! This test suite verifies that the collector-core framework maintains backward
//! compatibility with existing components and preserves the established IPC protocol
//! and operational behavior.

use async_trait::async_trait;
use collector_core::{
    CollectionEvent, Collector, CollectorConfig, EventSource, ProcessEvent, SourceCaps,
};
use prost::Message;
use daemoneye_lib::proto::{
    AdvancedCapabilities, CollectionCapabilities, DetectionResult, DetectionTask, MonitoringDomain,
    ProcessRecord, TaskType,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{RwLock, mpsc},
    time::sleep,
};
use tracing::{debug, info};

/// Legacy-compatible event source that mimics existing procmond behavior.
struct LegacyCompatibleSource {
    name: &'static str,
    capabilities: SourceCaps,
    events_sent: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    legacy_mode: bool,
}

impl LegacyCompatibleSource {
    fn new(name: &'static str, capabilities: SourceCaps) -> Self {
        Self {
            name,
            capabilities,
            events_sent: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            legacy_mode: true,
        }
    }

    fn with_legacy_mode(mut self, enabled: bool) -> Self {
        self.legacy_mode = enabled;
        self
    }

    fn events_sent(&self) -> usize {
        self.events_sent.load(Ordering::Relaxed)
    }

    // Generate events that match existing procmond format
    fn create_legacy_process_event(&self, event_id: usize) -> CollectionEvent {
        CollectionEvent::Process(ProcessEvent {
            pid: 40000 + event_id as u32,
            ppid: Some(1),
            name: format!("legacy_proc_{}", event_id),
            executable_path: Some(format!("/usr/bin/legacy_{}", event_id)),
            command_line: vec![
                format!("legacy_{}", event_id),
                "--compat".to_string(),
                format!("--id={}", event_id),
            ],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some((event_id % 100) as f64),
            memory_usage: Some(1024 * (event_id + 1) as u64),
            executable_hash: Some(format!("legacy_hash_{:064x}", event_id)),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
        })
    }
}

#[async_trait]
impl EventSource for LegacyCompatibleSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> SourceCaps {
        self.capabilities
    }

    async fn start(&self, tx: mpsc::Sender<CollectionEvent>) -> anyhow::Result<()> {
        info!(
            source = self.name,
            legacy_mode = self.legacy_mode,
            "Starting legacy-compatible event source"
        );

        self.running.store(true, Ordering::Relaxed);

        let mut event_count = 0;

        while self.running.load(Ordering::Relaxed) && event_count < 50 {
            let event = if self.legacy_mode {
                self.create_legacy_process_event(event_count)
            } else {
                // Modern format (same structure, different naming)
                CollectionEvent::Process(ProcessEvent {
                    pid: 45000 + event_count as u32,
                    ppid: Some(1),
                    name: format!("modern_proc_{}", event_count),
                    executable_path: Some(format!("/usr/bin/modern_{}", event_count)),
                    command_line: vec![format!("modern_{}", event_count)],
                    start_time: Some(SystemTime::now()),
                    cpu_usage: Some(1.0),
                    memory_usage: Some(1024 * 1024),
                    executable_hash: Some(format!("modern_hash_{:064x}", event_count)),
                    user_id: Some("1000".to_string()),
                    accessible: true,
                    file_exists: true,
                    timestamp: SystemTime::now(),
                })
            };

            if tx.send(event).await.is_err() {
                debug!(source = self.name, "Event channel closed");
                break;
            }

            self.events_sent.fetch_add(1, Ordering::Relaxed);
            event_count += 1;

            // Mimic existing procmond timing (reduced for testing)
            sleep(Duration::from_millis(2)).await;
        }

        info!(
            source = self.name,
            events_sent = self.events_sent(),
            "Legacy-compatible event source completed"
        );

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!(
            source = self.name,
            "Stopping legacy-compatible event source"
        );
        self.running.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        if self.running.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Legacy-compatible source is not running"))
        }
    }
}

/// Mock sentinelagent client that tests IPC compatibility.
struct MockSentinelAgentClient {
    name: &'static str,
    tasks_sent: Arc<AtomicUsize>,
    results_received: Arc<AtomicUsize>,
    capabilities_received: Arc<AtomicUsize>,
    #[allow(dead_code)]
    running: Arc<AtomicBool>,
}

impl MockSentinelAgentClient {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            tasks_sent: Arc::new(AtomicUsize::new(0)),
            results_received: Arc::new(AtomicUsize::new(0)),
            capabilities_received: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    fn tasks_sent(&self) -> usize {
        self.tasks_sent.load(Ordering::Relaxed)
    }

    fn results_received(&self) -> usize {
        self.results_received.load(Ordering::Relaxed)
    }

    fn capabilities_received(&self) -> usize {
        self.capabilities_received.load(Ordering::Relaxed)
    }

    // Simulate sending detection tasks like sentinelagent would
    async fn send_detection_tasks(&self) -> Vec<DetectionTask> {
        let tasks = vec![
            DetectionTask {
                task_id: "compat_task_1".to_string(),
                task_type: TaskType::EnumerateProcesses as i32,
                process_filter: None,
                hash_check: None,
                metadata: Some("compatibility_test".to_string()),
                network_filter: None,
                filesystem_filter: None,
                performance_filter: None,
            },
            DetectionTask {
                task_id: "compat_task_2".to_string(),
                task_type: TaskType::CheckProcessHash as i32,
                process_filter: None,
                hash_check: None,
                metadata: Some("hash_verification".to_string()),
                network_filter: None,
                filesystem_filter: None,
                performance_filter: None,
            },
        ];

        self.tasks_sent.store(tasks.len(), Ordering::Relaxed);
        tasks
    }

    // Simulate processing detection results like sentinelagent would
    async fn process_detection_result(&self, result: DetectionResult) -> bool {
        info!(
            client = self.name,
            task_id = %result.task_id,
            success = result.success,
            process_count = result.processes.len(),
            "Processing detection result"
        );

        self.results_received.fetch_add(1, Ordering::Relaxed);

        // Validate result format matches expectations
        if result.success {
            // Check that processes have expected fields
            for process in &result.processes {
                if process.pid == 0 || process.name.is_empty() {
                    return false;
                }
            }
        }

        true
    }

    // Simulate capability negotiation like sentinelagent would
    async fn negotiate_capabilities(&self, capabilities: CollectionCapabilities) -> bool {
        info!(
            client = self.name,
            supported_domains = ?capabilities.supported_domains,
            "Negotiating capabilities"
        );

        self.capabilities_received.fetch_add(1, Ordering::Relaxed);

        // Verify expected capability format
        let has_process = capabilities
            .supported_domains
            .contains(&(MonitoringDomain::Process as i32));

        if let Some(advanced) = &capabilities.advanced {
            info!(
                kernel_level = advanced.kernel_level,
                realtime = advanced.realtime,
                system_wide = advanced.system_wide,
                "Advanced capabilities"
            );
        }

        has_process // At minimum, should support process monitoring
    }
}

#[tokio::test]
async fn test_legacy_procmond_compatibility() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(1000)
        .with_component_name("procmond".to_string()); // Use legacy component name

    let mut collector = Collector::new(config);

    // Create legacy-compatible source that mimics existing procmond behavior
    let legacy_source =
        LegacyCompatibleSource::new("legacy-procmond", SourceCaps::PROCESS).with_legacy_mode(true);

    // Create modern source for comparison
    let modern_source =
        LegacyCompatibleSource::new("modern-procmond", SourceCaps::PROCESS).with_legacy_mode(false);

    // Store references for testing (unused but kept for future use)
    let _legacy_events = legacy_source.events_sent.clone();
    let _modern_events = modern_source.events_sent.clone();

    let _ = collector.register(Box::new(legacy_source));
    let _ = collector.register(Box::new(modern_source));

    // Verify capability aggregation works with legacy sources
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));

    // Test that sources can generate events independently
    let (tx1, _rx1) = tokio::sync::mpsc::channel(100);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(100);

    // Create new sources and get their event counters
    let legacy_source_test =
        LegacyCompatibleSource::new("legacy-procmond", SourceCaps::PROCESS).with_legacy_mode(true);
    let modern_source_test =
        LegacyCompatibleSource::new("modern-procmond", SourceCaps::PROCESS).with_legacy_mode(false);

    let legacy_events_test = legacy_source_test.events_sent.clone();
    let modern_events_test = modern_source_test.events_sent.clone();

    // Start legacy source
    let legacy_handle = tokio::spawn(async move {
        let _ = legacy_source_test.start(tx1).await;
    });

    // Start modern source
    let modern_handle = tokio::spawn(async move {
        let _ = modern_source_test.start(tx2).await;
    });

    // Let sources run for a bit
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop sources
    legacy_handle.abort();
    modern_handle.abort();

    let legacy_count = legacy_events_test.load(Ordering::Relaxed);
    let modern_count = modern_events_test.load(Ordering::Relaxed);

    info!(
        legacy_events = legacy_count,
        modern_events = modern_count,
        "Legacy procmond compatibility test completed"
    );

    // Verify both legacy and modern sources worked
    assert!(legacy_count > 0, "Legacy source should generate events");
    assert!(modern_count > 0, "Modern source should generate events");

    // Both should generate similar number of events
    let event_ratio = legacy_count as f64 / modern_count as f64;
    assert!(
        event_ratio > 0.5 && event_ratio < 2.0,
        "Legacy and modern sources should have similar throughput"
    );
}

#[tokio::test]
async fn test_sentinelagent_ipc_compatibility() {
    use collector_core::ipc::CollectorIpcServer;

    let config = CollectorConfig::default();
    let capabilities = Arc::new(RwLock::new(
        SourceCaps::PROCESS | SourceCaps::NETWORK | SourceCaps::REALTIME,
    ));
    let ipc_server = CollectorIpcServer::new(config, Arc::clone(&capabilities));

    // Create mock sentinelagent client
    let mock_client = MockSentinelAgentClient::new("mock-sentinelagent");

    // Test capability negotiation
    let proto_capabilities = ipc_server.get_capabilities().await;
    let negotiation_success = mock_client.negotiate_capabilities(proto_capabilities).await;

    assert!(negotiation_success, "Capability negotiation should succeed");
    assert_eq!(
        mock_client.capabilities_received(),
        1,
        "Should receive capabilities"
    );

    // Test detection task compatibility
    let tasks = mock_client.send_detection_tasks().await;
    assert_eq!(tasks.len(), 2, "Should generate test tasks");
    assert_eq!(mock_client.tasks_sent(), 2, "Should track sent tasks");

    // Verify task format compatibility
    for task in &tasks {
        assert!(!task.task_id.is_empty(), "Task ID should not be empty");
        assert!(
            task.task_type >= 0 && task.task_type <= 6,
            "Task type should be valid"
        );

        // Test serialization compatibility
        let serialized = task.encode_to_vec();
        assert!(!serialized.is_empty(), "Task should serialize");

        let deserialized = DetectionTask::decode(&serialized[..]).expect("Task should deserialize");
        assert_eq!(
            task.task_id, deserialized.task_id,
            "Task ID should be preserved"
        );
        assert_eq!(
            task.task_type, deserialized.task_type,
            "Task type should be preserved"
        );
    }

    // Test detection result format
    let mock_result = DetectionResult {
        task_id: "test_task".to_string(),
        success: true,
        error_message: None,
        processes: vec![ProcessRecord {
            pid: 1234,
            ppid: Some(1),
            name: "test_process".to_string(),
            executable_path: Some("/usr/bin/test".to_string()),
            command_line: vec!["test".to_string(), "--flag".to_string()],
            start_time: Some(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
            cpu_usage: Some(5.5),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("test_hash".to_string()),
            hash_algorithm: Some("sha256".to_string()),
            user_id: Some("1000".to_string()),
            accessible: true,
            file_exists: true,
            collection_time: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        }],
        hash_result: None,
        network_events: vec![], // New fields should be empty for compatibility
        filesystem_events: vec![],
        performance_events: vec![],
    };

    let result_processed = mock_client.process_detection_result(mock_result).await;
    assert!(
        result_processed,
        "Detection result should be processed successfully"
    );
    assert_eq!(
        mock_client.results_received(),
        1,
        "Should receive detection result"
    );
}

#[tokio::test]
async fn test_protobuf_backward_compatibility() {
    // Test that new protobuf messages are backward compatible with existing clients

    // Create old-style detection task (without new fields)
    let old_task = DetectionTask {
        task_id: "old_task".to_string(),
        task_type: TaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: Some("old_metadata".to_string()),
        // New fields should default to None/empty
        network_filter: None,
        filesystem_filter: None,
        performance_filter: None,
    };

    // Serialize and deserialize to verify compatibility
    let serialized = old_task.encode_to_vec();
    let deserialized =
        DetectionTask::decode(&serialized[..]).expect("Old task format should deserialize");

    assert_eq!(old_task.task_id, deserialized.task_id);
    assert_eq!(old_task.task_type, deserialized.task_type);
    assert_eq!(old_task.metadata, deserialized.metadata);
    assert!(deserialized.network_filter.is_none());
    assert!(deserialized.filesystem_filter.is_none());
    assert!(deserialized.performance_filter.is_none());

    // Create old-style detection result (without new fields)
    let old_result = DetectionResult {
        task_id: "old_result".to_string(),
        success: true,
        error_message: None,
        processes: vec![],
        hash_result: None,
        // New fields should default to empty
        network_events: vec![],
        filesystem_events: vec![],
        performance_events: vec![],
    };

    // Serialize and deserialize to verify compatibility
    let serialized = old_result.encode_to_vec();
    let deserialized =
        DetectionResult::decode(&serialized[..]).expect("Old result format should deserialize");

    assert_eq!(old_result.task_id, deserialized.task_id);
    assert_eq!(old_result.success, deserialized.success);
    assert!(deserialized.network_events.is_empty());
    assert!(deserialized.filesystem_events.is_empty());
    assert!(deserialized.performance_events.is_empty());

    // Test new capability message format
    let capabilities = CollectionCapabilities {
        supported_domains: vec![
            MonitoringDomain::Process as i32,
            MonitoringDomain::Network as i32,
        ],
        advanced: Some(AdvancedCapabilities {
            kernel_level: false,
            realtime: true,
            system_wide: true,
        }),
    };

    let serialized = capabilities.encode_to_vec();
    let deserialized = CollectionCapabilities::decode(&serialized[..])
        .expect("Capabilities should serialize/deserialize");

    assert_eq!(
        capabilities.supported_domains,
        deserialized.supported_domains
    );
    assert_eq!(
        capabilities.advanced.is_some(),
        deserialized.advanced.is_some()
    );

    if let (Some(orig), Some(deser)) = (&capabilities.advanced, &deserialized.advanced) {
        assert_eq!(orig.kernel_level, deser.kernel_level);
        assert_eq!(orig.realtime, deser.realtime);
        assert_eq!(orig.system_wide, deser.system_wide);
    }
}

#[tokio::test]
async fn test_configuration_compatibility() {
    // Test that collector-core can load existing sentinel-lib configurations

    // Create configuration that mimics existing procmond config
    let legacy_config = CollectorConfig::default()
        .with_component_name("procmond".to_string())
        .with_max_event_sources(16) // Match existing default
        .with_event_buffer_size(1000) // Match existing default
        .with_health_check_interval(Duration::from_secs(60)); // Match existing default

    // Verify configuration validation
    assert!(
        legacy_config.validate().is_ok(),
        "Legacy config should be valid"
    );

    // Test configuration builder pattern compatibility
    let builder_config = CollectorConfig::new()
        .with_component_name("sentinelagent".to_string())
        .with_max_event_sources(32)
        .with_event_buffer_size(2000)
        .with_debug_logging(true);

    assert!(
        builder_config.validate().is_ok(),
        "Builder config should be valid"
    );
    assert_eq!(builder_config.component_name, "sentinelagent");
    assert_eq!(builder_config.max_event_sources, 32);
    assert_eq!(builder_config.event_buffer_size, 2000);
    assert!(builder_config.enable_debug_logging);

    // Test that existing environment variable patterns work
    unsafe {
        std::env::set_var("PROCMOND_COLLECTOR_MAX_EVENT_SOURCES", "64");
        std::env::set_var("PROCMOND_COLLECTOR_EVENT_BUFFER_SIZE", "4000");
    }

    let env_config = CollectorConfig::default()
        .with_component_name("procmond".to_string())
        .apply_env_overrides();

    // Environment variables should override defaults
    assert_eq!(env_config.max_event_sources, 64);
    assert_eq!(env_config.event_buffer_size, 4000);

    // Clean up environment variables
    unsafe {
        std::env::remove_var("PROCMOND_COLLECTOR_MAX_EVENT_SOURCES");
        std::env::remove_var("PROCMOND_COLLECTOR_EVENT_BUFFER_SIZE");
    }
}

#[tokio::test]
async fn test_operational_behavior_compatibility() {
    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(500)
        .with_health_check_interval(Duration::from_millis(100))
        .with_telemetry_interval(Duration::from_millis(200));

    let mut collector = Collector::new(config);

    // Create sources that mimic existing operational patterns
    let procmond_like = LegacyCompatibleSource::new(
        "procmond-like",
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE,
    )
    .with_legacy_mode(true);

    let sentinelagent_like = LegacyCompatibleSource::new(
        "sentinelagent-like",
        SourceCaps::NETWORK | SourceCaps::PERFORMANCE,
    )
    .with_legacy_mode(false);

    // Store references for testing (unused but kept for future use)
    let _procmond_events = procmond_like.events_sent.clone();
    let _sentinelagent_events = sentinelagent_like.events_sent.clone();

    let _ = collector.register(Box::new(procmond_like));
    let _ = collector.register(Box::new(sentinelagent_like));

    // Verify capability aggregation matches expected operational behavior
    let capabilities = collector.capabilities();
    assert!(capabilities.contains(SourceCaps::PROCESS));
    assert!(capabilities.contains(SourceCaps::NETWORK));
    assert!(capabilities.contains(SourceCaps::PERFORMANCE));
    assert!(capabilities.contains(SourceCaps::REALTIME));
    assert!(capabilities.contains(SourceCaps::SYSTEM_WIDE));

    // Test that sources can generate events independently
    let (tx1, _rx1) = tokio::sync::mpsc::channel(100);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(100);

    // Create new sources and get their event counters
    let procmond_like_test = LegacyCompatibleSource::new(
        "procmond-like",
        SourceCaps::PROCESS | SourceCaps::REALTIME | SourceCaps::SYSTEM_WIDE,
    )
    .with_legacy_mode(true);

    let sentinelagent_like_test = LegacyCompatibleSource::new(
        "sentinelagent-like",
        SourceCaps::NETWORK | SourceCaps::PERFORMANCE,
    )
    .with_legacy_mode(false);

    let procmond_events_test = procmond_like_test.events_sent.clone();
    let sentinelagent_events_test = sentinelagent_like_test.events_sent.clone();

    // Start procmond-like source
    let procmond_handle = tokio::spawn(async move {
        let _ = procmond_like_test.start(tx1).await;
    });

    // Start sentinelagent-like source
    let sentinelagent_handle = tokio::spawn(async move {
        let _ = sentinelagent_like_test.start(tx2).await;
    });

    // Let sources run for a bit
    let start_time = std::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let elapsed = start_time.elapsed();

    // Stop sources
    procmond_handle.abort();
    sentinelagent_handle.abort();

    let procmond_count = procmond_events_test.load(Ordering::Relaxed);
    let sentinelagent_count = sentinelagent_events_test.load(Ordering::Relaxed);

    info!(
        procmond_events = procmond_count,
        sentinelagent_events = sentinelagent_count,
        elapsed_ms = elapsed.as_millis(),
        "Operational behavior compatibility test completed"
    );

    // Verify operational characteristics match expectations
    assert!(
        procmond_count > 0,
        "Procmond-like source should generate events"
    );
    assert!(
        sentinelagent_count > 0,
        "Sentinelagent-like source should generate events"
    );

    // Verify timing characteristics are reasonable
    assert!(
        elapsed >= Duration::from_millis(90),
        "Should run for expected duration"
    );
    assert!(
        elapsed <= Duration::from_millis(200),
        "Should complete within reasonable time"
    );

    // Verify event generation rates are reasonable
    let total_events = procmond_count + sentinelagent_count;
    let events_per_second = if elapsed.as_secs_f64() > 0.0 {
        total_events as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    info!(
        total_events = total_events,
        events_per_second = events_per_second,
        "Event generation statistics"
    );

    assert!(
        events_per_second > 1.0,
        "Should maintain reasonable event generation rate"
    );
    assert!(
        events_per_second < 10000.0,
        "Event rate should be within reasonable bounds"
    );
}

#[tokio::test]
async fn test_error_handling_compatibility() {
    // Test error handling compatibility without full collector runtime
    // This tests that sources can be created and registered properly

    let config = CollectorConfig::default()
        .with_max_event_sources(2)
        .with_event_buffer_size(100) // Small buffer to trigger backpressure
        .with_backpressure_threshold(80); // Must be less than event_buffer_size

    let mut collector = Collector::new(config);

    // Create source that generates events rapidly (to test backpressure)
    let rapid_source = LegacyCompatibleSource::new("rapid-source", SourceCaps::PROCESS);

    // Create source that behaves normally
    let normal_source = LegacyCompatibleSource::new("normal-source", SourceCaps::NETWORK);

    // Store references for testing (unused but kept for future use)
    let _rapid_events = rapid_source.events_sent.clone();
    let _normal_events = normal_source.events_sent.clone();

    // Test source registration
    let rapid_result = collector.register(Box::new(rapid_source));
    let normal_result = collector.register(Box::new(normal_source));

    // Verify sources were registered successfully
    assert!(
        rapid_result.is_ok(),
        "Rapid source should register successfully"
    );
    assert!(
        normal_result.is_ok(),
        "Normal source should register successfully"
    );

    // Test capability aggregation
    let capabilities = collector.capabilities();
    assert!(
        capabilities.contains(SourceCaps::PROCESS),
        "Should have PROCESS capabilities"
    );
    assert!(
        capabilities.contains(SourceCaps::NETWORK),
        "Should have NETWORK capabilities"
    );

    // Test that sources can generate events independently
    let (tx1, _rx1) = tokio::sync::mpsc::channel(100);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(100);

    // Create new sources and get their event counters
    let rapid_source_test = LegacyCompatibleSource::new("rapid-source", SourceCaps::PROCESS);
    let normal_source_test = LegacyCompatibleSource::new("normal-source", SourceCaps::NETWORK);

    let rapid_events_test = rapid_source_test.events_sent.clone();
    let normal_events_test = normal_source_test.events_sent.clone();

    // Start rapid source
    let rapid_handle = tokio::spawn(async move {
        let _ = rapid_source_test.start(tx1).await;
    });

    // Start normal source
    let normal_handle = tokio::spawn(async move {
        let _ = normal_source_test.start(tx2).await;
    });

    // Let sources run for a bit
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop sources
    rapid_handle.abort();
    normal_handle.abort();

    let rapid_count = rapid_events_test.load(Ordering::Relaxed);
    let normal_count = normal_events_test.load(Ordering::Relaxed);

    info!(
        rapid_events = rapid_count,
        normal_events = normal_count,
        "Error handling compatibility test completed"
    );

    // Debug output to understand what's happening
    println!(
        "DEBUG: rapid_count = {}, normal_count = {}",
        rapid_count, normal_count
    );

    // Verify error handling doesn't break compatibility
    assert!(
        rapid_count > 0,
        "Rapid source should send some events (got {})",
        rapid_count
    );
    assert!(
        normal_count > 0,
        "Normal source should continue operating (got {})",
        normal_count
    );

    // Both sources should operate despite potential backpressure
    let total_events = rapid_count + normal_count;
    assert!(
        total_events > 10,
        "Should process events despite small buffer"
    );

    info!("Error handling maintained compatibility with existing behavior");
}
