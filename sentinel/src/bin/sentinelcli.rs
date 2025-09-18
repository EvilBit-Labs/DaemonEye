// =============================================================================
// BUNDLED SENTINELCLI BINARY
// =============================================================================
// This file contains the bundled sentinelcli binary for distribution.
// It re-exports the sentinelcli functionality while allowing the sentinel package
// to have its own dependencies and versioning.
//
// BUNDLING STRATEGY:
// - This is a copy of the implementation from sentinelcli/src/main.rs
// - It's maintained separately to allow the sentinel package to have its own dependencies
// - Changes should be synchronized between this file and the original implementation
// - This allows users to install all components with `cargo install sentinel`
//
// SENTINELCLI ROLE:
// - Command-line interface for operators and security analysts
// - No network access, read-only database operations
// - JSON/table output with color handling and shell completions
// - Query interface for process data, alerts, and system information
// - Export functionality for data analysis and reporting
//
// FEATURES:
// - Interactive and non-interactive query modes
// - Multiple output formats (JSON, table, CSV)
// - Advanced filtering and search capabilities
// - Real-time monitoring and alert viewing
// - Configuration management and validation
// =============================================================================

use clap::Parser;
use sentinel_lib::{config, storage, telemetry};

#[derive(Parser)]
#[command(name = "sentinelcli")]
#[command(about = "SentinelD CLI interface")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/sentineld/processes.db")]
    database: String,

    /// Output format
    #[arg(short, long, default_value = "human")]
    format: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let cli = Cli::parse();

    // Load configuration
    let config_loader = config::ConfigLoader::new("sentinelcli");
    let _config = config_loader.load_blocking()?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("sentinelcli".to_string());

    // Initialize database
    let db_manager = storage::DatabaseManager::new(&_config.database.path)?;

    // Get database statistics
    let stats = db_manager.get_stats()?;

    // Output based on format
    match cli.format.as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        "human" => {
            println!("SentinelD Database Statistics");
            println!("============================");
            println!("Processes: {}", stats.process_count);
            println!("Rules: {}", stats.rule_count);
            println!("Alerts: {}", stats.alert_count);
            println!("System Info: {}", stats.system_info_count);
            println!("Scans: {}", stats.scan_count);
        }
        _ => {
            eprintln!("Unknown format: {}", cli.format);
            std::process::exit(1);
        }
    }

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("database_query".to_string());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check_blocking()?;
    println!("Health status: {}", health_check.status);

    println!("sentinelcli completed successfully");
    Ok(())
}
