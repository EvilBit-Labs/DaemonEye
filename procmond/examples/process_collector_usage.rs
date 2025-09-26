//! Example demonstrating the ProcessCollector trait usage in ProcessMessageHandler.
//!
//! This example shows how the refactored ProcessMessageHandler uses the ProcessCollector
//! trait for platform-agnostic process enumeration with proper error handling.

use async_trait::async_trait;
use collector_core::ProcessEvent;
use daemoneye_lib::{
    proto::{DetectionTask, TaskType},
    storage::DatabaseManager,
};
use procmond::{
    CollectionStats, ProcessCollectionConfig, ProcessCollectionResult, ProcessCollector,
    ProcessMessageHandler, SysinfoProcessCollector,
};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;

/// Example custom ProcessCollector implementation.
///
/// This demonstrates how to create a custom collector that could be used
/// for platform-specific optimizations or testing scenarios.
pub struct ExampleProcessCollector {
    name: String,
}

impl ExampleProcessCollector {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl ProcessCollector for ExampleProcessCollector {
    fn name(&self) -> &'static str {
        "example-collector"
    }

    fn capabilities(&self) -> procmond::ProcessCollectorCapabilities {
        procmond::ProcessCollectorCapabilities {
            basic_info: true,
            enhanced_metadata: false,
            executable_hashing: false,
            system_processes: true,
            kernel_threads: false,
            realtime_collection: false,
        }
    }

    async fn collect_processes(
        &self,
    ) -> ProcessCollectionResult<(Vec<ProcessEvent>, CollectionStats)> {
        // Example implementation that returns mock data
        let now = SystemTime::now();
        let processes = vec![ProcessEvent {
            pid: 1,
            ppid: None,
            name: "init".to_string(),
            executable_path: Some("/sbin/init".to_string()),
            command_line: vec!["/sbin/init".to_string()],
            start_time: Some(now),
            cpu_usage: Some(0.1),
            memory_usage: Some(1024 * 1024),
            executable_hash: None,
            user_id: Some("0".to_string()),
            accessible: true,
            file_exists: true,
            timestamp: now,
            platform_metadata: None,
        }];

        let stats = CollectionStats {
            total_processes: 1,
            successful_collections: 1,
            inaccessible_processes: 0,
            invalid_processes: 0,
            collection_duration_ms: 5,
        };

        Ok((processes, stats))
    }

    async fn collect_process(&self, pid: u32) -> ProcessCollectionResult<ProcessEvent> {
        if pid == 1 {
            let now = SystemTime::now();
            Ok(ProcessEvent {
                pid: 1,
                ppid: None,
                name: "init".to_string(),
                executable_path: Some("/sbin/init".to_string()),
                command_line: vec!["/sbin/init".to_string()],
                start_time: Some(now),
                cpu_usage: Some(0.1),
                memory_usage: Some(1024 * 1024),
                executable_hash: None,
                user_id: Some("0".to_string()),
                accessible: true,
                file_exists: true,
                timestamp: now,
                platform_metadata: None,
            })
        } else {
            Err(procmond::ProcessCollectionError::ProcessNotFound { pid })
        }
    }

    async fn health_check(&self) -> ProcessCollectionResult<()> {
        println!("Health check for {}", self.name);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for structured logging
    tracing_subscriber::fmt::init();

    println!("ProcessCollector Trait Usage Example");
    println!("====================================");

    // Create a temporary database for this example
    let temp_dir = tempfile::TempDir::new()?;
    let db_path = temp_dir.path().join("example.db");
    let db_manager = Arc::new(Mutex::new(DatabaseManager::new(&db_path)?));

    // Example 1: Using the default SysinfoProcessCollector
    println!("\n1. Using default SysinfoProcessCollector:");
    let handler_default = ProcessMessageHandler::new(Arc::clone(&db_manager));

    println!("   Collector name: {}", handler_default.collector.name());
    println!(
        "   Capabilities: {:?}",
        handler_default.collector.capabilities()
    );

    let task = DetectionTask::new_test_task("example-task-1", TaskType::EnumerateProcesses, None);
    let result = handler_default.enumerate_processes(&task).await?;

    println!(
        "   Enumeration result: success={}, processes={}",
        result.success,
        result.processes.len()
    );

    // Example 2: Using a custom ProcessCollector configuration
    println!("\n2. Using custom SysinfoProcessCollector configuration:");
    let custom_config = ProcessCollectionConfig {
        collect_enhanced_metadata: false,
        compute_executable_hashes: true,
        max_processes: 10,
        skip_system_processes: true,
        skip_kernel_threads: true,
    };
    let custom_collector = Box::new(SysinfoProcessCollector::new(custom_config));
    let handler_custom =
        ProcessMessageHandler::with_collector(Arc::clone(&db_manager), custom_collector);

    println!("   Collector name: {}", handler_custom.collector.name());
    println!(
        "   Capabilities: {:?}",
        handler_custom.collector.capabilities()
    );

    let task2 = DetectionTask::new_test_task("example-task-2", TaskType::EnumerateProcesses, None);
    let result2 = handler_custom.enumerate_processes(&task2).await?;

    println!(
        "   Enumeration result: success={}, processes={}",
        result2.success,
        result2.processes.len()
    );

    // Example 3: Using a completely custom ProcessCollector
    println!("\n3. Using custom ExampleProcessCollector:");
    let example_collector = Box::new(ExampleProcessCollector::new(
        "my-custom-collector".to_string(),
    ));
    let handler_example =
        ProcessMessageHandler::with_collector(Arc::clone(&db_manager), example_collector);

    println!("   Collector name: {}", handler_example.collector.name());
    println!(
        "   Capabilities: {:?}",
        handler_example.collector.capabilities()
    );

    let task3 = DetectionTask::new_test_task("example-task-3", TaskType::EnumerateProcesses, None);
    let result3 = handler_example.enumerate_processes(&task3).await?;

    println!(
        "   Enumeration result: success={}, processes={}",
        result3.success,
        result3.processes.len()
    );

    // Example 4: Demonstrating error handling
    println!("\n4. Demonstrating error handling:");

    // Test health check
    match handler_example.collector.health_check().await {
        Ok(()) => println!("   Health check: PASSED"),
        Err(e) => println!("   Health check: FAILED - {}", e),
    }

    // Test single process collection
    match handler_example.collector.collect_process(1).await {
        Ok(event) => println!(
            "   Single process collection: Found process '{}' (PID: {})",
            event.name, event.pid
        ),
        Err(e) => println!("   Single process collection: FAILED - {}", e),
    }

    // Test non-existent process
    match handler_example.collector.collect_process(99999).await {
        Ok(event) => println!(
            "   Non-existent process: Found process '{}' (PID: {})",
            event.name, event.pid
        ),
        Err(e) => println!("   Non-existent process: EXPECTED ERROR - {}", e),
    }

    println!("\nExample completed successfully!");
    println!("\nKey Benefits of ProcessCollector Trait:");
    println!("- Platform-agnostic interface for process enumeration");
    println!("- Extensible design for custom implementations");
    println!("- Proper error handling for inaccessible processes");
    println!("- Structured logging and comprehensive statistics");
    println!("- Easy testing with mock implementations");

    Ok(())
}
