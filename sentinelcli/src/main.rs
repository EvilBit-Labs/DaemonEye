use clap::{Arg, Command};
use sentinel_lib::{config, storage, telemetry};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let matches = Command::new("sentinelcli")
        .about("SentinelD CLI interface")
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            Arg::new("database")
                .short('d')
                .long("database")
                .value_name("PATH")
                .help("Database path")
                .default_value("/var/lib/sentineld/processes.db"),
        )
        .arg(
            Arg::new("format")
                .short('f')
                .long("format")
                .value_name("FORMAT")
                .help("Output format")
                .default_value("human"),
        )
        .get_matches();

    let _database = matches.get_one::<String>("database").map_or_else(
        || "/var/lib/sentineld/processes.db".to_owned(),
        Clone::clone,
    );
    let format = matches
        .get_one::<String>("format")
        .map_or_else(|| "human".to_owned(), Clone::clone);

    // Load configuration
    let config_loader = config::ConfigLoader::new("sentinelcli");
    let config = config_loader.load_blocking()?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("sentinelcli".to_owned());

    // Initialize database
    let db_manager = storage::DatabaseManager::new(&config.database.path)?;

    // Get database statistics
    let stats = db_manager.get_stats()?;

    // Output based on format
    match format.as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        "human" => {
            println!("SentinelD Database Statistics");
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

    println!("sentinelcli completed successfully");
    Ok(())
}
