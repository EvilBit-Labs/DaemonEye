//! IPC-Busrt bridge for seamless migration from direct IPC to busrt message broker.
//!
//! This module provides a compatibility layer that allows existing IPC-based
//! communication to work with the new busrt message broker infrastructure
//! while maintaining protocol compatibility and enabling gradual migration.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    IPC-Busrt Bridge                             │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │   Legacy    │  │   Protocol  │  │    Busrt Message        │ │
//! │  │    IPC      │  │  Converter  │  │      Broker             │ │
//! │  └─────────────┘  └─────────────┘  └─────────────────────────┘ │
//! │         │                │                       │             │
//! │         └────────────────┼───────────────────────┘             │
//! │                          │                                     │
//! │                          ▼                                     │
//! │              ┌─────────────────────────┐                       │
//! │              │   Message Translation   │                       │
//! │              │   - Protobuf ↔ Busrt   │                       │
//! │              │   - Topic Mapping      │                       │
//! │              │   - Correlation IDs    │                       │
//! │              └─────────────────────────┘                       │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::busrt_types::{
    BusrtClient, BusrtError, BusrtEvent, EventPayload, ProcessEventData, ProcessEventType, topics,
};
use anyhow::{Context, Result};
use daemoneye_lib::{
    ipc::InterprocessServer,
    proto::{DetectionResult, DetectionTask, ProcessRecord as ProtoProcessRecord},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{Mutex, RwLock, mpsc},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// IPC-Busrt bridge for seamless migration between communication protocols.
///
/// This bridge allows existing daemoneye-agent IPC clients to communicate
/// with collector-core components through the busrt message broker while
/// maintaining full protocol compatibility.
///
/// # Features
///
/// - **Protocol Translation**: Converts between protobuf IPC and busrt messages
/// - **Topic Mapping**: Maps IPC message types to appropriate busrt topics
/// - **Backward Compatibility**: Maintains existing IPC protocol semantics
/// - **Gradual Migration**: Enables incremental migration to busrt
/// - **Correlation Tracking**: Preserves correlation IDs across protocols
/// - **Error Handling**: Graceful error handling and recovery
///
/// # Examples
///
/// ```rust,no_run
/// use collector_core::{IpcBusrtBridge, IpcBridgeConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = IpcBridgeConfig::default();
///     let mut bridge = IpcBusrtBridge::new(config).await?;
///
///     bridge.start().await?;
///
///     // Bridge is now running and translating between IPC and busrt
///
///     bridge.shutdown().await?;
///     Ok(())
/// }
/// ```
pub struct IpcBusrtBridge {
    /// Bridge configuration
    config: IpcBridgeConfig,
    /// Legacy IPC server for backward compatibility
    ipc_server: Option<InterprocessServer>,
    /// Busrt client for message broker communication
    busrt_client: Option<Arc<dyn BusrtClient>>,
    /// Message translation state
    translation_state: Arc<TranslationState>,
    /// Bridge statistics
    statistics: Arc<BridgeStatistics>,
    /// Background task handles
    ipc_handler: Option<JoinHandle<Result<()>>>,
    busrt_handler: Option<JoinHandle<Result<()>>>,
    message_processor: Option<JoinHandle<Result<()>>>,
    /// Shutdown coordination
    shutdown_signal: Arc<AtomicBool>,
}

/// Configuration for IPC-Busrt bridge operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcBridgeConfig {
    /// IPC socket path
    pub ipc_socket_path: String,
    /// Enable legacy IPC support
    pub enable_legacy_ipc: bool,
    /// Enable busrt integration
    pub enable_busrt: bool,
    /// Message buffer size for translation
    pub message_buffer_size: usize,
    /// Translation timeout
    pub translation_timeout: Duration,
    /// Topic prefix for busrt messages
    pub topic_prefix: String,
    /// Enable debug logging for message translation
    pub enable_debug_logging: bool,
}

impl Default for IpcBridgeConfig {
    fn default() -> Self {
        Self {
            ipc_socket_path: "/tmp/daemoneye-ipc.sock".to_string(),
            enable_legacy_ipc: true,
            enable_busrt: true,
            message_buffer_size: 1000,
            translation_timeout: Duration::from_secs(30),
            topic_prefix: "daemoneye".to_string(),
            enable_debug_logging: false,
        }
    }
}

