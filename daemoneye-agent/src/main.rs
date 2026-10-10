#![forbid(unsafe_code)]

use clap::Parser;
use daemoneye_agent::detection_cycle::{
    PROCMOND_COLLECTOR_ID, build_signals, ingest_cycle, load_persisted_rules, next_cycle_ordinal,
    next_window, open_event_store, persist_alerts, run_detection_cycle,
};
use daemoneye_lib::detection::execution::completeness::IngestSnapshot;
use daemoneye_lib::detection::execution::executor::RuleExecutor;
use daemoneye_lib::storage::ingest::{self, IngestConfig};
use daemoneye_lib::{alerting, config, detection_bounds, telemetry};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, error, info, warn};

mod broker_manager;
mod collector_admission;
mod collector_config;
mod collector_registry;
mod health;
mod integrity_alerts;
mod ipc_server;
mod pushdown_renewal;

use broker_manager::BrokerManager;
use collector_config::CollectorsConfig;
use ipc_server::IpcServerManager;

#[derive(Parser)]
#[command(name = "daemoneye-agent")]
#[command(about = "DaemonEye Detection and Alerting Orchestrator")]
#[command(version)]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/daemoneye/processes.db")]
    database: String,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments first - this will handle --help and --version automatically
    let cli = Cli::parse();
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Test mode: exit early to keep existing integration test semantics (set DAEMONEYE_AGENT_TEST_MODE=1)
    if std::env::var("DAEMONEYE_AGENT_TEST_MODE").is_ok_and(|v| v == "1") {
        #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
        {
            println!("daemoneye-agent started successfully")
        };
        return Ok(());
    }

    // Load configuration
    let config_loader = config::ConfigLoader::new("daemoneye-agent");
    let mut config = config_loader.load()?;

    // Override database path from CLI argument if provided
    config.database.path = cli.database.into();

    // Initialize telemetry
    let mut telemetry = telemetry::TelemetryCollector::new("daemoneye-agent".to_owned());

    // Initialize the event store and the ingest pipeline that writes into it.
    let event_store = Arc::new(open_event_store(&config.database.path, &config.database)?);
    let ingest_handle = ingest::spawn(Arc::clone(&event_store), IngestConfig::default());
    // One above the last ordinal committed, so a restart never reuses a sequence the stored
    // watermark would discard as already delivered.
    let mut next_ordinal = Some(next_cycle_ordinal(&event_store, PROCMOND_COLLECTOR_ID)?);

    // Initialize embedded EventBus broker
    let broker_manager =
        BrokerManager::with_detection_config(config.broker.clone(), &config.detection);

    // Start the embedded broker
    if let Err(e) = broker_manager.start().await {
        error!(error = %e, "Failed to start embedded EventBus broker");
        return Err(e.into());
    }

    // Wait for broker to become healthy
    let broker_startup_timeout = Duration::from_secs(config.broker.startup_timeout_seconds);
    if let Err(e) = broker_manager
        .wait_for_healthy(broker_startup_timeout)
        .await
    {
        error!(error = %e, "Embedded broker failed to become healthy");
        return Err(e.into());
    }

    info!(
        socket_path = %broker_manager.socket_path(),
        "Embedded EventBus broker is healthy and ready"
    );

    // Initialize IPC server for CLI communication
    let cli_ipc_config = ipc_server::create_cli_ipc_config();
    let ipc_server_manager = IpcServerManager::new(cli_ipc_config);

    // Start the IPC server
    if let Err(e) = ipc_server_manager.start().await {
        error!(error = %e, "Failed to start IPC server for CLI communication");
        return Err(e.into());
    }

    // Wait for IPC server to become healthy
    let ipc_startup_timeout = Duration::from_secs(10); // 10 second timeout for IPC server
    if let Err(e) = ipc_server_manager
        .wait_for_healthy(ipc_startup_timeout)
        .await
    {
        error!(error = %e, "IPC server failed to become healthy");
        return Err(e.into());
    }

    info!(
        endpoint_path = %ipc_server_manager.endpoint_path(),
        "IPC server is healthy and ready for CLI communication"
    );

    // =========================================================================
    // Loading State Coordination
    // =========================================================================
    // The agent starts in Loading state. We load the collectors configuration,
    // wait for all expected collectors to register, then transition through:
    // Loading -> Ready -> SteadyState

    // Load collectors configuration (defaults to empty if file doesn't exist)
    let collectors_config_path = std::path::Path::new("/etc/daemoneye/collectors.json");
    let collectors_config = match CollectorsConfig::load_from_file(collectors_config_path) {
        Ok(loaded_config) => {
            info!(
                path = %collectors_config_path.display(),
                "Loaded collectors configuration from file"
            );
            loaded_config
        }
        Err(e) => {
            debug!(
                path = %collectors_config_path.display(),
                error = %e,
                "No collectors config file found, using empty configuration"
            );
            CollectorsConfig::default()
        }
    };

    let expected_count = collectors_config.enabled_collectors().count();

    info!(
        config_path = %collectors_config_path.display(),
        total_collectors = collectors_config.collectors.len(),
        enabled_collectors = expected_count,
        "Loaded collectors configuration"
    );

    // Set the collectors configuration on the broker manager
    broker_manager
        .set_collectors_config(collectors_config.clone())
        .await;

    // Get the startup timeout from the collectors configuration
    let startup_timeout = broker_manager.get_startup_timeout().await;

    let current_agent_state = broker_manager.agent_state().await;
    info!(
        agent_state = %current_agent_state,
        expected_collectors = expected_count,
        startup_timeout_secs = startup_timeout.as_secs(),
        "Waiting for collectors to register"
    );

    // Wait for all expected collectors to register
    // If no collectors are expected, this will return immediately
    // Poll every second for collector readiness
    let poll_interval = Duration::from_secs(1);
    match broker_manager
        .wait_for_collectors_ready(startup_timeout, poll_interval)
        .await
    {
        Ok(true) => {
            info!("All expected collectors have registered");

            // Transition to Ready state
            if let Err(e) = broker_manager.transition_to_ready().await {
                error!(error = %e, "Failed to transition to Ready state");
                broker_manager
                    .mark_startup_failed(format!("Failed to transition to Ready: {e}"))
                    .await;
                return Err(e.into());
            }

            let ready_state = broker_manager.agent_state().await;
            info!(agent_state = %ready_state, "Agent is now Ready");

            // Drop privileges (stub - not yet implemented)
            if let Err(e) = broker_manager.drop_privileges().await {
                error!(error = %e, "Failed to drop privileges");
                // Continue anyway - privilege dropping failure is not fatal
                warn!("Continuing with elevated privileges");
            }

            // Transition to SteadyState (this broadcasts "begin monitoring" internally)
            if let Err(e) = broker_manager.transition_to_steady_state().await {
                error!(error = %e, "Failed to transition to SteadyState");
                // This shouldn't happen if we're in Ready state, but log and continue
                warn!("Agent may not be in expected state");
            }

            let steady_state = broker_manager.agent_state().await;
            info!(
                agent_state = %steady_state,
                "Agent startup complete, entering steady state operation"
            );
        }
        Ok(false) => {
            // Timeout - not all collectors registered in time
            // The wait_for_collectors_ready function already marked startup as failed
            error!(
                expected = expected_count,
                "Startup timeout: not all collectors registered in time"
            );

            let current_state = broker_manager.agent_state().await;
            error!(
                agent_state = %current_state,
                "Agent startup failed due to timeout"
            );

            return Err(anyhow::anyhow!("Startup timeout: not all collectors registered").into());
        }
        Err(e) => {
            error!(
                error = %e,
                expected = expected_count,
                "Startup error while waiting for collectors"
            );
            broker_manager
                .mark_startup_failed(format!("Startup error: {e}"))
                .await;

            let current_state = broker_manager.agent_state().await;
            error!(
                agent_state = %current_state,
                "Agent startup failed"
            );

            return Err(e.into());
        }
    }

    // =========================================================================
    // Initialize Detection and Alerting
    // =========================================================================

    // The detection engine the admission gate already feeds. It is *not* constructed here: a
    // second engine would leave this one's catalog empty forever, so every rule would defer under
    // R18 and never plan, with nothing to see in the logs.
    let detection_engine = Arc::clone(broker_manager.detection_engine());

    // Reload every persisted rule; a rejected one is logged with its id and does not stop startup.
    let mut startup_engine = detection_engine.lock().await;
    let loaded = load_persisted_rules(&event_store, &mut startup_engine)?;
    // One executor for the process, sharing the engine's compiled-pattern cache so a pattern the
    // planner validated is not compiled twice.
    let executor = RuleExecutor::new(
        Arc::clone(&event_store),
        startup_engine.regex_cache(),
        &config.detection,
    )?;
    drop(startup_engine);
    info!(loaded_rules = loaded, "Loaded persisted detection rules");

    // Initialize alert manager
    let mut alert_manager = alerting::AlertManager::new();
    let stdout_sink = Box::new(alerting::StdoutSink::new(
        "stdout".to_owned(),
        alerting::OutputFormat::Json,
    ));
    alert_manager.add_sink(stdout_sink);

    // Indicate startup success before entering main loop
    #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
    {
        println!("daemoneye-agent started successfully")
    };

    // Main collection loop using IPC client
    let scan_interval = Duration::from_millis(config.app.scan_interval_ms);
    info!(
        interval_ms = config.app.scan_interval_ms,
        "Entering main collection+detection loop with RPC client"
    );

    // Graceful shutdown signal future
    let shutdown_signal = async {
        // Wait for Ctrl+C
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "Failed to listen for shutdown signal");
        }
    };

    // Main loop task
    let mut iteration: u64 = 0;

    // The previous cycle's high-water mark: the exclusive start of the next cycle's window (R3).
    // Rows already stored when the agent starts belong to an earlier run and are not re-alerted.
    let mut previous_high_water_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or_default()
        });
    // Ingest's saturation counter at the end of the previous cycle, to report the delta.
    let mut last_saturation_alerts = 0_u64;

    // Session-scoped ssdeep binary-change tracker (R2 AC7). Holds the last
    // ssdeep digest per executable path so a similarity drop versus the
    // previously recorded value raises a binary-change observation. Built from
    // the default fuzzy config (validated); falls back to the default tracker if
    // the config is ever out of range.
    let mut binary_change_tracker = integrity_alerts::BinaryChangeTracker::new(
        daemoneye_lib::integrity::fuzzy::FuzzyConfig::default(),
    )
    .unwrap_or_default();

    // R16's clock. Deliberately its own ticker rather than a step inside the scan branch:
    // scan_interval_ms is an operator-tunable up to an hour, and a scan interval above the task
    // TTL would expire every pushed task before the loop next woke, marking every rule unhealthy
    // on a timer. Task lifetime and collection cadence are unrelated concerns, so they get
    // unrelated timers. Ticking at half the renewal interval bounds how late a due renewal can be
    // by that half, which keeps two consecutive failed sends inside the TTL.
    // `checked_div` rather than `/`: `clippy::arithmetic_side_effects` is denied, and a divisor
    // that cannot be zero here still has to say so in the types.
    let tick = detection_bounds::PUSHDOWN_TASK_RENEWAL_INTERVAL
        .checked_div(2)
        .unwrap_or(detection_bounds::PUSHDOWN_TASK_RENEWAL_INTERVAL);
    let mut renewal_ticker = tokio::time::interval(tick);
    renewal_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // The scan gets its own ticker for the same reason, and neither branch may be a bare
    // `sleep(..)`: `tokio::select!` drops the losing futures, so a timer built inline restarts from
    // zero every time the other branch wins. A scan_interval at or above the renewal tick would
    // then never elapse and collection would stop silently. Two `Interval`s advance independently.
    // Started one interval out so the first scan still waits `scan_interval` — `tokio::time::interval`
    // otherwise fires immediately. `Delay` keeps at least `scan_interval` between scans when one
    // overruns, matching what the previous `sleep` did; `Burst` would fire back-to-back to catch up.
    let scan_start = tokio::time::Instant::now()
        .checked_add(scan_interval)
        .unwrap_or_else(tokio::time::Instant::now);
    let mut scan_ticker = tokio::time::interval_at(scan_start, scan_interval);
    scan_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    tokio::pin!(shutdown_signal);

    loop {
        tokio::select! {
            () = &mut shutdown_signal => {
                info!("Shutdown signal received; commencing graceful shutdown");
                break;
            }
            _ = renewal_ticker.tick() => {
                // Keep every enabled rule's pushed half alive, and mark unhealthy the ones whose
                // task lapsed (R16). This evaluates nothing; it only runs the TTL clock.
                let now = SystemTime::now();
                // R16's re-issue trigger, before the renewal pass: a collector that registered
                // since the last tick lost its accepted-task map, so its whole active set is sent
                // anew. Doing it first refreshes those tasks, so the renewal pass that follows
                // does not send them a second time.
                if let Some(admission) = broker_manager.collector_admission() {
                    let reissued = pushdown_renewal::reissue_registered_collectors(
                        &detection_engine,
                        admission,
                        &broker_manager,
                        now,
                    )
                    .await;
                    if !reissued.failed.is_empty() {
                        let failed = reissued.failed.len();
                        warn!(failed_tasks = failed, "Task re-issue did not reach its collector");
                    }
                }

                let renewal = pushdown_renewal::run_renewal_cycle(
                    &detection_engine,
                    &broker_manager,
                    now,
                )
                .await;
                if !renewal.expired_rules.is_empty() {
                    let expired = renewal.expired_rules.len();
                    warn!(expired_rules = expired, "Pushed halves lapsed; rules marked unhealthy");
                }
            }
            _ = scan_ticker.tick() => {
                iteration = iteration.saturating_add(1);
                let loop_start = Instant::now();

                // Periodic RPC health checks (every 10 iterations)
                if iteration.is_multiple_of(10) {
                    // Get list of registered collectors from RPC clients
                    let collector_ids = broker_manager.list_registered_collector_ids().await;

                    for collector_id in collector_ids {
                        match broker_manager.health_check_rpc(&collector_id).await {
                            Ok(health_data) => {
                                info!(
                                    collector_id = %collector_id,
                                    status = ?health_data.status,
                                    "RPC health check completed"
                                );
                            }
                            Err(e) => {
                                warn!(
                                    collector_id = %collector_id,
                                    error = %e,
                                    "RPC health check failed"
                                );
                            }
                        }
                    }
                }

                // Request process enumeration from procmond via RPC
                let task = daemoneye_lib::proto::DetectionTask::new_enumerate_processes(
                    uuid::Uuid::new_v4().to_string(),
                    None,
                );

                // Integrity-signal alerts raised from per-process wire flags
                // (ssdeep_degraded / on_disk_state). Read from the proto
                // records before they are converted to the native model, which
                // does not carry these flags.
                let mut integrity_alert_batch: Vec<daemoneye_lib::models::Alert> = Vec::new();

                let mut collection: Result<(), String> = Ok(());
                let processes = match broker_manager.execute_task_rpc("procmond", task).await {
                    Ok(result) => {
                        if result.success {
                            info!(
                                process_count = result.processes.len(),
                                "Successfully collected process data from procmond via RPC"
                            );
                            integrity_alert_batch =
                                integrity_alerts::detect_integrity_alerts(&result.processes);
                            integrity_alert_batch
                                .extend(binary_change_tracker.observe(&result.processes));
                            // Parse process data from DetectionResult.processes
                            result
                                .processes
                                .into_iter()
                                .map(Into::into)
                                .collect()
                        } else {
                            let message = result
                                .error_message
                                .as_deref()
                                .unwrap_or("Unknown error")
                                .to_owned();
                            warn!(
                                error = %message,
                                "Procmond returned error during process enumeration via RPC"
                            );
                            collection = Err(message);
                            Vec::new()
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to collect processes from procmond via RPC");
                        collection = Err(e.to_string());
                        Vec::new()
                    }
                };

                // Commit this cycle's rows before anything evaluates them. A cycle that collected
                // nothing consumes no ordinal, so the next one stays contiguous for the watermark.
                let mut ingested_high_water_ms = previous_high_water_ms;
                let mut sequence_gaps = Vec::new();
                match next_ordinal {
                    Some(ordinal) if !processes.is_empty() => {
                        let ingested = ingest_cycle(
                            &ingest_handle,
                            PROCMOND_COLLECTOR_ID,
                            ordinal,
                            &processes,
                        )
                        .await;
                        match ingested {
                            Ok(outcome) => {
                                next_ordinal = ordinal.checked_add(1);
                                ingested_high_water_ms =
                                    ingested_high_water_ms.max(outcome.high_water_ms);
                                debug!(
                                    submitted = outcome.submitted,
                                    high_water_ms = outcome.high_water_ms,
                                    gaps = outcome.gaps.len(),
                                    "Ingested cycle"
                                );
                                sequence_gaps = outcome.gaps;
                            }
                            Err(e) => {
                                error!(error = %e, "Ingest failed; this cycle's rows are not durable");
                                telemetry.record_error();
                            }
                        }
                    }
                    Some(_) => {}
                    None => error!("Cycle ordinals exhausted; collected rows are not being stored"),
                }

                // Evaluate every eligible rule over what this cycle added to the store.
                let detection_timer = telemetry::PerformanceTimer::start("detection_execution".to_owned());
                let saturation_now = ingest_handle
                    .metrics()
                    .saturation_alerts
                    .load(Ordering::Relaxed);
                let heartbeat = broker_manager.collector_heartbeat_health(PROCMOND_COLLECTOR_ID).await;
                let signals = build_signals(
                    PROCMOND_COLLECTOR_ID,
                    collection,
                    heartbeat,
                    IngestSnapshot {
                        saturation_delta: saturation_now.saturating_sub(last_saturation_alerts),
                        sequence_gaps,
                    },
                );
                last_saturation_alerts = saturation_now;
                let window = next_window(previous_high_water_ms, ingested_high_water_ms);
                previous_high_water_ms = ingested_high_water_ms;
                let cycle = run_detection_cycle(&*detection_engine, &executor, window, &signals).await;
                if cycle.dropped_after_reeligibility > 0 {
                    warn!(
                        dropped = cycle.dropped_after_reeligibility,
                        "Rules changed while they ran; their results were dropped"
                    );
                }
                let mut alerts = cycle.alerts;
                // Fold in integrity-signal alerts so they share the dedup,
                // rate-limit, and delivery path of detection-rule alerts.
                alerts.extend(integrity_alert_batch);

                if !alerts.is_empty() {
                    info!(count = alerts.len(), "Generated alerts");
                }
                for alert in &alerts {
                    match alert_manager.send_alert(alert).await {
                        Ok(results) => {
                            if results.is_empty() { warn!("Alert generated but no sinks succeeded"); }
                        }
                        Err(e) => {
                            error!(error=?e, "Failed to deliver alert");
                            telemetry.record_error();
                        }
                    }
                }
                // Every delivered alert is also stored, so its completeness marker is readable later.
                let stored_alerts = persist_alerts(&event_store, &alerts);
                if stored_alerts != alerts.len() {
                    warn!(stored = stored_alerts, total = alerts.len(), "Some alerts were not persisted");
                }
                let detection_duration = detection_timer.finish();
                telemetry.record_operation(detection_duration);

                // Update telemetry with rough resource usage snapshot (placeholder zeros for now)
                telemetry.update_resource_usage(0.0, 0);
                if iteration.is_multiple_of(10) { // periodic health check every 10 iterations
                    let h = telemetry.health_check();
                    info!(status=%h.status, "Telemetry health check");

                    // Check broker health
                    let broker_health = broker_manager.health_check().await;
                    match broker_health {
                        broker_manager::BrokerHealth::Healthy => {
                            if let Some(stats) = broker_manager.statistics().await {
                                debug!(
                                    messages_published = stats.messages_published,
                                    messages_delivered = stats.messages_delivered,
                                    active_subscribers = stats.active_subscribers,
                                    uptime_seconds = stats.uptime_seconds,
                                    "Broker health check passed"
                                );
                            }
                        }
                        broker_manager::BrokerHealth::Unhealthy(ref error) => {
                            warn!(error = %error, "Broker health check failed");
                        }
                        broker_manager::BrokerHealth::Starting
                        | broker_manager::BrokerHealth::ShuttingDown
                        | broker_manager::BrokerHealth::Stopped => {
                            debug!(status = ?broker_health, "Broker health status");
                        }
                    }

                    // Check IPC server health
                    let ipc_health = ipc_server_manager.health_check().await;
                    match ipc_health {
                        ipc_server::IpcServerHealth::Healthy => {
                            debug!("IPC server health check passed");
                        }
                        ipc_server::IpcServerHealth::Unhealthy(ref error) => {
                            warn!(error = %error, "IPC server health check failed");
                        }
                        ipc_server::IpcServerHealth::Starting
                        | ipc_server::IpcServerHealth::ShuttingDown
                        | ipc_server::IpcServerHealth::Stopped => {
                            debug!(status = ?ipc_health, "IPC server health status");
                        }
                    }
                }
                let loop_elapsed = loop_start.elapsed();
                #[allow(clippy::as_conversions)] // Safe: loop elapsed will not overflow u64
                let elapsed_ms = loop_elapsed.as_millis() as u64;
                if loop_elapsed > scan_interval { warn!(elapsed_ms = elapsed_ms, "Loop overran scan interval"); }
            }
        }
    }

    // Gracefully shutdown both services in parallel
    info!("Shutting down IPC server and embedded EventBus broker");

    let (ipc_result, broker_result) =
        tokio::join!(ipc_server_manager.shutdown(), broker_manager.shutdown());

    if let Err(e) = ipc_result {
        error!(error = %e, "Failed to shutdown IPC server gracefully");
    }

    if let Err(e) = broker_result {
        error!(error = %e, "Failed to shutdown embedded broker gracefully");
    }

    // After the broker, so nothing is still submitting; commits whatever is queued.
    ingest_handle.flush_and_stop().await;

    #[allow(clippy::print_stdout, clippy::semicolon_if_nothing_returned)]
    {
        println!("daemoneye-agent shutdown complete.")
    };
    Ok(())
}
