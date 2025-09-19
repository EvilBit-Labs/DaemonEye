#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_lib::{config, storage, telemetry};
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::Mutex;

mod ipc;

use ipc::error::IpcError;
use ipc::{IpcConfig, create_ipc_server};
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
                self.enumerate_processes(&task).await
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

    /// Enumerate all processes on the system
    async fn enumerate_processes(&self, task: &DetectionTask) -> Result<DetectionResult, IpcError> {
        let mut system = System::new_all();
        system.refresh_all();

        let processes: Vec<ProtoProcessRecord> = system
            .processes()
            .iter()
            .map(|(pid, process)| self.convert_process_to_record(pid, process))
            .collect();

        Ok(DetectionResult::success(&task.task_id, processes))
    }

    /// Convert a sysinfo process to a ProtoProcessRecord
    fn convert_process_to_record(
        &self,
        pid: &sysinfo::Pid,
        process: &sysinfo::Process,
    ) -> ProtoProcessRecord {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(|p| p.as_u32());
        let name = process.name().to_string_lossy().to_string();
        let executable_path = process.exe().map(|path| path.to_string_lossy().to_string());
        let command_line = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        let start_time = Some(process.start_time() as i64);
        let cpu_usage = Some(process.cpu_usage() as f64);
        let memory_usage = Some(process.memory() * 1024);
        let executable_hash = None; // Would need file hashing implementation
        let hash_algorithm = None;
        let user_id = process.user_id().map(|uid| uid.to_string());
        let accessible = true; // Process is accessible if we can enumerate it
        let file_exists = executable_path.is_some();
        let collection_time = chrono::Utc::now().timestamp_millis();

        ProtoProcessRecord {
            pid: pid_u32,
            ppid,
            name,
            executable_path,
            command_line,
            start_time,
            cpu_usage,
            memory_usage,
            executable_hash,
            hash_algorithm,
            user_id,
            accessible,
            file_exists,
            collection_time,
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

    /// IPC socket path
    #[arg(short, long, default_value = "/tmp/sentineld-procmond.sock")]
    socket: String,
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
    let ipc_server = initialize_ipc_server(&db_manager, &cli.socket).await?;

    // Keep the server running until shutdown signal
    run_ipc_server(ipc_server).await?;

    Ok(())
}

/// Initialize the IPC server with proper configuration
async fn initialize_ipc_server(
    db_manager: &Arc<Mutex<storage::DatabaseManager>>,
    socket_path: &str,
) -> Result<sentinel_lib::ipc::InterprocessServer, IpcError> {
    let ipc_config = IpcConfig {
        path: socket_path.to_string(),
        max_connections: 10,
        connection_timeout_secs: 30,
        message_timeout_secs: 60,
    };

    let mut ipc_server = create_ipc_server(ipc_config)?;

    // Create and set the message handler
    let process_handler = Arc::new(ProcessMessageHandler {
        database: Arc::clone(db_manager),
    });
    ipc_server.set_handler(move |task: DetectionTask| {
        let handler = Arc::clone(&process_handler);
        async move {
            let result = handler.handle_detection_task(task).await;
            result.map_err(|e| {
                tracing::error!("IPC handler error: {}", e);
                sentinel_lib::ipc::IpcError::Encode(format!("Handler error: {}", e))
            })
        }
    });

    // Start the IPC server
    ipc_server.start().await?;
    println!("IPC server started successfully");

    Ok(ipc_server)
}

/// Run the IPC server until shutdown signal
async fn run_ipc_server(
    mut ipc_server: sentinel_lib::ipc::InterprocessServer,
) -> Result<(), IpcError> {
    // Keep the server running until shutdown signal
    tokio::signal::ctrl_c().await?;
    println!("Shutdown signal received, stopping IPC server...");

    // Stop the IPC server
    ipc_server.stop().await?;
    println!("procmond stopped successfully");

    Ok(())
}