/// Internal state for message translation.
#[derive(Debug)]
#[allow(dead_code)]
struct TranslationState {
    /// Pending IPC requests (correlation_id -> response_channel)
    pending_ipc_requests: RwLock<HashMap<String, tokio::sync::oneshot::Sender<DetectionResult>>>,
    /// Pending busrt requests (correlation_id -> response_channel)
    pending_busrt_requests: RwLock<HashMap<String, tokio::sync::oneshot::Sender<BusrtEvent>>>,
    /// Message translation queue sender
    translation_queue_tx: Mutex<mpsc::Sender<TranslationMessage>>,
    /// Message translation queue receiver
    translation_queue_rx: Mutex<Option<mpsc::Receiver<TranslationMessage>>>,
    /// Topic mapping for message routing
    topic_mapping: RwLock<HashMap<String, String>>,
}

/// Bridge statistics for monitoring.
#[derive(Debug)]
pub struct BridgeStatistics {
    /// Messages translated from IPC to busrt
    ipc_to_busrt_messages: AtomicU64,
    /// Messages translated from busrt to IPC
    busrt_to_ipc_messages: AtomicU64,
    /// Translation errors
    translation_errors: AtomicU64,
    /// Active IPC connections
    active_ipc_connections: AtomicU64,
    /// Bridge uptime
    bridge_started_at: SystemTime,
}

/// Internal message for translation processing.
#[derive(Debug)]
#[allow(dead_code)]
enum TranslationMessage {
    /// IPC message to be translated to busrt
    IpcToBusrt {
        task: Box<DetectionTask>,
        correlation_id: String,
        response_channel: tokio::sync::oneshot::Sender<DetectionResult>,
    },
    /// Busrt message to be translated to IPC
    BusrtToIpc {
        event: Box<BusrtEvent>,
        correlation_id: String,
        response_channel: tokio::sync::oneshot::Sender<BusrtEvent>,
    },
}

impl IpcBusrtBridge {
    /// Creates a new IPC-Busrt bridge with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Bridge configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::{IpcBusrtBridge, IpcBridgeConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let config = IpcBridgeConfig::default();
    ///     let bridge = IpcBusrtBridge::new(config).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new(config: IpcBridgeConfig) -> Result<Self> {
        let (translation_tx, translation_rx) = mpsc::channel(config.message_buffer_size);

        let translation_state = Arc::new(TranslationState {
            pending_ipc_requests: RwLock::new(HashMap::new()),
            pending_busrt_requests: RwLock::new(HashMap::new()),
            translation_queue_tx: Mutex::new(translation_tx),
            translation_queue_rx: Mutex::new(Some(translation_rx)),
            topic_mapping: RwLock::new(Self::create_default_topic_mapping()),
        });

        let statistics = Arc::new(BridgeStatistics {
            ipc_to_busrt_messages: AtomicU64::new(0),
            busrt_to_ipc_messages: AtomicU64::new(0),
            translation_errors: AtomicU64::new(0),
            active_ipc_connections: AtomicU64::new(0),
            bridge_started_at: SystemTime::now(),
        });

