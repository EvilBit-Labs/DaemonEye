#![forbid(unsafe_code)]

use clap::Parser;
use daemoneye_lib::{alerting, config, detection, models, storage, telemetry};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

mod broker_manager;
mod collector_registry;
mod health;
mod ipc_server;

use broker_manager::BrokerManager;
use ipc_server::IpcServerManager;

#[derive(Parser)]
#[command(name = "daemoneye-agent")]
#[command(about = "DaemonEye Detection and Alerting Orchestrator")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/daemoneye/processes.db")]
    database: String,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments first - this will handle --help and --version automatically
    let cli = Cli::parse();
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Test mode: exit early to keep existing integration test semantics (set DAEMONEYE_AGENT_TEST_MODE=1)
    if std::env::var("DAEMONEYE_AGENT_TEST_MODE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
        {
            println!("daemoneye-agent started successfully")
        };
        return Ok(());
    }

    // Load configuration
    let config_loader = config::ConfigLoader::new("daemoneye-agent");
    let mut config = config_loader.load()?;

    // Override database path from CLI argument if provided
    config.database.path = cli.database.into();

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("daemoneye-agent".to_owned());

    // Initialize database
    let _db_manager = storage::DatabaseManager::new(&config.database.path)?;

    // Initialize embedded EventBus broker
    let broker_manager = BrokerManager::new(config.broker.clone());

    // Start the embedded broker
    if let Err(e) = broker_manager.start().await {
        error!(error = %e, "Failed to start embedded EventBus broker");
        return Err(e.into());
    }

    // Wait for broker to become healthy
    let broker_startup_timeout = Duration::from_secs(config.broker.startup_timeout_seconds);
    if let Err(e) = broker_manager
        .wait_for_healthy(broker_startup_timeout)
        .await
    {
        error!(error = %e, "Embedded broker failed to become healthy");
        return Err(e.into());
    }

    info!(
        socket_path = %broker_manager.socket_path(),
        "Embedded EventBus broker is healthy and ready"
    );

    // Initialize IPC server for CLI communication
    let cli_ipc_config = ipc_server::create_cli_ipc_config();
    let ipc_server_manager = IpcServerManager::new(cli_ipc_config);

    // Start the IPC server
    if let Err(e) = ipc_server_manager.start().await {
        error!(error = %e, "Failed to start IPC server for CLI communication");
        return Err(e.into());
    }

    // Wait for IPC server to become healthy
    let ipc_startup_timeout = Duration::from_secs(10); // 10 second timeout for IPC server
    if let Err(e) = ipc_server_manager
        .wait_for_healthy(ipc_startup_timeout)
        .await
    {
        error!(error = %e, "IPC server failed to become healthy");
        return Err(e.into());
    }

    info!(
        endpoint_path = %ipc_server_manager.endpoint_path(),
        "IPC server is healthy and ready for CLI communication"
    );

    // Initialize detection engine
    let mut detection_engine = detection::DetectionEngine::new();

    // Create a sample detection rule
    let rule = models::DetectionRule::new(
        "rule-1".to_owned(),
        "Test Rule".to_owned(),
        "Test detection rule".to_owned(),
        "SELECT * FROM processes WHERE name = 'test'".to_owned(),
        "test".to_owned(),
        models::AlertSeverity::Medium,
    );

    // Load the rule
    detection_engine.load_rule(rule)?;

    // Initialize alert manager
    let mut alert_manager = alerting::AlertManager::new();
    let stdout_sink = Box::new(alerting::StdoutSink::new(
        "stdout".to_owned(),
        alerting::OutputFormat::Json,
    ));
    alert_manager.add_sink(stdout_sink);

    // Indicate startup success before entering main loop
    #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
    {
        println!("daemoneye-agent started successfully")
    };

    // Main collection loop using IPC client
    let scan_interval = Duration::from_millis(config.app.scan_interval_ms);
    info!(
        interval_ms = config.app.scan_interval_ms,
        "Entering main collection+detection loop with RPC client"
    );

    // Graceful shutdown signal future
    let shutdown_signal = async {
        // Wait for Ctrl+C
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "Failed to listen for shutdown signal");
        }
    };

    // Main loop task
    // detection_engine already mutable above; reuse directly
    let mut iteration: u64 = 0;

    tokio::pin!(shutdown_signal);

    loop {
        tokio::select! {
            () = &mut shutdown_signal => {
                info!("Shutdown signal received; commencing graceful shutdown");
                break;
            }
            () = tokio::time::sleep(scan_interval) => {
                iteration = iteration.saturating_add(1);
                let loop_start = Instant::now();

                // Periodic RPC health checks (every 10 iterations)
                if iteration.is_multiple_of(10) {
                    // Get list of registered collectors from RPC clients
                    let collector_ids = broker_manager.list_registered_collector_ids().await;

                    for collector_id in collector_ids {
                        match broker_manager.health_check_rpc(&collector_id).await {
                            Ok(health_data) => {
                                info!(
                                    collector_id = %collector_id,
                                    status = ?health_data.status,
                                    "RPC health check completed"
                                );
                            }
                            Err(e) => {
                                warn!(
                                    collector_id = %collector_id,
                                    error = %e,
                                    "RPC health check failed"
                                );
                            }
                        }
                    }
                }

                // Request process enumeration from procmond via RPC
                let task = daemoneye_lib::proto::DetectionTask::new_enumerate_processes(
                    uuid::Uuid::new_v4().to_string(),
                    None,
                );

                let processes = match broker_manager.execute_task_rpc("procmond", task).await {
                    Ok(result) => {
                        if result.success {
                            info!(
                                process_count = result.processes.len(),
                                "Successfully collected process data from procmond via RPC"
                            );
                            // Parse process data from DetectionResult.processes
                            result
                                .processes
                                .into_iter()
                                .map(Into::into)
                                .collect()
                        } else {
                            warn!(
                                error = %result.error_message.as_deref().unwrap_or("Unknown error"),
                                "Procmond returned error during process enumeration via RPC"
                            );
                            Vec::new()
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to collect processes from procmond via RPC");
                        Vec::new()
                    }
                };

                // Execute detection rules against collected processes
                let detection_timer = telemetry::PerformanceTimer::start("detection_execution".to_owned());
                let alerts = detection_engine.execute_rules(&processes);

                if !alerts.is_empty() {
                    info!(count = alerts.len(), "Generated alerts");
                }
                for alert in &alerts {
                    match alert_manager.send_alert(alert).await {
                        Ok(results) => {
                            if results.is_empty() { warn!("Alert generated but no sinks succeeded"); }
                        }
                        Err(e) => {
                            error!(error=?e, "Failed to deliver alert");
                            telemetry.record_error();
                        }
                    }
                }
                let detection_duration = detection_timer.finish();
                telemetry.record_operation(detection_duration);

                // Update telemetry with rough resource usage snapshot (placeholder zeros for now)
                telemetry.update_resource_usage(0.0, 0);
                if iteration.is_multiple_of(10) { // periodic health check every 10 iterations
                    let h = telemetry.health_check();
                    info!(status=%h.status, "Telemetry health check");

                    // Check broker health
                    let broker_health = broker_manager.health_check().await;
                    match broker_health {
                        broker_manager::BrokerHealth::Healthy => {
                            if let Some(stats) = broker_manager.statistics().await {
                                debug!(
                                    messages_published = stats.messages_published,
                                    messages_delivered = stats.messages_delivered,
                                    active_subscribers = stats.active_subscribers,
                                    uptime_seconds = stats.uptime_seconds,
                                    "Broker health check passed"
                                );
                            }
                        }
                        broker_manager::BrokerHealth::Unhealthy(ref error) => {
                            warn!(error = %error, "Broker health check failed");
                        }
                        broker_manager::BrokerHealth::Starting
                        | broker_manager::BrokerHealth::ShuttingDown
                        | broker_manager::BrokerHealth::Stopped => {
                            debug!(status = ?broker_health, "Broker health status");
                        }
                    }

                    // Check IPC server health
                    let ipc_health = ipc_server_manager.health_check().await;
                    match ipc_health {
                        ipc_server::IpcServerHealth::Healthy => {
                            debug!("IPC server health check passed");
                        }
                        ipc_server::IpcServerHealth::Unhealthy(ref error) => {
                            warn!(error = %error, "IPC server health check failed");
                        }
                        ipc_server::IpcServerHealth::Starting
                        | ipc_server::IpcServerHealth::ShuttingDown
                        | ipc_server::IpcServerHealth::Stopped => {
                            debug!(status = ?ipc_health, "IPC server health status");
                        }
                    }
                }
                let loop_elapsed = loop_start.elapsed();
                #[allow(clippy::as_conversions)] // Safe: loop elapsed will not overflow u64
                let elapsed_ms = loop_elapsed.as_millis() as u64;
                if loop_elapsed > scan_interval { warn!(elapsed_ms = elapsed_ms, "Loop overran scan interval"); }
            }
        }
    }

    // Gracefully shutdown both services in parallel
    info!("Shutting down IPC server and embedded EventBus broker");

    let (ipc_result, broker_result) =
        tokio::join!(ipc_server_manager.shutdown(), broker_manager.shutdown());

    if let Err(e) = ipc_result {
        error!(error = %e, "Failed to shutdown IPC server gracefully");
    }

    if let Err(e) = broker_result {
        error!(error = %e, "Failed to shutdown embedded broker gracefully");
    }

    #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
    {
        println!("daemoneye-agent shutdown complete.")
    };
    Ok(())
}
