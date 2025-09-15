use sentinel_lib::{config, models, storage, telemetry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