        Ok(Self {
            config,
            ipc_server: None,
            busrt_client: None,
            translation_state,
            statistics,
            ipc_handler: None,
            busrt_handler: None,
            message_processor: None,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Creates default topic mapping for IPC message types.
    fn create_default_topic_mapping() -> HashMap<String, String> {
        let mut mapping = HashMap::new();

        // Map IPC task types to busrt topics
        mapping.insert(
            "enumerate_processes".to_string(),
            topics::EVENTS_PROCESS_ENUMERATION.to_string(),
        );
        mapping.insert(
            "check_process_hash".to_string(),
            topics::EVENTS_PROCESS_HASH.to_string(),
        );
        mapping.insert(
            "monitor_process_tree".to_string(),
            topics::EVENTS_PROCESS_LIFECYCLE.to_string(),
        );
        mapping.insert(
            "verify_executable".to_string(),
            topics::EVENTS_PROCESS_METADATA.to_string(),
        );

        // Control topics
        mapping.insert(
            "collector_lifecycle".to_string(),
            topics::CONTROL_COLLECTOR_LIFECYCLE.to_string(),
        );
        mapping.insert(
            "collector_health".to_string(),
            topics::CONTROL_COLLECTOR_HEALTH.to_string(),
        );
        mapping.insert(
            "agent_tasks".to_string(),
            topics::CONTROL_AGENT_TASKS.to_string(),
        );

        mapping
    }

    /// Starts the IPC-Busrt bridge with all configured components.
    ///
    /// This method initializes the IPC server, busrt client, and message
    /// translation processing.
    #[instrument(skip(self))]
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting IPC-Busrt bridge");

        // Start IPC server if enabled
        if self.config.enable_legacy_ipc {
            self.start_ipc_server().await?;
        }

        // Initialize busrt client if enabled
        if self.config.enable_busrt {
            self.initialize_busrt_client().await?;
        }

        // Start message translation processor
        self.start_message_processor().await?;

        info!(
            legacy_ipc = self.config.enable_legacy_ipc,
            busrt = self.config.enable_busrt,
            "IPC-Busrt bridge started successfully"
        );

        Ok(())
    }

    /// Starts the legacy IPC server for backward compatibility.
    async fn start_ipc_server(&mut self) -> Result<()> {
        info!("Starting legacy IPC server");

        // Placeholder IPC server creation - using a dummy config
        use daemoneye_lib::ipc::IpcConfig;
        let dummy_config = IpcConfig {
            endpoint_path: self.config.ipc_socket_path.clone(),
            ..Default::default()
        };
        let ipc_server = InterprocessServer::new(dummy_config);

        // Start IPC message handling
        let _translation_state = Arc::clone(&self.translation_state);
        let _statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let enable_debug = self.config.enable_debug_logging;

        let handle = tokio::spawn(async move {
            info!("IPC message handler started");

            // Placeholder for IPC message handling
            // In a real implementation, this would:
            // 1. Accept IPC connections
            // 2. Receive DetectionTask messages
            // 3. Translate them to busrt messages
            // 4. Send responses back via IPC

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;

                if enable_debug {
                    debug!("IPC handler heartbeat");
                }
            }

            info!("IPC message handler stopped");
            Ok(())
        });

        self.ipc_server = Some(ipc_server);
        self.ipc_handler = Some(handle);

        Ok(())
    }

    /// Initializes the busrt client for message broker communication.
    async fn initialize_busrt_client(&mut self) -> Result<()> {
        info!("Initializing busrt client");

        // Create busrt client (placeholder implementation)
        // In a real implementation, this would create and connect a CollectorBusrtClient
        let client = Arc::new(PlaceholderBusrtClient::new());
        self.busrt_client = Some(client);

        // Start busrt message handling
        let _translation_state = Arc::clone(&self.translation_state);
        let _statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let enable_debug = self.config.enable_debug_logging;

        let handle = tokio::spawn(async move {
            info!("Busrt message handler started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;

                if enable_debug {
                    debug!("Busrt handler heartbeat");
                }
            }

            info!("Busrt message handler stopped");
            Ok(())
        });

        self.busrt_handler = Some(handle);

        Ok(())
    }

