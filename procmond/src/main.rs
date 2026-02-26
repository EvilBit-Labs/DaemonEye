#![forbid(unsafe_code)]

use clap::Parser;
use collector_core::{CollectionEvent, Collector, CollectorConfig, CollectorRegistrationConfig};
use daemoneye_lib::{config, storage, telemetry};
use procmond::{
    ProcessEventSource, ProcessSourceConfig,
    event_bus_connector::EventBusConnector,
    monitor_collector::{ProcmondMonitorCollector, ProcmondMonitorConfig},
    registration::{RegistrationConfig, RegistrationManager, RegistrationState},
    rpc_service::{RpcServiceConfig, RpcServiceHandler},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error, info, warn};

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

        // Ensure WAL directory exists (fail-fast to avoid confusing WAL init failures)
        std::fs::create_dir_all(&wal_dir).map_err(|e| {
            error!(
                wal_dir = ?wal_dir,
                error = %e,
                "Failed to create WAL directory"
            );
            e
        })?;

        let mut event_bus_connector = EventBusConnector::new(wal_dir.clone()).await?;

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

        // Wrap EventBusConnector in Arc<RwLock<>> for sharing between components
        // Note: backpressure receiver is taken from collector_event_bus below, not here
        let event_bus = Arc::new(RwLock::new(event_bus_connector));

        // ========================================================================
        // Initialize Registration Manager
        // ========================================================================
        let registration_config = RegistrationConfig::default();
        let registration_manager = Arc::new(RegistrationManager::new(
            Arc::clone(&event_bus),
            actor_handle.clone(),
            registration_config,
        ));

        info!(
            collector_id = %registration_manager.collector_id(),
            "Registration manager initialized"
        );

        // Perform registration with daemoneye-agent
        info!("Registering with daemoneye-agent");
        match registration_manager.register().await {
            Ok(response) => {
                info!(
                    collector_id = %response.collector_id,
                    heartbeat_interval_ms = response.heartbeat_interval_ms,
                    assigned_topics = ?response.assigned_topics,
                    "Registration successful"
                );
            }
            Err(e) => {
                // Log warning but continue - procmond can operate without registration
                // in standalone/development scenarios
                warn!(
                    error = %e,
                    "Registration failed, continuing in standalone mode"
                );
            }
        }

        // Start heartbeat task (only publishes when registered)
        let heartbeat_task =
            RegistrationManager::spawn_heartbeat_task(Arc::clone(&registration_manager));
        info!("Heartbeat task started");

        // ========================================================================
        // Initialize RPC Service Handler
        // ========================================================================
        let rpc_config = RpcServiceConfig::default();
        let rpc_service =
            RpcServiceHandler::new(actor_handle.clone(), Arc::clone(&event_bus), rpc_config);

        info!(
            control_topic = %rpc_service.config().control_topic,
            "RPC service handler initialized"
        );

        // Create a separate EventBusConnector for the collector with its own WAL directory
        // to avoid conflicts with the shared event_bus connector.
        // Note: The collector takes ownership of its connector, while the registration
        // and RPC services share a separate connector for control messages.
        // TODO: Refactor to share the connector more elegantly when EventBusConnector
        // supports both ProcessEvent and generic message publishing.
        let collector_wal_dir = wal_dir.join("collector");
        std::fs::create_dir_all(&collector_wal_dir).map_err(|e| {
            error!(
                wal_dir = ?collector_wal_dir,
                error = %e,
                "Failed to create collector WAL directory"
            );
            e
        })?;
        let mut collector_event_bus = EventBusConnector::new(collector_wal_dir).await?;

        // Connect collector's EventBusConnector and replay WAL (required for publishing)
        match collector_event_bus.connect().await {
            Ok(()) => {
                info!("Collector EventBusConnector connected");
                if let Err(e) = collector_event_bus.replay_wal().await {
                    warn!(error = %e, "Failed to replay collector WAL");
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Collector EventBusConnector failed to connect, will buffer events"
                );
            }
        }

        // Take backpressure receiver from the collector's event bus (not the shared one)
        // so the backpressure monitor listens to the correct connector
        let collector_backpressure_rx = collector_event_bus.take_backpressure_receiver();

        collector.set_event_bus_connector(collector_event_bus);

        // Spawn backpressure monitor task if we have the receiver
        let original_interval = Duration::from_secs(cli.interval);
        let backpressure_task = collector_backpressure_rx.map_or_else(
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

        // Clone handles for shutdown task
        let shutdown_handle = actor_handle.clone();
        let shutdown_registration = Arc::clone(&registration_manager);

        // Spawn task to handle graceful shutdown on Ctrl+C
        let shutdown_task = tokio::spawn(async move {
            // Wait for Ctrl+C
            if let Err(e) = tokio::signal::ctrl_c().await {
                error!(error = %e, "Failed to listen for Ctrl+C signal");
                return;
            }
            info!("Received Ctrl+C, initiating graceful shutdown");

            // Deregister from agent
            if shutdown_registration.state().await == RegistrationState::Registered {
                debug!("Deregistering from daemoneye-agent");
                if let Err(e) = shutdown_registration
                    .deregister(Some("Graceful shutdown".to_owned()))
                    .await
                {
                    warn!(error = %e, "Deregistration failed");
                }
            }

            // Send graceful shutdown to actor
            match shutdown_handle.graceful_shutdown().await {
                Ok(()) => info!("Actor shutdown completed successfully"),
                Err(e) => error!(error = %e, "Actor shutdown failed"),
            }
        });

        // Keep RPC service reference alive (it will be used for handling incoming requests)
        // In a full implementation, we would spawn a task to process RPC requests from the event bus
        let _rpc_service = rpc_service;

        // Startup behavior: begin monitoring immediately on launch.
        //
        // The collector currently does not wait for an explicit "begin monitoring"
        // command from the agent. This makes procmond usable in isolation and in
        // test environments without requiring the full agent/broker stack.
        //
        // If coordinated startup with the agent becomes a hard requirement in the
        // future, this is the place to integrate a subscription to a
        // `control.collector.lifecycle` (or similar) control topic and defer
        // calling `begin_monitoring()` until the appropriate control message is
        // received.
        info!("Starting collection immediately on startup");
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

        // Clean up backpressure monitor task
        if let Some(bp_task) = backpressure_task {
            bp_task.abort();
            info!("Backpressure monitor task aborted");
        }

        // Clean up heartbeat task
        heartbeat_task.abort();
        info!("Heartbeat task aborted");

        // Wait for event consumer to exit naturally (channel sender is dropped)
        // Use a timeout to avoid hanging indefinitely
        match tokio::time::timeout(Duration::from_secs(5), event_consumer_task).await {
            Ok(Ok(())) => info!("Event consumer task completed successfully"),
            Ok(Err(e)) => error!(error = %e, "Event consumer task join error"),
            Err(_) => {
                warn!("Event consumer task did not complete within timeout");
            }
        }

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
