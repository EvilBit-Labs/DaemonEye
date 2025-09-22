//! Example demonstrating advanced IPC client features
//!
//! This example shows how to use the advanced IPC client with:
//! - Multiple collector endpoints
//! - Load balancing strategies
//! - Connection pooling
//! - Circuit breaker patterns
//! - Comprehensive metrics collection

use daemoneye_lib::ipc::{
    IpcConfig, TransportType,
    client::{CollectorEndpoint, LoadBalancingStrategy, ResilientIpcClient},
};
use daemoneye_lib::proto::{DetectionTask, TaskType};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging (using println for this example)
    println!("\u{1f527} Initializing advanced IPC client example...");

    // Create multiple collector endpoints for load balancing and failover
    let endpoints = vec![
        CollectorEndpoint::new(
            "primary-collector".to_owned(),
            "/tmp/collector-primary.sock".to_owned(),
            1, // Highest priority
        ),
        CollectorEndpoint::new(
            "secondary-collector".to_owned(),
            "/tmp/collector-secondary.sock".to_owned(),
            2, // Lower priority
        ),
        CollectorEndpoint::new(
            "backup-collector".to_owned(),
            "/tmp/collector-backup.sock".to_owned(),
            3, // Lowest priority
        ),
    ];

    // Create IPC configuration
    let config = IpcConfig {
        transport: TransportType::Interprocess,
        endpoint_path: "/tmp/default-collector.sock".to_owned(), // Fallback endpoint
        max_frame_bytes: 1024 * 1024,
        accept_timeout_ms: 5000,
        read_timeout_ms: 30000,
        write_timeout_ms: 10000,
        max_connections: 32, // Allow more connections for multiple collectors
        panic_strategy: daemoneye_lib::ipc::PanicStrategy::Unwind,
    };

    // Create advanced client with multiple endpoints and weighted load balancing
    let mut client = ResilientIpcClient::new_with_endpoints(
        &config,
        endpoints,
        LoadBalancingStrategy::Weighted, // Use health-based weighted selection
    );

    println!("\u{1f680} Advanced IPC Client Example");
    println!("===============================");

    // Display initial endpoint configuration
    let client_endpoints = client.get_endpoints().await;
    println!("\n\u{1f4e1} Configured Endpoints:");
    for endpoint in &client_endpoints {
        println!(
            "  \u{2022} {} (priority: {}, healthy: {})",
            endpoint.id, endpoint.priority, endpoint.is_healthy
        );
    }

    // Add another endpoint dynamically
    let dynamic_endpoint = CollectorEndpoint::new(
        "dynamic-collector".to_owned(),
        "/tmp/collector-dynamic.sock".to_owned(),
        4,
    );
    client.add_endpoint(dynamic_endpoint).await;
    println!("\n\u{2795} Added dynamic endpoint");

    // Note: Skipping health check in this demo since no servers are running
    println!("\n\u{1f3e5} Health check (skipped - no servers running in demo)");

    // Get comprehensive statistics
    let stats = client.get_stats().await;
    println!("\n\u{1f4ca} Client Statistics:");
    println!(
        "  \u{2022} Load balancing strategy: {}",
        match stats.load_balancing_strategy {
            LoadBalancingStrategy::RoundRobin => "RoundRobin",
            LoadBalancingStrategy::Weighted => "Weighted",
            LoadBalancingStrategy::Priority => "Priority",
            _ => "Unknown",
        }
    );
    println!(
        "  \u{2022} Total pooled connections: {}",
        stats.total_pooled_connections
    );
    println!("  \u{2022} Endpoint count: {}", stats.endpoint_stats.len());

    // Display endpoint-specific statistics
    println!("\n\u{1f3af} Endpoint Statistics:");
    for endpoint_stat in &stats.endpoint_stats {
        println!(
            "  \u{2022} {} (priority: {})",
            endpoint_stat.endpoint_id, endpoint_stat.priority
        );
        println!("    - Health score: {:.2}", endpoint_stat.health_score);
        println!(
            "    - Circuit breaker: {}",
            match endpoint_stat.circuit_breaker_state {
                daemoneye_lib::ipc::client::CircuitBreakerState::Closed => "Closed",
                daemoneye_lib::ipc::client::CircuitBreakerState::Open => "Open",
                daemoneye_lib::ipc::client::CircuitBreakerState::HalfOpen => "HalfOpen",
                _ => "Unknown",
            }
        );
        println!("    - Healthy: {}", endpoint_stat.is_healthy);
    }

    // Display metrics
    let metrics = client.metrics();
    println!("\n\u{1f4c8} Performance Metrics:");
    println!(
        "  \u{2022} Tasks sent: {}",
        metrics.tasks_sent_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Tasks completed: {}",
        metrics.tasks_completed_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Tasks failed: {}",
        metrics.tasks_failed_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Connection attempts: {}",
        metrics.connection_attempts_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Connections established: {}",
        metrics
            .connections_established_total
            .load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Success rate: {:.2}%",
        metrics.success_rate() * 100.0
    );

    // Simulate sending tasks (these will fail since no servers are running)
    println!("\n\u{1f504} Simulating task distribution...");
    for i in 1..=3 {
        let task = DetectionTask {
            task_id: format!("demo-task-{i}"),
            task_type: TaskType::EnumerateProcesses.into(),
            process_filter: None,
            hash_check: None,
            metadata: Some("Advanced client demo".to_owned()),
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        println!("  \u{1f4e4} Sending task: {}", task.task_id);

        // Use a short timeout for demo purposes
        match tokio::time::timeout(Duration::from_millis(500), client.send_task(task)).await {
            Ok(Ok(result)) => {
                println!("    \u{2705} Task completed: {}", result.task_id);
            }
            Ok(Err(e)) => {
                println!("    \u{274c} Task failed: {e}");
            }
            Err(_) => {
                println!("    \u{23f0} Task timed out (expected - no servers running)");
            }
        }

        // Small delay between tasks
        sleep(Duration::from_millis(100)).await;
    }

    // Show updated metrics after task attempts
    println!("\n\u{1f4ca} Updated Metrics:");
    println!(
        "  \u{2022} Tasks sent: {}",
        metrics.tasks_sent_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Tasks failed: {}",
        metrics.tasks_failed_total.load(Ordering::Relaxed)
    );
    println!(
        "  \u{2022} Average task duration: {:.2}ms",
        metrics.average_task_duration_ms()
    );
    println!(
        "  \u{2022} Connection success rate: {:.2}%",
        metrics.connection_success_rate() * 100.0
    );

    // Demonstrate endpoint priority updates
    println!("\n\u{1f527} Updating endpoint priorities...");
    client.update_endpoint_priority("backup-collector", 1).await;
    println!("  \u{2022} Promoted backup-collector to highest priority");

    // Show final endpoint statistics
    let final_stats = client.get_stats().await;
    println!("\n\u{1f3c1} Final Endpoint Priorities:");
    for endpoint_stat in &final_stats.endpoint_stats {
        println!(
            "  \u{2022} {} (priority: {})",
            endpoint_stat.endpoint_id, endpoint_stat.priority
        );
    }

    // Cleanup connections
    client.cleanup_connections().await;
    println!("\n\u{1f9f9} Connection cleanup completed");

    println!("\n\u{2728} Advanced IPC Client demo completed!");
    println!("This example demonstrated:");
    println!("  \u{2022} Multiple collector endpoint management");
    println!("  \u{2022} Load balancing strategies (Weighted, RoundRobin, Priority)");
    println!("  \u{2022} Circuit breaker patterns for fault tolerance");
    println!("  \u{2022} Connection pooling and resource management");
    println!("  \u{2022} Comprehensive metrics collection");
    println!("  \u{2022} Dynamic endpoint configuration");
    println!("  \u{2022} Health monitoring and diagnostics");

    Ok(())
}
