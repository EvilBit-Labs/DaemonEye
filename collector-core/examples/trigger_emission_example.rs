//! Example demonstrating trigger request emission for analysis collector coordination.
//!
//! This example shows how to:
//! 1. Set up a trigger manager with collector capabilities
//! 2. Register trigger conditions for analysis coordination
//! 3. Emit trigger requests to analysis collectors
//! 4. Handle timeout tracking and correlation
//! 5. Monitor emission statistics

use collector_core::{
    EventBusConfig, LocalEventBus,
    event::{AnalysisType, TriggerPriority, TriggerRequest},
    trigger::{
        ConditionType, ProcessTriggerData, TriggerCapabilities, TriggerCondition, TriggerConfig,
        TriggerManager, TriggerResourceLimits,
    },
};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("🚀 Starting trigger emission example");

    // 1. Create trigger manager with configuration
    let config = TriggerConfig {
        max_triggers_per_collector: 50,
        rate_limit_window_secs: 60,
        deduplication_window_secs: 300,
        max_pending_triggers: 1000,
        enable_metadata_tracking: true,
    };
    let mut manager = TriggerManager::new(config);

    // 2. Register collector capabilities for binary hasher
    let binary_hasher_caps = TriggerCapabilities {
        collector_id: "binary-hasher".to_string(),
        supported_conditions: vec![
            ConditionType::ProcessNamePattern("malware".to_string()),
            ConditionType::MissingExecutable,
            ConditionType::HashMismatch,
        ],
        supported_analysis: vec![AnalysisType::BinaryHash],
        max_trigger_rate: 100,
        max_concurrent_analysis: 10,
        supported_priorities: vec![
            TriggerPriority::Low,
            TriggerPriority::Normal,
            TriggerPriority::High,
            TriggerPriority::Critical,
        ],
        resource_limits: TriggerResourceLimits {
            max_memory_per_task: 100 * 1024 * 1024, // 100MB
            max_analysis_time_ms: 30_000,           // 30 seconds
            max_queue_depth: 1000,
        },
    };

    manager.register_collector_capabilities(binary_hasher_caps)?;
    println!("✅ Registered binary hasher capabilities");

    // 3. Register collector capabilities for memory analyzer
    let memory_analyzer_caps = TriggerCapabilities {
        collector_id: "memory-analyzer".to_string(),
        supported_conditions: vec![
            ConditionType::ResourceAnomaly {
                cpu_threshold: 80.0,
                memory_threshold: 1024 * 1024 * 1024, // 1GB
            },
            ConditionType::SuspiciousParentChild,
        ],
        supported_analysis: vec![
            AnalysisType::MemoryAnalysis,
            AnalysisType::BehavioralAnalysis,
        ],
        max_trigger_rate: 50,
        max_concurrent_analysis: 5,
        supported_priorities: vec![
            TriggerPriority::Normal,
            TriggerPriority::High,
            TriggerPriority::Critical,
        ],
        resource_limits: TriggerResourceLimits {
            max_memory_per_task: 500 * 1024 * 1024, // 500MB
            max_analysis_time_ms: 60_000,           // 60 seconds
            max_queue_depth: 500,
        },
    };

    manager.register_collector_capabilities(memory_analyzer_caps)?;
    println!("✅ Registered memory analyzer capabilities");

    // 4. Set up event bus for trigger emission (optional - for demonstration)
    let event_bus_config = EventBusConfig::default();
    let mut event_bus = LocalEventBus::new(event_bus_config).await?;
    event_bus.start().await?;
    manager.set_event_bus(Box::new(event_bus)).await;
    println!("✅ Configured event bus for trigger emission");

    // 5. Register trigger conditions
    let suspicious_process_condition = TriggerCondition {
        id: "suspicious-process-detection".to_string(),
        description: "Detect processes with suspicious names".to_string(),
        analysis_type: AnalysisType::BinaryHash,
        priority: TriggerPriority::High,
        target_collector: "binary-hasher".to_string(),
        condition_type: ConditionType::ProcessNamePattern("malware".to_string()),
    };

    manager.register_condition(suspicious_process_condition)?;
    println!("✅ Registered suspicious process condition");

    let missing_executable_condition = TriggerCondition {
        id: "missing-executable-detection".to_string(),
        description: "Detect processes with missing executable files".to_string(),
        analysis_type: AnalysisType::BinaryHash,
        priority: TriggerPriority::Critical,
        target_collector: "binary-hasher".to_string(),
        condition_type: ConditionType::MissingExecutable,
    };

    manager.register_condition(missing_executable_condition)?;
    println!("✅ Registered missing executable condition");

    // 6. Simulate process data that would trigger analysis
    let suspicious_process = ProcessTriggerData {
        pid: 1234,
        name: "malware.exe".to_string(),
        executable_path: Some("/tmp/malware.exe".to_string()),
        file_exists: true,
        cpu_usage: Some(15.5),
        memory_usage: Some(50 * 1024 * 1024), // 50MB
        executable_hash: Some("suspicious_hash_123".to_string()),
    };

    let missing_executable_process = ProcessTriggerData {
        pid: 5678,
        name: "phantom_process".to_string(),
        executable_path: Some("/nonexistent/path".to_string()),
        file_exists: false,
        cpu_usage: Some(5.0),
        memory_usage: Some(10 * 1024 * 1024), // 10MB
        executable_hash: None,
    };

    // 7. Evaluate trigger conditions and generate trigger requests
    println!("\n🔍 Evaluating trigger conditions...");

    let suspicious_triggers = manager.evaluate_triggers(&suspicious_process)?;
    println!(
        "Generated {} triggers for suspicious process",
        suspicious_triggers.len()
    );

    let missing_executable_triggers = manager.evaluate_triggers(&missing_executable_process)?;
    println!(
        "Generated {} triggers for missing executable",
        missing_executable_triggers.len()
    );

    // 8. Emit trigger requests to analysis collectors
    println!("\n📤 Emitting trigger requests...");

    for trigger in &suspicious_triggers {
        match manager.emit_trigger_request(trigger.clone()).await {
            Ok(()) => {
                println!(
                    "✅ Emitted trigger: {} -> {}",
                    trigger.trigger_id, trigger.target_collector
                );
            }
            Err(e) => {
                println!("❌ Failed to emit trigger {}: {}", trigger.trigger_id, e);
            }
        }
    }

    for trigger in &missing_executable_triggers {
        match manager.emit_trigger_request(trigger.clone()).await {
            Ok(()) => {
                println!(
                    "✅ Emitted trigger: {} -> {}",
                    trigger.trigger_id, trigger.target_collector
                );
            }
            Err(e) => {
                println!("❌ Failed to emit trigger {}: {}", trigger.trigger_id, e);
            }
        }
    }

    // 9. Demonstrate manual trigger request creation and emission
    println!("\n🎯 Creating manual trigger request...");

    let manual_trigger = TriggerRequest {
        trigger_id: "manual_trigger_001".to_string(),
        target_collector: "memory-analyzer".to_string(),
        analysis_type: AnalysisType::MemoryAnalysis,
        priority: TriggerPriority::High,
        target_pid: Some(9999),
        target_path: Some("/usr/bin/suspicious_app".to_string()),
        correlation_id: "investigation_001".to_string(),
        metadata: {
            let mut metadata = HashMap::new();
            metadata.insert("investigation_id".to_string(), "INV-2024-001".to_string());
            metadata.insert("analyst".to_string(), "security_team".to_string());
            metadata.insert("priority_reason".to_string(), "IOC_match".to_string());
            metadata
        },
        timestamp: SystemTime::now(),
    };

    match manager.emit_trigger_request(manual_trigger.clone()).await {
        Ok(()) => {
            println!(
                "✅ Emitted manual trigger: {} -> {}",
                manual_trigger.trigger_id, manual_trigger.target_collector
            );
        }
        Err(e) => {
            println!("❌ Failed to emit manual trigger: {}", e);
        }
    }

    // 10. Monitor trigger statistics
    println!("\n📊 Trigger system statistics:");
    let stats = manager.get_statistics();
    println!("  Registered conditions: {}", stats.registered_conditions);
    println!(
        "  Registered capabilities: {}",
        stats.registered_capabilities
    );
    println!("  Pending triggers: {}", stats.pending_triggers);
    println!("  Emission stats:");
    println!("    Total emitted: {}", stats.emission_stats.total_emitted);
    println!(
        "    Successful emissions: {}",
        stats.emission_stats.successful_emissions
    );
    println!(
        "    Validation failures: {}",
        stats.emission_stats.validation_failures
    );
    println!(
        "    Event bus failures: {}",
        stats.emission_stats.event_bus_failures
    );
    println!(
        "    Pending responses: {}",
        stats.emission_stats.pending_responses
    );
    println!(
        "    Avg latency: {:.2}ms",
        stats.emission_stats.avg_emission_latency_ms
    );

    // 11. Demonstrate timeout handling
    println!("\n⏰ Demonstrating timeout handling...");

    // Wait a bit to simulate processing time
    sleep(Duration::from_millis(100)).await;

    // Handle any timeouts (in a real system, this would be called periodically)
    let timed_out_triggers = manager.handle_trigger_timeouts().await?;
    if !timed_out_triggers.is_empty() {
        println!("⚠️  Found {} timed out triggers", timed_out_triggers.len());
        for trigger_id in &timed_out_triggers {
            println!("    Timed out: {}", trigger_id);
        }
    } else {
        println!("✅ No triggers have timed out");
    }

    // 12. Simulate completing some trigger requests
    println!("\n✅ Simulating trigger completion...");

    for trigger in &suspicious_triggers {
        manager
            .complete_trigger_request(&trigger.trigger_id)
            .await?;
        println!("Completed trigger: {}", trigger.trigger_id);
    }

    // 13. Final statistics
    println!("\n📈 Final statistics:");
    let final_stats = manager.get_statistics();
    println!(
        "  Total emitted: {}",
        final_stats.emission_stats.total_emitted
    );
    println!(
        "  Pending responses: {}",
        final_stats.emission_stats.pending_responses
    );
    println!("  Timeouts: {}", final_stats.emission_stats.timeouts);

    println!("\n🎉 Trigger emission example completed successfully!");

    Ok(())
}
