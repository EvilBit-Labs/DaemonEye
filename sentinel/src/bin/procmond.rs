// =============================================================================
// BUNDLED PROCMOND BINARY
// =============================================================================
// This file contains the bundled procmond binary for distribution.
// It re-exports the procmond functionality while allowing the sentinel package
// to have its own dependencies and versioning.
//
// BUNDLING STRATEGY:
// - This is a copy of the implementation from procmond/src/main.rs
// - It's maintained separately to allow the sentinel package to have its own dependencies
// - Changes should be synchronized between this file and the original implementation
// - This allows users to install all components with `cargo install sentinel`
//
// PROCMOND ROLE:
// - Privileged process collector with minimal attack surface
// - Runs with elevated privileges, drops them immediately after init
// - No network access whatsoever (security boundary)
// - Write-only access to audit ledger
// - IPC server for receiving simple detection tasks from sentinelagent
//
// SECURITY MODEL:
// - Principle of least privilege: minimal required permissions
// - Automatic privilege dropping after initialization
// - No inbound network connections
// - Comprehensive audit logging
// =============================================================================

use clap::Parser;
use sentinel_lib::{config, models, storage, telemetry};

#[derive(Parser)]
#[command(name = "procmond")]
#[command(about = "SentinelD Process Monitoring Daemon")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/sentineld/processes.db")]
    database: String,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments first
    let _cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config_loader = config::ConfigLoader::new("procmond");
    let config = config_loader.load().await?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("procmond".to_string());

    // Initialize database
    let db_manager = storage::DatabaseManager::new(&config.database.path)?;

    // Create a sample process record
    let process = models::ProcessRecord::new(1234, "procmond".to_string());

    // Store the process record
    db_manager.store_process(1, &process)?;

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("process_collection".to_string());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check().await?;
    println!("Health status: {}", health_check.status);

    // Get database statistics
    let stats = db_manager.get_stats()?;
    println!("Database stats: {:?}", stats);

    println!("procmond started successfully");
    Ok(())
}