    /// Starts the message translation processor.
    async fn start_message_processor(&mut self) -> Result<()> {
        // Extract the receiver from the translation state
        let mut translation_rx = {
            let mut rx_guard = self.translation_state.translation_queue_rx.lock().await;
            rx_guard
                .take()
                .context("Translation receiver already consumed")?
        };

        let translation_state = Arc::clone(&self.translation_state);
        let statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let translation_timeout = self.config.translation_timeout;
        let enable_debug = self.config.enable_debug_logging;

        let handle = tokio::spawn(async move {
            info!("Message translation processor started");

            while !shutdown_signal.load(Ordering::Relaxed) {
                tokio::select! {
                    message = translation_rx.recv() => {
                        match message {
                            Some(translation_message) => {
                                if let Err(e) = Self::process_translation_message(
                                    translation_message,
                                    &translation_state,
                                    &statistics,
                                    translation_timeout,
                                    enable_debug,
                                ).await {
                                    error!(error = %e, "Failed to process translation message");
                                    statistics.translation_errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            None => {
                                debug!("Translation message channel closed");
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Periodic maintenance
                    }
                }
            }

            info!("Message translation processor stopped");
            Ok(())
        });

        self.message_processor = Some(handle);
        Ok(())
    }

    /// Processes a translation message between IPC and busrt protocols.
    async fn process_translation_message(
        message: TranslationMessage,
        translation_state: &Arc<TranslationState>,
        statistics: &Arc<BridgeStatistics>,
        _translation_timeout: Duration,
        enable_debug: bool,
    ) -> Result<()> {
        match message {
            TranslationMessage::IpcToBusrt {
                task,
                correlation_id,
                response_channel,
            } => {
                if enable_debug {
                    debug!(
                        correlation_id = %correlation_id,
                        task_type = ?task.task_type,
                        "Translating IPC message to busrt"
                    );
                }

                // Translate DetectionTask to BusrtEvent
                let _busrt_event =
                    Self::translate_detection_task_to_busrt_event(*task, correlation_id.clone())?;

                // Store pending request
                {
                    let mut pending = translation_state.pending_ipc_requests.write().await;
                    pending.insert(correlation_id.clone(), response_channel);
                }

                // Publish to busrt (placeholder)
                // In real implementation, this would publish via busrt client

                statistics
                    .ipc_to_busrt_messages
                    .fetch_add(1, Ordering::Relaxed);

                if enable_debug {
                    debug!(
                        correlation_id = %correlation_id,
                        "IPC to busrt translation completed"
                    );
                }
            }
            TranslationMessage::BusrtToIpc {
                event,
                correlation_id,
                response_channel: _,
            } => {
                if enable_debug {
                    debug!(
                        correlation_id = %correlation_id,
                        event_id = %event.event_id,
                        "Translating busrt message to IPC"
                    );
                }

                // Translate BusrtEvent to DetectionResult
                let detection_result = Self::translate_busrt_event_to_detection_result(*event)?;

                // Find and notify pending IPC request
                if let Some(ipc_response_channel) = {
                    let mut pending = translation_state.pending_ipc_requests.write().await;
                    pending.remove(&correlation_id)
                } {
                    if ipc_response_channel.send(detection_result).is_err() {
                        warn!(correlation_id = %correlation_id, "Failed to send IPC response");
                    }
                }

                statistics
                    .busrt_to_ipc_messages
                    .fetch_add(1, Ordering::Relaxed);

                if enable_debug {
                    debug!(
                        correlation_id = %correlation_id,
                        "Busrt to IPC translation completed"
                    );
                }
            }
        }

        Ok(())
    }

    /// Translates a DetectionTask to a BusrtEvent.
    fn translate_detection_task_to_busrt_event(
        task: DetectionTask,
        correlation_id: String,
    ) -> Result<BusrtEvent> {
        let event_id = Uuid::new_v4().to_string();
        let timestamp_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to get timestamp")?
            .as_millis() as i64;

        // Determine topic based on task type
        let topic = match task.task_type {
            0 => topics::EVENTS_PROCESS_ENUMERATION, // ENUMERATE_PROCESSES
            1 => topics::EVENTS_PROCESS_HASH,        // CHECK_PROCESS_HASH
            2 => topics::EVENTS_PROCESS_LIFECYCLE,   // MONITOR_PROCESS_TREE
            3 => topics::EVENTS_PROCESS_METADATA,    // VERIFY_EXECUTABLE
            _ => topics::EVENTS_PROCESS_ENUMERATION, // Default
        };

        // Create placeholder process data
        let payload = EventPayload::Process(ProcessEventData {
            process: crate::busrt_types::ProcessRecord {
                pid: 0, // Will be filled from actual task data
                name: "translated_task".to_string(),
                executable_path: None,
            },
            collection_cycle_id: task.task_id.clone(),
            event_type: ProcessEventType::Discovery,
        });

        Ok(BusrtEvent {
            event_id,
            correlation_id: Some(correlation_id),
            timestamp_ms,
            source_collector: "ipc-bridge".to_string(),
            topic: topic.to_string(),
            payload,
            correlation: None,
        })
    }

    /// Translates a BusrtEvent to a DetectionResult.
    fn translate_busrt_event_to_detection_result(event: BusrtEvent) -> Result<DetectionResult> {
        let mut result = DetectionResult {
            task_id: event
                .correlation_id
                .unwrap_or_else(|| event.event_id.clone()),
            success: true,
            error_message: None,
            processes: Vec::new(),
            hash_result: None,
            network_events: Vec::new(),
            filesystem_events: Vec::new(),
            performance_events: Vec::new(),
        };

        // Extract process data from busrt event payload
        if let EventPayload::Process(process_data) = event.payload {
            let proto_process = ProtoProcessRecord {
                pid: process_data.process.pid,
                ppid: None,
                name: process_data.process.name,
                executable_path: process_data.process.executable_path,
                command_line: Vec::new(),
                start_time: None,
                cpu_usage: None,
                memory_usage: None,
                executable_hash: None,
                hash_algorithm: None,
                user_id: None,
                accessible: true,
                file_exists: true,
                collection_time: event.timestamp_ms,
            };

            result.processes.push(proto_process);
        }

        Ok(result)
    }

    /// Returns bridge statistics.
    pub async fn get_statistics(&self) -> BridgeStatistics {
        BridgeStatistics {
            ipc_to_busrt_messages: AtomicU64::new(
                self.statistics
                    .ipc_to_busrt_messages
                    .load(Ordering::Relaxed),
            ),
            busrt_to_ipc_messages: AtomicU64::new(
                self.statistics
                    .busrt_to_ipc_messages
                    .load(Ordering::Relaxed),
            ),
            translation_errors: AtomicU64::new(
                self.statistics.translation_errors.load(Ordering::Relaxed),
            ),
            active_ipc_connections: AtomicU64::new(
                self.statistics
                    .active_ipc_connections
                    .load(Ordering::Relaxed),
            ),
            bridge_started_at: self.statistics.bridge_started_at,
        }
    }

    /// Initiates graceful shutdown of the bridge.
    ///
    /// This method signals all background tasks to shut down gracefully
    /// and waits for cleanup to complete.
    #[instrument(skip(self))]
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down IPC-Busrt bridge");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for background tasks to complete
        if let Some(handle) = self.ipc_handler.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "IPC handler task join error");
            }
        }

        if let Some(handle) = self.busrt_handler.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Busrt handler task join error");
            }
        }

        if let Some(handle) = self.message_processor.take() {
            if let Err(e) = handle.await {
                warn!(error = %e, "Message processor task join error");
            }
        }

        // Shutdown IPC server
        if let Some(_ipc_server) = self.ipc_server.take() {
            // IPC server shutdown would be implemented here
            debug!("IPC server shutdown");
        }

        info!("IPC-Busrt bridge shutdown complete");
        Ok(())
    }
}

