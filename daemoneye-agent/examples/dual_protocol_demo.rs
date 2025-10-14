//! Demonstration of dual-protocol architecture
//!
//! This example shows how daemoneye-agent runs both:
//! 1. EventBus broker for collector-core component communication
//! 2. IPC server for daemoneye-cli communication
//!
//! Both services coexist without interference.

use daemoneye_agent::{BrokerManager, IpcServerManager, create_cli_ipc_config};
use daemoneye_lib::config::BrokerConfig;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{Level, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    info!("Starting dual-protocol architecture demonstration");

    // Create configurations for both services
    let broker_config = BrokerConfig {
        socket_path: "/tmp/demo-eventbus.sock".to_string(),
        enabled: true,
        startup_timeout_seconds: 10,
        shutdown_timeout_seconds: 10,
        max_connections: 50,
        message_buffer_size: 1000,
        topic_hierarchy: daemoneye_lib::config::TopicHierarchyConfig::default(),
    };

    let mut cli_ipc_config = create_cli_ipc_config();
    cli_ipc_config.endpoint_path = "/tmp/demo-cli.sock".to_string();

    // Create both managers
    let broker_manager = BrokerManager::new(broker_config);
    let ipc_server_manager = IpcServerManager::new(cli_ipc_config);

    info!("Starting EventBus broker and IPC server...");

    // Start both services in parallel
    let (broker_result, ipc_result) =
        tokio::join!(broker_manager.start(), ipc_server_manager.start());

    if let Err(e) = broker_result {
        eprintln!("Failed to start EventBus broker: {}", e);
        return Err(e.into());
    }

    if let Err(e) = ipc_result {
        eprintln!("Failed to start IPC server: {}", e);
        return Err(e.into());
    }

    info!("Both services started successfully!");

    // Wait for both to become healthy
    info!("Waiting for services to become healthy...");
    let (broker_health_result, ipc_health_result) = tokio::join!(
        broker_manager.wait_for_healthy(Duration::from_secs(5)),
        ipc_server_manager.wait_for_healthy(Duration::from_secs(5))
    );

    if let Err(e) = broker_health_result {
        eprintln!("EventBus broker failed to become healthy: {}", e);
        return Err(e.into());
    }

    if let Err(e) = ipc_health_result {
        eprintln!("IPC server failed to become healthy: {}", e);
        return Err(e.into());
    }

    info!("Both services are healthy and ready!");

    // Display service information
    info!("EventBus broker socket: {}", broker_manager.socket_path());
    info!(
        "IPC server endpoint: {}",
        ipc_server_manager.endpoint_path()
    );

    // Run for a few seconds to demonstrate coexistence
    info!("Services running in dual-protocol mode...");
    for i in 1..=5 {
        sleep(Duration::from_secs(1)).await;

        // Check health of both services
        let broker_health = broker_manager.health_check().await;
        let ipc_health = ipc_server_manager.health_check().await;

        info!(
            "Health check {} - Broker: {:?}, IPC: {:?}",
            i, broker_health, ipc_health
        );

        // Show broker statistics
        if let Some(stats) = broker_manager.statistics().await {
            info!(
                "Broker stats - Published: {}, Delivered: {}, Subscribers: {}",
                stats.messages_published, stats.messages_delivered, stats.active_subscribers
            );
        }
    }

    info!("Shutting down services...");

    // Shutdown both services in parallel
    let (broker_shutdown_result, ipc_shutdown_result) =
        tokio::join!(broker_manager.shutdown(), ipc_server_manager.shutdown());

    if let Err(e) = broker_shutdown_result {
        eprintln!("Failed to shutdown EventBus broker: {}", e);
    }

    if let Err(e) = ipc_shutdown_result {
        eprintln!("Failed to shutdown IPC server: {}", e);
    }

    info!("Dual-protocol architecture demonstration complete!");
    Ok(())
}
