use futures::StreamExt;
use sentinel_lib::{
    alerting,
    collection::{ProcessCollectionService, SysinfoProcessCollector},
    config, detection, models, storage, telemetry,
};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config_loader = config::ConfigLoader::new("sentinelagent");
    let config = config_loader.load().await?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("sentinelagent".to_string());

    // Initialize database
    let _db_manager = storage::DatabaseManager::new(&config.database.path)?;

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
    println!("sentinelagent started successfully");

    // Test mode: exit early to keep existing integration test semantics (set SENTINELAGENT_TEST_MODE=1)
    if std::env::var("SENTINELAGENT_TEST_MODE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(());
    }

    // Collector for real process data (streaming)
    let process_collector = SysinfoProcessCollector::new();
    let scan_interval = Duration::from_millis(config.app.scan_interval_ms);
    info!(
        interval_ms = config.app.scan_interval_ms,
        "Entering main collection+detection loop"
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
                // Establish a soft deadline for collection (75% of interval) to avoid overlap
                let collection_deadline = Some(Instant::now() + (scan_interval * 3 / 4));
                let mut stream = process_collector.stream_processes(collection_deadline);
                let mut processes: Vec<models::ProcessRecord> = Vec::new();
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(proc_record) => {
                            processes.push(proc_record);
                            if let Some(max) = config.app.max_processes { if processes.len() >= max { break; } }
                        },
                        Err(e) => {
                            match e {
                                sentinel_lib::collection::CollectionError::Timeout => { warn!("Process collection timed out; continuing with partial data"); break; },
                                other => { warn!(error=?other, "Process collection item error"); }
                            }
                        }
                    }
                }

                // Execute detection rules against collected processes
                let detection_timer = telemetry::PerformanceTimer::start("detection_execution".to_string());
                match detection_engine.execute_rules(&processes).await {
                    Ok(alerts) => {
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
                    }
                    Err(e) => {
                        error!(error=?e, "Detection execution failed");
                        telemetry.record_error();
                    }
                }

                // Update telemetry with rough resource usage snapshot (placeholder zeros for now)
                telemetry.update_resource_usage(0.0, 0);
                if iteration % 10 == 0 { // periodic health check every 10 iterations
                    match telemetry.health_check().await {
                        Ok(h) => info!(status=%h.status, "Telemetry health check"),
                        Err(e) => warn!(error=?e, "Telemetry health check failed"),
                    }
                }
                let loop_elapsed = loop_start.elapsed();
                if loop_elapsed > scan_interval { warn!(elapsed_ms = loop_elapsed.as_millis() as u64, "Loop overran scan interval"); }
            }
        }
    }

    println!("sentinelagent shutdown complete.");
    Ok(())
}
