#![forbid(unsafe_code)]

use clap::Parser;
use daemoneye_lib::{alerting, config, detection, models, storage, telemetry};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

mod ipc_client;
use ipc_client::{IpcClientManager, create_default_ipc_config};

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
        eprintln!("Error: {}", e);
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
        println!("daemoneye-agent started successfully");
        return Ok(());
    }

    // Load configuration
    let config_loader = config::ConfigLoader::new("daemoneye-agent");
    let mut config = config_loader.load()?;

    // Override database path from CLI argument if provided
    config.database.path = cli.database.into();

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("daemoneye-agent".to_string());

    // Initialize database
    let _db_manager = storage::DatabaseManager::new(&config.database.path)?;

    // Initialize IPC client for communication with procmond
    let ipc_config = create_default_ipc_config();
    let mut ipc_manager = IpcClientManager::new(ipc_config)?;

    // Wait for procmond to become available
    info!("Waiting for procmond to become available...");
    if let Err(e) = ipc_manager.wait_for_procmond(Duration::from_secs(30)).await {
        warn!(
            "Procmond not available: {}. Continuing without process monitoring.",
            e
        );
    } else {
        info!("Connected to procmond successfully");
    }

    // Initialize detection engine
    let mut detection_engine = detection::DetectionEngine::new();

    // Create a sample detection rule
    let rule = models::DetectionRule::new(
        "rule-1".to_string(),
        "Test Rule".to_string(),
        "Test detection rule".to_string(),
        "SELECT * FROM processes WHERE name = 'test'".to_string(),
        "test".to_string(),
        models::AlertSeverity::Medium,
    );

    // Load the rule
    detection_engine.load_rule(rule)?;

    // Initialize alert manager
    let mut alert_manager = alerting::AlertManager::new();
    let stdout_sink = Box::new(alerting::StdoutSink::new(
        "stdout".to_string(),
        alerting::OutputFormat::Json,
    ));
    alert_manager.add_sink(stdout_sink);

    // Indicate startup success before entering main loop
    println!("daemoneye-agent started successfully");

    // Main collection loop using IPC client
    let scan_interval = Duration::from_millis(config.app.scan_interval_ms);
    info!(
        interval_ms = config.app.scan_interval_ms,
        "Entering main collection+detection loop with IPC client"
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
    let mut alert_manager = alert_manager; // mutable for sink operations
    let mut iteration: u64 = 0;

    tokio::pin!(shutdown_signal);

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                info!("Shutdown signal received; commencing graceful shutdown");
                break;
            }
            _ = tokio::time::sleep(scan_interval) => {
                iteration += 1;
                let loop_start = Instant::now();

                // Request process enumeration from procmond via IPC
                let processes = match ipc_manager.enumerate_processes().await {
                    Ok(result) => {
                        if result.success {
                            info!(
                                process_count = result.processes.len(),
                                "Successfully collected process data from procmond"
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
                                "Procmond returned error during process enumeration"
                            );
                            Vec::new()
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to collect processes from procmond");
                        // Check if we should try to reconnect
                        if !ipc_manager.is_healthy().await {
                            warn!("IPC client is unhealthy, attempting reconnection");
                            if let Err(reconnect_err) = ipc_manager.force_reconnect().await {
                                error!(error = %reconnect_err, "Failed to reconnect to procmond");
                            }
                        }
                        Vec::new()
                    }
                };

                // Execute detection rules against collected processes
                let detection_timer = telemetry::PerformanceTimer::start("detection_execution".to_string());
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
                if iteration % 10 == 0 { // periodic health check every 10 iterations
                    let h = telemetry.health_check();
                    info!(status=%h.status, "Telemetry health check");
                }
                let loop_elapsed = loop_start.elapsed();
                if loop_elapsed > scan_interval { warn!(elapsed_ms = loop_elapsed.as_millis() as u64, "Loop overran scan interval"); }
            }
        }
    }

    println!("daemoneye-agent shutdown complete.");
    Ok(())
}
