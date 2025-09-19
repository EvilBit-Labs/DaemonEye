#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_lib::{config, models, storage, telemetry};
use std::sync::Arc;
use tokio::sync::Mutex;

mod ipc;

use ipc::error::IpcError;
use ipc::server::IpcServer;
use ipc::{IpcConfig, SimpleMessageHandler, create_ipc_server};
use sentinel_lib::proto::{DetectionResult, DetectionTask, ProtoProcessRecord, ProtoTaskType};

/// Message handler for IPC communication with process monitoring
#[allow(dead_code)]
struct ProcessMessageHandler {
    database: Arc<Mutex<storage::DatabaseManager>>,
}

impl ProcessMessageHandler {
    async fn handle_detection_task(
        &self,
        task: DetectionTask,
    ) -> Result<DetectionResult, IpcError> {
        tracing::info!("Received detection task: {}", task.task_id);

        match task.task_type {
            task_type if task_type == ProtoTaskType::EnumerateProcesses as i32 => {
                // Simulate process enumeration
                let processes = vec![ProtoProcessRecord {
                    pid: 1234,
                    ppid: Some(1),
                    name: "test-process".to_string(),
                    executable_path: Some("/usr/bin/test".to_string()),
                    command_line: vec!["test".to_string(), "--arg".to_string()],
                    start_time: Some(chrono::Utc::now().timestamp()),
                    cpu_usage: Some(25.5),
                    memory_usage: Some(1024 * 1024),
                    executable_hash: Some("abc123def456".to_string()),
                    hash_algorithm: Some("sha256".to_string()),
                    user_id: Some("1000".to_string()),
                    accessible: true,
                    file_exists: true,
                    collection_time: chrono::Utc::now().timestamp_millis(),
                }];

                Ok(DetectionResult::success(&task.task_id, processes))
            }
            _ => {
                tracing::warn!("Unsupported task type: {}", task.task_type);
                Ok(DetectionResult::failure(
                    &task.task_id,
                    "Unsupported task type",
                ))
            }
        }
    }
}

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
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments first - this will handle --help and --version automatically
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config_loader = config::ConfigLoader::new("procmond");
    let _config = config_loader.load().await?;

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("procmond".to_string());

    // Initialize database
    let db_manager = Arc::new(Mutex::new(storage::DatabaseManager::new(&cli.database)?));

    // Create a sample process record
    let process = models::ProcessRecord::new(1234, "procmond".to_string());

    // Store the process record
    db_manager.lock().await.store_process(1, &process)?;

    // Record operation in telemetry
    let timer = telemetry::PerformanceTimer::start("process_collection".to_string());
    let duration = timer.finish();
    telemetry.record_operation(duration);

    // Perform health check
    let health_check = telemetry.health_check().await?;
    println!("Health status: {}", health_check.status);

    // Get database statistics
    let stats = db_manager.lock().await.get_stats()?;
    println!("Database stats: {:?}", stats);

    // Initialize and start IPC server
    let ipc_server = initialize_ipc_server(&db_manager).await?;

    // Keep the server running until shutdown signal
    run_ipc_server(ipc_server).await?;

    Ok(())
}

/// Initialize the IPC server with proper configuration
async fn initialize_ipc_server(
    db_manager: &Arc<Mutex<storage::DatabaseManager>>,
) -> Result<Box<dyn IpcServer>, IpcError> {
    let ipc_config = IpcConfig {
        path: "/tmp/sentineld-procmond.sock".to_string(),
        max_connections: 10,
        connection_timeout_secs: 30,
        message_timeout_secs: 60,
    };

    let mut ipc_server = create_ipc_server(ipc_config)?;

    // Create and set the message handler
    let process_handler = Arc::new(ProcessMessageHandler {
        database: Arc::clone(db_manager),
    });
    let handler = SimpleMessageHandler::new("ProcessMessageHandler".to_string(), move |task| {
        let handler = Arc::clone(&process_handler);
        async move { handler.handle_detection_task(task).await }
    });
    ipc_server.set_handler(handler);

    // Start the IPC server
    ipc_server.start().await?;
    println!("IPC server started successfully");

    Ok(Box::new(ipc_server))
}

/// Run the IPC server until shutdown signal
async fn run_ipc_server(mut ipc_server: Box<dyn IpcServer>) -> Result<(), IpcError> {
    // Keep the server running until shutdown signal
    tokio::signal::ctrl_c().await?;
    println!("Shutdown signal received, stopping IPC server...");

    // Stop the IPC server
    ipc_server.stop().await?;
    println!("procmond stopped successfully");

    Ok(())
}
