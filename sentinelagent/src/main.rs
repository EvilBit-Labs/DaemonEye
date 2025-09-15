use sentinel_lib::{alerting, config, detection, models, storage, telemetry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Create sample process data
    let processes = vec![
        models::ProcessRecord::new(1234, "test-process".to_string()),
        models::ProcessRecord::new(5678, "another-process".to_string()),
    ];

    // Execute detection rules
    let alerts = detection_engine.execute_rules(&processes).await?;
    println!("Generated {} alerts", alerts.len());

    // Send alerts
    for alert in &alerts {
        let results = alert_manager.send_alert(alert).await?;
        println!("Sent alert to {} sinks", results.len());
    }

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("detection_execution".to_string());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check().await?;
    println!("Health status: {}", health_check.status);

    println!("sentinelagent started successfully");
    Ok(())
}
