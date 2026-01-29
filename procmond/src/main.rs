#![forbid(unsafe_code)]

use clap::Parser;
use collector_core::{Collector, CollectorConfig, CollectorRegistrationConfig};
use daemoneye_lib::{config, storage, telemetry};
use procmond::{ProcessEventSource, ProcessSourceConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

/// Parse and validate the collection interval argument.
///
/// Ensures the interval is within acceptable bounds (5-3600 seconds).
/// Returns a clear error message if validation fails.
fn parse_interval(s: &str) -> Result<u64, String> {
    let interval: u64 = s
        .parse()
        .map_err(|_parse_err| format!("Invalid interval '{s}': must be a number"))?;

    if interval < 5 {
        Err(format!(
            "Interval too small: {interval} seconds. Minimum allowed is 5 seconds"
        ))
    } else if interval > 3600 {
        Err(format!(
            "Interval too large: {interval} seconds. Maximum allowed is 3600 seconds (1 hour)"
        ))
    } else {
        Ok(interval)
    }
}

#[derive(Parser)]
#[command(name = "procmond")]
#[command(about = "DaemonEye Process Monitoring Daemon")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/daemoneye/processes.db")]
    database: String,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Collection interval in seconds (minimum: 5, maximum: 3600)
    #[arg(short, long, default_value = "30", value_parser = parse_interval)]
    interval: u64,

    /// Maximum processes to collect per cycle (0 = unlimited)
    #[arg(long, default_value = "0")]
    max_processes: usize,

    /// Enable enhanced metadata collection
    #[arg(long)]
    enhanced_metadata: bool,

    /// Enable executable hashing
    #[arg(long)]
    compute_hashes: bool,
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments first - this will handle --help and --version automatically
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config_loader = config::ConfigLoader::new("procmond");
    let _config = config_loader.load()?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("procmond".to_owned());

    // Initialize database
    let db_manager = Arc::new(Mutex::new(storage::DatabaseManager::new(&cli.database)?));

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("process_collection".to_owned());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check();
    info!(status = %health_check.status, "Health check completed");

    // Get database statistics
    let stats = db_manager.lock().await.get_stats()?;
    info!(
        processes = stats.processes,
        rules = stats.rules,
        alerts = stats.alerts,
        "Database stats retrieved"
    );

    // Create collector configuration
    let mut collector_config = CollectorConfig::new()
        .with_component_name("procmond".to_owned())
        .with_ipc_endpoint(daemoneye_lib::ipc::IpcConfig::default().endpoint_path)
        .with_max_event_sources(1)
        .with_event_buffer_size(1000)
        .with_shutdown_timeout(Duration::from_secs(30))
        .with_health_check_interval(Duration::from_secs(60))
        .with_telemetry(true)
        .with_debug_logging(cli.log_level == "debug");

    // Enable broker registration for RPC service
    // Note: In a real deployment, the broker would be provided via configuration
    // For now, we'll configure registration but it will only work if a broker is available
    collector_config.registration = Some(CollectorRegistrationConfig {
        enabled: true,
        broker: None, // Will be set if broker is available via environment/config
        collector_id: Some("procmond".to_owned()),
        collector_type: Some("procmond".to_owned()),
        topic: "control.collector.registration".to_owned(),
        timeout: Duration::from_secs(10),
        retry_attempts: 3,
        heartbeat_interval: Duration::from_secs(30),
        attributes: HashMap::new(),
    });

    // Create process source configuration
    let process_config = ProcessSourceConfig {
        collection_interval: Duration::from_secs(cli.interval),
        collect_enhanced_metadata: cli.enhanced_metadata,
        max_processes_per_cycle: cli.max_processes,
        compute_executable_hashes: cli.compute_hashes,
        ..Default::default()
    };

    // Create process event source
    let process_source = ProcessEventSource::with_config(db_manager, process_config);

    // Log RPC service status before moving collector_config
    // The RPC service will be automatically started by collector-core after broker registration
    let registration_enabled = collector_config
        .registration
        .as_ref()
        .is_some_and(|r| r.enabled);
    let collector_id_str = collector_config
        .registration
        .as_ref()
        .and_then(|r| r.collector_id.as_deref())
        .unwrap_or("procmond");

    if registration_enabled {
        info!(
            collector_id = %collector_id_str,
            "RPC service will be initialized after broker registration"
        );
    }

    // Create and configure collector
    let mut collector = Collector::new(collector_config);
    collector.register(Box::new(process_source))?;

    // Run the collector (this will handle IPC, event processing, and lifecycle management)
    collector.run().await?;

    Ok(())
}