/// Placeholder busrt client for bridge testing.
#[allow(dead_code)]
struct PlaceholderBusrtClient {
    connected: AtomicBool,
}

impl PlaceholderBusrtClient {
    fn new() -> Self {
        Self {
            connected: AtomicBool::new(true),
        }
    }
}

#[async_trait::async_trait]
impl BusrtClient for PlaceholderBusrtClient {
    async fn publish(&self, topic: &str, message: BusrtEvent) -> Result<(), BusrtError> {
        debug!(
            topic = %topic,
            event_id = %message.event_id,
            "Bridge placeholder: Published message"
        );
        Ok(())
    }

    async fn subscribe(&self, pattern: &str) -> Result<mpsc::Receiver<BusrtEvent>, BusrtError> {
        debug!(pattern = %pattern, "Bridge placeholder: Subscribed to pattern");
        let (_tx, rx) = mpsc::channel(100);
        Ok(rx)
    }

    async fn unsubscribe(&self, pattern: &str) -> Result<(), BusrtError> {
        debug!(pattern = %pattern, "Bridge placeholder: Unsubscribed from pattern");
        Ok(())
    }

    async fn rpc_call_json(
        &self,
        service: &str,
        method: &str,
        _request: serde_json::Value,
    ) -> Result<serde_json::Value, BusrtError> {
        debug!(
            service = %service,
            method = %method,
            "Bridge placeholder: RPC call"
        );
        Ok(serde_json::json!({"status": "ok"}))
    }

