#![forbid(unsafe_code)]

use clap::Parser;
use collector_core::{Collector, CollectorConfig};
use daemoneye_lib::{config, storage, telemetry};
use procmond::{ProcessEventSource, ProcessSourceConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Parse and validate the collection interval argument.
///
/// Ensures the interval is within acceptable bounds (5-3600 seconds).
/// Returns a clear error message if validation fails.
fn parse_interval(s: &str) -> Result<u64, String> {
    let interval: u64 = s
        .parse()
        .map_err(|_| format!("Invalid interval '{}': must be a number", s))?;

    if interval < 5 {
        Err(format!(
            "Interval too small: {} seconds. Minimum allowed is 5 seconds",
            interval
        ))
    } else if interval > 3600 {
        Err(format!(
            "Interval too large: {} seconds. Maximum allowed is 3600 seconds (1 hour)",
            interval
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
    let mut telemetry = telemetry::TelemetryCollector::new("procmond".to_string());

    // Initialize database
    let db_manager = Arc::new(Mutex::new(storage::DatabaseManager::new(&cli.database)?));

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("process_collection".to_string());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check();
    println!("Health status: {}", health_check.status);

    // Get database statistics
    let stats = db_manager.lock().await.get_stats()?;
    println!("Database stats: {:?}", stats);

    // Create collector configuration
    let collector_config = CollectorConfig::new()
        .with_component_name("procmond".to_string())
        .with_max_event_sources(1)
        .with_event_buffer_size(1000)
        .with_shutdown_timeout(Duration::from_secs(30))
        .with_health_check_interval(Duration::from_secs(60))
        .with_telemetry(true)
        .with_debug_logging(cli.log_level == "debug");

    // Create process source configuration
    let process_config = ProcessSourceConfig {
        collection_interval: Duration::from_secs(cli.interval),
        collect_enhanced_metadata: cli.enhanced_metadata,
        max_processes_per_cycle: cli.max_processes,
        compute_executable_hashes: cli.compute_hashes,
    };

    // Create process event source
    let process_source = ProcessEventSource::with_config(db_manager, process_config);

    // Create and configure collector
    let mut collector = Collector::new(collector_config);
    collector.register(Box::new(process_source))?;

    // Run the collector (this will handle IPC, event processing, and lifecycle management)
    collector.run().await?;

    Ok(())
}
