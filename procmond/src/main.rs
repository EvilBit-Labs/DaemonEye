#![forbid(unsafe_code)]

use clap::Parser;
use collector_core::{CollectionEvent, Collector, CollectorConfig, CollectorRegistrationConfig};
use daemoneye_lib::{config, storage, telemetry};
use procmond::{
    ProcessEventSource, ProcessSourceConfig,
    event_bus_connector::EventBusConnector,
    monitor_collector::{ProcmondMonitorCollector, ProcmondMonitorConfig},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

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

    // Check for broker configuration via environment variable
    // DAEMONEYE_BROKER_SOCKET: If set, use actor mode with EventBusConnector
    // If not set, use standalone mode with collector-core
    let broker_socket = std::env::var("DAEMONEYE_BROKER_SOCKET").ok();

    if let Some(ref socket_path) = broker_socket {
        // ========================================================================
        // Actor Mode: Use ProcmondMonitorCollector with EventBusConnector
        // ========================================================================
        info!(
            socket_path = %socket_path,
            "Broker socket configured, starting in actor mode"
        );

        // Create actor channel (bounded, capacity: 100)
        let (actor_handle, message_receiver) = ProcmondMonitorCollector::create_channel();

        // Create ProcmondMonitorConfig from CLI arguments
        let monitor_config = ProcmondMonitorConfig {
            base_config: collector_core::MonitorCollectorConfig {
                collection_interval: Duration::from_secs(cli.interval),
                ..Default::default()
            },
            process_config: procmond::process_collector::ProcessCollectionConfig {
                collect_enhanced_metadata: cli.enhanced_metadata,
                max_processes: cli.max_processes,
                compute_executable_hashes: cli.compute_hashes,
                ..Default::default()
            },
            ..Default::default()
        };

        // Create the actor-based collector
        let mut collector = ProcmondMonitorCollector::new(
            Arc::clone(&db_manager),
            monitor_config,
            message_receiver,
        )?;

        // Initialize EventBusConnector with WAL directory
        let wal_dir = PathBuf::from(&cli.database).parent().map_or_else(
            || PathBuf::from("/var/lib/daemoneye/wal"),
            |p| p.join("wal"),
        );

        // Ensure WAL directory exists
        if let Err(e) = std::fs::create_dir_all(&wal_dir) {
            warn!(
                wal_dir = ?wal_dir,
                error = %e,
                "Failed to create WAL directory, continuing anyway"
            );
        }

        let mut event_bus_connector = EventBusConnector::new(wal_dir).await?;

        // Attempt to connect to the broker
        match event_bus_connector.connect().await {
            Ok(()) => {
                info!("Connected to daemoneye-agent broker");

                // Replay any events from WAL (crash recovery)
                match event_bus_connector.replay_wal().await {
                    Ok(replayed) if replayed > 0 => {
                        info!(
                            replayed = replayed,
                            "Replayed events from WAL after connection"
                        );
                    }
                    Ok(_) => {
                        info!("No events to replay from WAL");
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to replay WAL, some events may be delayed");
                    }
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to connect to broker, will buffer events until connection available"
                );
            }
        }

        // Take the backpressure receiver before moving connector to collector
        let backpressure_rx = event_bus_connector.take_backpressure_receiver();

        // Set the EventBusConnector on the collector
        collector.set_event_bus_connector(event_bus_connector);

        // Spawn backpressure monitor task if we have the receiver
        let original_interval = Duration::from_secs(cli.interval);
        let _backpressure_task = backpressure_rx.map_or_else(
            || {
                warn!("Backpressure receiver not available, dynamic interval adjustment disabled");
                None
            },
            |bp_rx| {
                Some(ProcmondMonitorCollector::spawn_backpressure_monitor(
                    actor_handle.clone(),
                    bp_rx,
                    original_interval,
                ))
            },
        );

        // Create event channel for the actor's output
        let (event_tx, mut event_rx) = mpsc::channel::<CollectionEvent>(1000);

        // Clone handle for shutdown task
        let shutdown_handle = actor_handle.clone();

        // Spawn task to handle graceful shutdown on Ctrl+C
        let shutdown_task = tokio::spawn(async move {
            // Wait for Ctrl+C
            if let Err(e) = tokio::signal::ctrl_c().await {
                error!(error = %e, "Failed to listen for Ctrl+C signal");
                return;
            }
            info!("Received Ctrl+C, initiating graceful shutdown");

            // Send graceful shutdown to actor
            match shutdown_handle.graceful_shutdown().await {
                Ok(()) => info!("Actor shutdown completed successfully"),
                Err(e) => error!(error = %e, "Actor shutdown failed"),
            }
        });

        // Startup coordination: Wait for "begin monitoring" command from agent
        // For now, if the broker is configured, we assume the agent will send
        // the begin monitoring command. In a complete implementation, this would
        // subscribe to control.collector.lifecycle topic.
        //
        // TODO: Implement subscription to control.collector.lifecycle topic
        // and wait for "begin monitoring" broadcast from agent.
        //
        // For now, send BeginMonitoring immediately to start collection.
        // This allows testing without requiring the full agent infrastructure.
        info!("Starting collection immediately (TODO: wait for agent 'begin monitoring' command)");
        if let Err(e) = actor_handle.begin_monitoring() {
            error!(error = %e, "Failed to send BeginMonitoring command");
        }

        // Spawn the actor task
        let actor_task = tokio::spawn(async move {
            if let Err(e) = collector.run(event_tx).await {
                error!(error = %e, "Actor run loop failed");
            }
        });

        // Spawn task to consume events from the actor (logging only for now)
        let event_consumer_task = tokio::spawn(async move {
            let mut event_count = 0_u64;
            while let Some(event) = event_rx.recv().await {
                event_count = event_count.saturating_add(1);
                if event_count.is_multiple_of(100) {
                    info!(total_events = event_count, "Processing collection events");
                }
                // In a full implementation, events would be sent to downstream processors
                match event {
                    CollectionEvent::Process(pe) => {
                        tracing::trace!(pid = pe.pid, name = %pe.name, "Received process event");
                    }
                    CollectionEvent::Network(_)
                    | CollectionEvent::Filesystem(_)
                    | CollectionEvent::Performance(_)
                    | CollectionEvent::TriggerRequest(_) => {
                        tracing::trace!("Received non-process event");
                    }
                }
            }
            info!(total_events = event_count, "Event consumer task exiting");
        });

        // Wait for actor to complete (either by shutdown or error)
        tokio::select! {
            result = actor_task => {
                if let Err(e) = result {
                    error!(error = %e, "Actor task panicked");
                }
            }
            _ = shutdown_task => {
                info!("Shutdown task completed");
            }
        }

        // Clean up event consumer
        event_consumer_task.abort();

        info!("Procmond actor mode shutdown complete");
    } else {
        // ========================================================================
        // Standalone Mode: Use ProcessEventSource with collector-core
        // ========================================================================
        info!("No broker socket configured, starting in standalone mode");

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

        // Enable broker registration for RPC service (if broker becomes available)
        collector_config.registration = Some(CollectorRegistrationConfig {
            enabled: true,
            broker: None,
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

        // Log RPC service status
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

        // Run the collector (handles IPC, event processing, and lifecycle management)
        collector.run().await?;
    }

    Ok(())
}
