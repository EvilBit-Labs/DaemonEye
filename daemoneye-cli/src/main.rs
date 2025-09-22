#![forbid(unsafe_code)]

use clap::Parser;
use daemoneye_lib::{config, storage, telemetry};

/// DaemonEye CLI interface
#[derive(Parser)]
#[command(name = "daemoneye-cli")]
#[command(about = "DaemonEye CLI interface")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short = 'd', long = "database", value_name = "PATH")]
    #[arg(default_value = "/var/lib/daemoneye/processes.db")]
    database: String,

    /// Output format
    #[arg(short = 'f', long = "format", value_name = "FORMAT")]
    #[arg(default_value = "human")]
    format: String,
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let cli = Cli::parse();

    let database_path = cli.database;
    let format = cli.format;

    // Load configuration
    let config_loader = config::ConfigLoader::new("daemoneye-cli");
    let _config = config_loader.load_blocking()?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("daemoneye-cli".to_owned());

    // Initialize database using the provided path
    let db_manager = storage::DatabaseManager::new(&database_path)?;

    // Get database statistics
    let stats = db_manager.get_stats()?;

    // Output based on format
    match format.as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        "human" => {
            println!("DaemonEye Database Statistics");
            println!("============================");
            println!("Processes: {}", stats.processes);
            println!("Rules: {}", stats.rules);
            println!("Alerts: {}", stats.alerts);
            println!("System Info: {}", stats.system_info);
            println!("Scans: {}", stats.scans);
        }
        _ => {
            eprintln!("Unknown format: {format}");
            std::process::exit(1);
        }
    }

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("database_query".to_owned());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check_blocking();
    println!("Health status: {}", health_check.status);

    println!("daemoneye-cli completed successfully");
    Ok(())
}