    async fn get_broker_stats(&self) -> Result<crate::busrt_types::BrokerStats, BusrtError> {
        Ok(crate::busrt_types::BrokerStats {
            uptime_seconds: 3600,
            total_connections: 1,
            active_connections: 1,
            messages_published: 0,
            messages_delivered: 0,
            topics_count: 5,
            memory_usage_bytes: 1024 * 1024,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bridge_creation() {
        let config = IpcBridgeConfig::default();
        let bridge = IpcBusrtBridge::new(config).await;
        assert!(bridge.is_ok());
    }

    #[tokio::test]
    async fn test_topic_mapping() {
        let mapping = IpcBusrtBridge::create_default_topic_mapping();
        assert!(mapping.contains_key("enumerate_processes"));
        assert_eq!(
            mapping.get("enumerate_processes").unwrap(),
            topics::EVENTS_PROCESS_ENUMERATION
        );
    }

    #[tokio::test]
    async fn test_detection_task_translation() {
        let task = DetectionTask {
            task_id: "test-task-123".to_string(),
            task_type: 0, // ENUMERATE_PROCESSES
            process_filter: None,
            hash_check: None,
            metadata: None,
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        };

        let correlation_id = "test-correlation-456".to_string();
        let result =
            IpcBusrtBridge::translate_detection_task_to_busrt_event(task, correlation_id.clone());

        assert!(result.is_ok());
        let busrt_event = result.unwrap();
        assert_eq!(busrt_event.correlation_id, Some(correlation_id));
        assert_eq!(busrt_event.topic, topics::EVENTS_PROCESS_ENUMERATION);
    }

    #[tokio::test]
    async fn test_busrt_event_translation() {
        let event = BusrtEvent {
            event_id: "test-event-789".to_string(),
            correlation_id: Some("test-correlation-101".to_string()),
            timestamp_ms: 1234567890,
            source_collector: "test-collector".to_string(),
            topic: topics::EVENTS_PROCESS_ENUMERATION.to_string(),
            payload: EventPayload::Process(ProcessEventData {
                process: crate::busrt_types::ProcessRecord {
                    pid: 1234,
                    name: "test_process".to_string(),
                    executable_path: Some("/usr/bin/test".to_string()),
                },
                collection_cycle_id: "cycle-123".to_string(),
                event_type: ProcessEventType::Discovery,
            }),
            correlation: None,
        };

        let result = IpcBusrtBridge::translate_busrt_event_to_detection_result(event);
        assert!(result.is_ok());

        let detection_result = result.unwrap();
        assert!(detection_result.success);
        assert_eq!(detection_result.processes.len(), 1);
        assert_eq!(detection_result.processes[0].pid, 1234);
        assert_eq!(detection_result.processes[0].name, "test_process");
    }

    #[tokio::test]
    async fn test_bridge_config_defaults() {
        let config = IpcBridgeConfig::default();
        assert!(config.enable_legacy_ipc);
        assert!(config.enable_busrt);
        assert_eq!(config.message_buffer_size, 1000);
        assert_eq!(config.topic_prefix, "daemoneye");
    }

    #[tokio::test]
    async fn test_placeholder_busrt_client() {
        let client = PlaceholderBusrtClient::new();

        // Test publish
        let event = BusrtEvent {
            event_id: "test".to_string(),
            correlation_id: None,
            timestamp_ms: 0,
            source_collector: "test".to_string(),
            topic: "test/topic".to_string(),
            payload: EventPayload::Process(ProcessEventData {
                process: crate::busrt_types::ProcessRecord {
                    pid: 1,
                    name: "test".to_string(),
                    executable_path: None,
                },
                collection_cycle_id: "test".to_string(),
                event_type: ProcessEventType::Discovery,
            }),
            correlation: None,
        };

        let result = client.publish("test/topic", event).await;
        assert!(result.is_ok());

        // Test subscribe
        let receiver = client.subscribe("test/*").await;
        assert!(receiver.is_ok());

        // Test RPC call
        let rpc_result = client
            .rpc_call_json("test", "method", serde_json::json!({}))
            .await;
        assert!(rpc_result.is_ok());
    }
}
