//! EventBus connector for reliable event delivery with WAL integration.
//!
//! This module provides the [`EventBusConnector`] component that integrates the
//! Write-Ahead Log (WAL) with the daemoneye-eventbus for reliable, crash-recoverable
//! event delivery from procmond to daemoneye-agent.
//!
//! # Overview
//!
//! The connector implements a durable publishing pattern:
//! 1. Events are first written to the WAL for durability
//! 2. If connected, events are published to the broker
//! 3. On successful publish, WAL entries are marked as published
//! 4. If disconnected, events are buffered in memory (up to 10MB)
//! 5. On reconnection, WAL is replayed to recover unpublished events
//!
//! # Connection Configuration
//!
//! The connector reads the broker socket path from the `DAEMONEYE_BROKER_SOCKET`
//! environment variable. If not set, connection attempts will fail with
//! [`EventBusConnectorError::EnvNotSet`].
//!
//! # Topic Mapping
//!
//! Process events are published to topic hierarchy under `events.process`:
//! - [`ProcessEventType::Start`] -> `events.process.start`
//! - [`ProcessEventType::Stop`] -> `events.process.stop`
//! - [`ProcessEventType::Modify`] -> `events.process.modify`
//!
//! # Examples
//!
//! ```rust,no_run
//! use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};
//! use collector_core::event::ProcessEvent;
//! use std::path::PathBuf;
//! use std::time::SystemTime;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create connector with WAL directory
//! let mut connector = EventBusConnector::new(PathBuf::from("/var/lib/procmond/wal")).await?;
//!
//! // Connect to broker
//! connector.connect().await?;
//!
//! // Publish a process start event
//! let event = ProcessEvent {
//!     pid: 1234,
//!     ppid: Some(1),
//!     name: "example".to_string(),
//!     executable_path: Some("/usr/bin/example".to_string()),
//!     command_line: vec!["example".to_string(), "--flag".to_string()],
//!     start_time: Some(SystemTime::now()),
//!     cpu_usage: None,
//!     memory_usage: None,
//!     executable_hash: None,
//!     user_id: Some("1000".to_string()),
//!     accessible: true,
//!     file_exists: true,
//!     timestamp: SystemTime::now(),
//!     platform_metadata: None,
//! };
//!
//! let sequence = connector.publish(event, ProcessEventType::Start).await?;
//! println!("Published event with sequence: {}", sequence);
//!
//! // Graceful shutdown
//! connector.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use crate::wal::{WalError, WriteAheadLog};
use collector_core::event::ProcessEvent;
use daemoneye_eventbus::{
    ClientConfig, CollectionEvent as EventBusCollectionEvent, EventBusClient,
    ProcessEvent as EventBusProcessEvent, SocketConfig,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Environment variable name for broker socket path.
const BROKER_SOCKET_ENV: &str = "DAEMONEYE_BROKER_SOCKET";

/// Maximum buffer size in bytes (10MB).
const MAX_BUFFER_SIZE: usize = 10 * 1024 * 1024;

/// Default Windows named pipe name.
const DEFAULT_WINDOWS_PIPE: &str = r"\\.\pipe\DaemonEye-broker";

/// Errors that can occur during event bus connector operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EventBusConnectorError {
    /// WAL operation failed.
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),

    /// EventBus operation failed.
    #[error("EventBus error: {0}")]
    EventBus(String),

    /// Connection to broker failed.
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Buffer has reached capacity.
    #[error("Buffer overflow: buffer is at capacity")]
    BufferOverflow,

    /// Required environment variable is not set.
    #[error("Environment variable not set: {0}")]
    EnvNotSet(String),

    /// Serialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Result type for event bus connector operations.
pub type EventBusConnectorResult<T> = Result<T, EventBusConnectorError>;

/// Backpressure signal indicating buffer pressure state.
///
/// These signals are emitted when the buffer crosses threshold levels,
/// allowing upstream producers to adjust their event generation rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackpressureSignal {
    /// Buffer has reached high-water mark (70% full).
    /// Upstream should slow down event production.
    Activated,

    /// Buffer has dropped below low-water mark (50% full).
    /// Normal event production can resume.
    Released,
}

/// Type of process event for topic routing.
///
/// Determines which topic the event will be published to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcessEventType {
    /// Process started - published to `events.process.start`.
    Start,
    /// Process stopped - published to `events.process.stop`.
    Stop,
    /// Process modified (e.g., name change) - published to `events.process.modify`.
    Modify,
}

impl ProcessEventType {
    /// Get the topic string for this event type.
    const fn topic(self) -> &'static str {
        match self {
            Self::Start => "events.process.start",
            Self::Stop => "events.process.stop",
            Self::Modify => "events.process.modify",
        }
    }

    /// Get the event type as a short string for WAL persistence.
    const fn to_type_string(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Modify => "modify",
        }
    }

    /// Parse event type from a string stored in WAL.
    ///
    /// Returns `Start` as a default for unknown or legacy entries.
    fn from_type_string(s: &str) -> Self {
        match s {
            "start" => Self::Start,
            "stop" => Self::Stop,
            "modify" => Self::Modify,
            _ => {
                warn!(event_type = s, "Unknown event type, defaulting to Start");
                Self::Start
            }
        }
    }
}

/// An event buffered in memory when disconnected from the broker.
#[derive(Debug)]
struct BufferedEvent {
    /// WAL sequence number for this event.
    sequence: u64,
    /// The process event to publish.
    event: ProcessEvent,
    /// Topic to publish to.
    topic: String,
    /// Estimated size in bytes for buffer accounting.
    size_bytes: usize,
}

impl BufferedEvent {
    /// Create a new buffered event with size estimation.
    fn new(sequence: u64, event: ProcessEvent, topic: String) -> Self {
        // Estimate size based on event fields
        let size_bytes = Self::estimate_size(&event, &topic);
        Self {
            sequence,
            event,
            topic,
            size_bytes,
        }
    }

    /// Estimate the serialized size of an event.
    fn estimate_size(event: &ProcessEvent, topic: &str) -> usize {
        // Base overhead for struct fields
        let mut size = 64_usize;

        // Add string lengths
        size = size.saturating_add(event.name.len());
        if let Some(ref path) = event.executable_path {
            size = size.saturating_add(path.len());
        }
        for arg in &event.command_line {
            size = size.saturating_add(arg.len());
        }
        if let Some(ref hash) = event.executable_hash {
            size = size.saturating_add(hash.len());
        }
        if let Some(ref uid) = event.user_id {
            size = size.saturating_add(uid.len());
        }
        if let Some(ref meta) = event.platform_metadata {
            // Rough estimate for JSON metadata
            size = size.saturating_add(meta.to_string().len());
        }
        size = size.saturating_add(topic.len());

        size
    }
}

/// Connector for publishing events to daemoneye-agent's broker with WAL-backed durability.
///
/// The `EventBusConnector` provides reliable event delivery by integrating the
/// Write-Ahead Log for persistence with the EventBus client for network transport.
/// Events are guaranteed to be delivered at least once, even across process restarts.
///
/// # Architecture
///
/// ```text
/// ProcessEvent -> WAL (disk) -> EventBusClient -> Broker
///                    |              ^
///                    |              |
///                    v              |
///               BufferedEvent ------+
///                 (memory)      (reconnect)
/// ```
///
/// # Thread Safety
///
/// This struct is designed for single-threaded async usage. The WAL uses internal
/// mutexes for thread safety, but the EventBusClient and buffer are not thread-safe.
pub struct EventBusConnector {
    /// Write-ahead log for event persistence.
    wal: WriteAheadLog,

    /// EventBus client for broker communication (None when disconnected).
    client: Option<EventBusClient>,

    /// In-memory buffer for events when disconnected.
    buffer: VecDeque<BufferedEvent>,

    /// Current total size of buffered events in bytes.
    buffer_size_bytes: usize,

    /// Maximum buffer size in bytes (10MB).
    max_buffer_size: usize,

    /// Whether currently connected to the broker.
    connected: bool,

    /// Channel for sending backpressure signals.
    backpressure_tx: mpsc::Sender<BackpressureSignal>,

    /// Template receiver for backpressure signals (taken once).
    backpressure_rx_template: Option<mpsc::Receiver<BackpressureSignal>>,

    /// Client ID for identification with the broker.
    client_id: String,

    /// Socket configuration for reconnection.
    socket_config: Option<SocketConfig>,

    /// Number of consecutive reconnection failures.
    reconnect_attempts: u32,

    /// Last reconnection attempt time (for backoff).
    last_reconnect_attempt: Option<std::time::Instant>,
}

impl EventBusConnector {
    /// Create a new EventBusConnector with WAL at the specified directory.
    ///
    /// This creates the WAL directory if it doesn't exist and initializes
    /// the connector in a disconnected state. Call [`connect()`](Self::connect)
    /// to establish connection to the broker.
    ///
    /// # Arguments
    ///
    /// * `wal_dir` - Directory path for WAL files
    ///
    /// # Returns
    ///
    /// A new `EventBusConnector` instance ready for connection
    ///
    /// # Errors
    ///
    /// Returns `EventBusConnectorError::Wal` if WAL initialization fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::EventBusConnector;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let connector = EventBusConnector::new(PathBuf::from("/var/lib/procmond/wal")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(wal_dir: PathBuf) -> EventBusConnectorResult<Self> {
        info!(wal_dir = ?wal_dir, "Initializing EventBusConnector");

        let wal = WriteAheadLog::new(wal_dir).await?;

        // Create backpressure channel with small buffer for signals
        let (backpressure_tx, backpressure_rx) = mpsc::channel(16);

        // Generate unique client ID
        let client_id = format!("procmond-{}", uuid::Uuid::new_v4());

        Ok(Self {
            wal,
            client: None,
            buffer: VecDeque::new(),
            buffer_size_bytes: 0,
            max_buffer_size: MAX_BUFFER_SIZE,
            connected: false,
            backpressure_tx,
            backpressure_rx_template: Some(backpressure_rx),
            client_id,
            socket_config: None,
            reconnect_attempts: 0,
            last_reconnect_attempt: None,
        })
    }

    /// Connect to the daemoneye-agent broker.
    ///
    /// Reads the broker socket path from the `DAEMONEYE_BROKER_SOCKET` environment
    /// variable and establishes a connection. If already connected, this is a no-op.
    ///
    /// After successful connection, any events in the WAL that were not yet published
    /// should be replayed using [`replay_wal()`](Self::replay_wal).
    ///
    /// # Errors
    ///
    /// - `EventBusConnectorError::EnvNotSet` if `DAEMONEYE_BROKER_SOCKET` is not set
    /// - `EventBusConnectorError::Connection` if connection to broker fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::EventBusConnector;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // SAFETY: Single-threaded example before any concurrent operations
    /// unsafe { std::env::set_var("DAEMONEYE_BROKER_SOCKET", "/tmp/daemoneye-broker.sock") };
    /// let mut connector = EventBusConnector::new(PathBuf::from("/tmp/wal")).await?;
    /// connector.connect().await?;
    /// assert!(connector.is_connected());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(&mut self) -> EventBusConnectorResult<()> {
        if self.connected {
            debug!("Already connected to broker");
            return Ok(());
        }

        // Get socket path from environment
        let socket_path = std::env::var(BROKER_SOCKET_ENV)
            .map_err(|e| EventBusConnectorError::EnvNotSet(format!("{BROKER_SOCKET_ENV}: {e}")))?;

        info!(socket_path = %socket_path, client_id = %self.client_id, "Connecting to broker");

        // Create socket configuration
        let socket_config = SocketConfig {
            unix_path: socket_path.clone(),
            windows_pipe: DEFAULT_WINDOWS_PIPE.to_owned(),
            connection_limit: 1,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: MAX_BUFFER_SIZE,
            rate_limit_config: None,
        };

        // Store config for potential reconnection
        self.socket_config = Some(socket_config.clone());

        // Create client configuration with reasonable defaults for procmond
        let client_config = ClientConfig {
            max_reconnect_attempts: 5,
            connection_timeout: std::time::Duration::from_secs(10),
            health_check_interval: std::time::Duration::from_secs(30),
            health_check_timeout: std::time::Duration::from_secs(5),
            ..ClientConfig::default()
        };

        // Attempt to connect
        let client = EventBusClient::new(self.client_id.clone(), socket_config, client_config)
            .await
            .map_err(|e| EventBusConnectorError::Connection(e.to_string()))?;

        self.client = Some(client);
        self.connected = true;
        self.reconnect_attempts = 0;
        self.last_reconnect_attempt = None;

        info!(client_id = %self.client_id, "Connected to broker successfully");

        Ok(())
    }

    /// Attempt to reconnect to the broker with exponential backoff.
    ///
    /// This method is called automatically when publishing detects a disconnection.
    /// It uses exponential backoff to avoid overwhelming the broker during outages.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if reconnection succeeded
    /// - `Ok(false)` if reconnection was skipped due to backoff
    /// - `Err` if reconnection was attempted but failed
    ///
    /// # Backoff Strategy
    ///
    /// - Initial delay: 100ms
    /// - Maximum delay: 30 seconds
    /// - Multiplier: 2x per attempt
    /// - Jitter: ±10%
    async fn try_reconnect(&mut self) -> EventBusConnectorResult<bool> {
        const MIN_BACKOFF_MS: u64 = 100;
        const MAX_BACKOFF_MS: u64 = 30_000;
        const BACKOFF_MULTIPLIER: u32 = 2;

        // Check if we have socket config (required for reconnection)
        if self.socket_config.is_none() {
            debug!("Cannot reconnect: no socket config stored");
            return Ok(false);
        }

        // Calculate backoff delay
        let base_delay_ms = MIN_BACKOFF_MS.saturating_mul(
            BACKOFF_MULTIPLIER
                .saturating_pow(self.reconnect_attempts)
                .into(),
        );
        let delay_ms = base_delay_ms.min(MAX_BACKOFF_MS);

        // Check if enough time has passed since last attempt
        if let Some(last_attempt) = self.last_reconnect_attempt {
            let elapsed = last_attempt.elapsed();
            if elapsed.as_millis() < u128::from(delay_ms) {
                // Safe: elapsed_ms is capped at delay_ms which fits in u64
                let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
                debug!(
                    delay_remaining_ms = delay_ms.saturating_sub(elapsed_ms),
                    "Reconnection skipped due to backoff"
                );
                return Ok(false);
            }
        }

        // Update attempt tracking
        self.last_reconnect_attempt = Some(std::time::Instant::now());
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);

        info!(
            attempt = self.reconnect_attempts,
            delay_ms = delay_ms,
            "Attempting reconnection to broker"
        );

        // Attempt reconnection using stored config
        // Safe: we checked socket_config.is_none() above and returned early
        let Some(socket_config) = self.socket_config.clone() else {
            // Should never reach here due to early return above
            return Ok(false);
        };
        let client_config = ClientConfig {
            max_reconnect_attempts: 5,
            connection_timeout: std::time::Duration::from_secs(10),
            health_check_interval: std::time::Duration::from_secs(30),
            health_check_timeout: std::time::Duration::from_secs(5),
            ..ClientConfig::default()
        };

        match EventBusClient::new(self.client_id.clone(), socket_config, client_config).await {
            Ok(client) => {
                self.client = Some(client);
                self.connected = true;
                self.reconnect_attempts = 0;
                self.last_reconnect_attempt = None;

                info!(client_id = %self.client_id, "Reconnected to broker successfully");

                // Replay WAL after reconnection
                if let Err(e) = self.replay_wal().await {
                    warn!(error = %e, "Failed to replay WAL after reconnection");
                }

                Ok(true)
            }
            Err(e) => {
                warn!(
                    attempt = self.reconnect_attempts,
                    error = %e,
                    "Reconnection attempt failed"
                );
                Err(EventBusConnectorError::Connection(format!(
                    "Reconnection failed (attempt {}): {}",
                    self.reconnect_attempts, e
                )))
            }
        }
    }

    /// Publish a process event with durability guarantees.
    ///
    /// This method implements the following flow:
    /// 1. Write event to WAL (durability guarantee)
    /// 2. If connected: publish to broker via EventBusClient
    ///    - On success: mark WAL entry as published
    /// 3. If disconnected: add to in-memory buffer
    /// 4. Check buffer level for backpressure signals
    ///
    /// # Arguments
    ///
    /// * `event` - Process event to publish
    /// * `event_type` - Type of event for topic routing
    ///
    /// # Returns
    ///
    /// The WAL sequence number assigned to this event
    ///
    /// # Errors
    ///
    /// - `EventBusConnectorError::Wal` if WAL write fails
    /// - `EventBusConnectorError::BufferOverflow` if disconnected and buffer is full
    /// - `EventBusConnectorError::EventBus` if publish fails (event is still buffered)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::{EventBusConnector, ProcessEventType};
    /// use collector_core::event::ProcessEvent;
    /// use std::path::PathBuf;
    /// use std::time::SystemTime;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut connector = EventBusConnector::new(PathBuf::from("/tmp/wal")).await?;
    /// connector.connect().await?;
    ///
    /// let event = ProcessEvent {
    ///     pid: 1234,
    ///     ppid: None,
    ///     name: "test".to_string(),
    ///     executable_path: None,
    ///     command_line: vec![],
    ///     start_time: None,
    ///     cpu_usage: None,
    ///     memory_usage: None,
    ///     executable_hash: None,
    ///     user_id: None,
    ///     accessible: true,
    ///     file_exists: true,
    ///     timestamp: SystemTime::now(),
    ///     platform_metadata: None,
    /// };
    ///
    /// let sequence = connector.publish(event, ProcessEventType::Start).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn publish(
        &mut self,
        event: ProcessEvent,
        event_type: ProcessEventType,
    ) -> EventBusConnectorResult<u64> {
        let topic = event_type.topic().to_owned();
        let event_type_str = event_type.to_type_string().to_owned();

        // Step 1: Write to WAL for durability (with event type for replay)
        let sequence = self
            .wal
            .write_with_type(event.clone(), event_type_str)
            .await?;

        debug!(
            sequence = sequence,
            topic = %topic,
            pid = event.pid,
            "Event written to WAL"
        );

        // Step 2: Ensure connected (attempt reconnection if needed)
        if !self.connected
            && let Err(e) = self.try_reconnect().await
        {
            debug!(
                error = %e,
                "Reconnection attempt failed, will buffer event"
            );
        }

        // Step 3: Try to publish or buffer
        if self.connected {
            self.try_publish_or_buffer(sequence, event, topic).await?;
        } else {
            self.buffer_event(sequence, event, topic)?;
        }

        Ok(sequence)
    }

    /// Attempt to publish an event to the broker, buffering on failure.
    ///
    /// If publish succeeds, marks the event as published in WAL.
    /// If publish fails, disconnects and buffers the event.
    async fn try_publish_or_buffer(
        &mut self,
        sequence: u64,
        event: ProcessEvent,
        topic: String,
    ) -> EventBusConnectorResult<()> {
        match self.publish_to_broker(&event, &topic).await {
            Ok(()) => {
                // Successfully published - mark as published in WAL
                self.mark_published_with_warning(sequence).await;
                Ok(())
            }
            Err(e) => {
                // Publish failed - disconnect and buffer
                warn!(
                    sequence = sequence,
                    error = %e,
                    "Failed to publish event, buffering"
                );
                self.connected = false;
                self.buffer_event(sequence, event, topic)
            }
        }
    }

    /// Mark an event as published in WAL, logging warnings on failure.
    ///
    /// Failures are non-fatal since WAL cleanup will happen eventually.
    async fn mark_published_with_warning(&self, sequence: u64) {
        if let Err(e) = self.wal.mark_published(sequence).await {
            warn!(
                sequence = sequence,
                error = %e,
                "Failed to mark event as published in WAL"
            );
        }
    }

    /// Take ownership of the backpressure signal receiver.
    ///
    /// This can only be called once. Subsequent calls return `None`.
    /// The receiver should be monitored by upstream producers to implement
    /// backpressure handling.
    ///
    /// # Returns
    ///
    /// The backpressure signal receiver, or `None` if already taken
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::{EventBusConnector, BackpressureSignal};
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut connector = EventBusConnector::new(PathBuf::from("/tmp/wal")).await?;
    /// let mut bp_rx = connector.take_backpressure_receiver()
    ///     .expect("First call should succeed");
    ///
    /// // Monitor in a separate task
    /// tokio::spawn(async move {
    ///     while let Some(signal) = bp_rx.recv().await {
    ///         match signal {
    ///             BackpressureSignal::Activated => println!("Slow down!"),
    ///             BackpressureSignal::Released => println!("Resume normal rate"),
    ///             _ => {} // Handle future variants
    ///         }
    ///     }
    /// });
    ///
    /// // Second call returns None
    /// assert!(connector.take_backpressure_receiver().is_none());
    /// # Ok(())
    /// # }
    /// ```
    #[allow(clippy::missing_const_for_fn)] // take() is not const
    pub fn take_backpressure_receiver(&mut self) -> Option<mpsc::Receiver<BackpressureSignal>> {
        self.backpressure_rx_template.take()
    }

    /// Replay unpublished events from the WAL after reconnection.
    ///
    /// This should be called after a successful [`connect()`](Self::connect)
    /// following a disconnection or restart. It reads all events from the WAL
    /// and attempts to publish those that haven't been marked as published.
    ///
    /// # Returns
    ///
    /// The number of events successfully replayed
    ///
    /// # Errors
    ///
    /// Returns `EventBusConnectorError::Wal` if WAL replay fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::EventBusConnector;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut connector = EventBusConnector::new(PathBuf::from("/tmp/wal")).await?;
    /// connector.connect().await?;
    ///
    /// // Replay any events from previous run
    /// let replayed = connector.replay_wal().await?;
    /// println!("Replayed {} events from WAL", replayed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn replay_wal(&mut self) -> EventBusConnectorResult<usize> {
        info!("Starting WAL replay");

        // Use replay_entries to get full entries with sequences and event types
        let entries = self.wal.replay_entries().await?;
        let total = entries.len();

        if total == 0 {
            info!("No events to replay from WAL");
            return Ok(0);
        }

        info!(event_count = total, "Replaying events from WAL");

        let mut replayed = 0_usize;
        let mut last_successful_sequence = 0_u64;

        for entry in entries {
            // Get the topic from the stored event type, or default to Start for legacy entries
            let event_type = entry
                .event_type
                .as_ref()
                .map_or(ProcessEventType::Start, |s| {
                    ProcessEventType::from_type_string(s)
                });
            let topic = event_type.topic();

            if self.connected {
                match self.publish_to_broker(&entry.event, topic).await {
                    Ok(()) => {
                        replayed = replayed.saturating_add(1);
                        // Track the actual WAL sequence for proper cleanup
                        last_successful_sequence = entry.sequence;
                    }
                    Err(e) => {
                        warn!(
                            sequence = entry.sequence,
                            error = %e,
                            "Failed to replay event, stopping replay"
                        );
                        self.connected = false;
                        // Buffer this event and remaining ones
                        let buffered_event =
                            BufferedEvent::new(entry.sequence, entry.event, topic.to_owned());
                        if self.add_to_buffer(buffered_event).is_err() {
                            warn!("Buffer full during WAL replay, some events may be lost");
                        }
                        break;
                    }
                }
            } else {
                // Lost connection during replay - buffer remaining events
                let buffered_event =
                    BufferedEvent::new(entry.sequence, entry.event, topic.to_owned());
                if self.add_to_buffer(buffered_event).is_err() {
                    warn!("Buffer full during WAL replay, some events may be lost");
                    break;
                }
            }
        }

        // Mark replayed events as published in WAL using actual sequence numbers
        if last_successful_sequence > 0
            && let Err(e) = self.wal.mark_published(last_successful_sequence).await
        {
            warn!(
                sequence = last_successful_sequence,
                error = %e,
                "Failed to mark replayed events as published"
            );
        }

        // Also flush the in-memory buffer
        let buffer_flushed = self.flush_buffer().await;

        info!(
            wal_replayed = replayed,
            buffer_flushed = buffer_flushed,
            "WAL replay completed"
        );

        Ok(replayed.saturating_add(buffer_flushed))
    }

    /// Gracefully shutdown the connector.
    ///
    /// This attempts to flush any buffered events before closing the connection.
    /// The WAL is not affected and can be replayed on next startup.
    ///
    /// # Note
    ///
    /// Client shutdown errors are logged but not propagated, as shutdown is
    /// best-effort. The connector will be marked as disconnected regardless
    /// of whether the underlying client shutdown succeeds.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use procmond::event_bus_connector::EventBusConnector;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut connector = EventBusConnector::new(PathBuf::from("/tmp/wal")).await?;
    /// connector.connect().await?;
    ///
    /// // ... use connector ...
    ///
    /// connector.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(&mut self) -> EventBusConnectorResult<()> {
        info!("Shutting down EventBusConnector");

        // Try to flush buffer before shutdown
        if self.connected {
            let flushed = self.flush_buffer().await;
            debug!(flushed = flushed, "Flushed buffer before shutdown");
        }

        // Close the client connection
        if let Some(client) = self.client.take()
            && let Err(e) = client.shutdown().await
        {
            error!(error = %e, "Error during client shutdown");
        }

        self.connected = false;

        info!(
            buffered_events = self.buffer.len(),
            buffer_bytes = self.buffer_size_bytes,
            "EventBusConnector shutdown complete"
        );

        Ok(())
    }

    /// Check if currently connected to the broker.
    ///
    /// Note that this reflects the last known connection state. The actual
    /// connection may have been lost since the last operation.
    ///
    /// # Returns
    ///
    /// `true` if connected, `false` otherwise
    pub const fn is_connected(&self) -> bool {
        self.connected
    }

    /// Get the current buffer usage as a percentage (0-100).
    ///
    /// This can be used for monitoring and alerting on buffer pressure.
    ///
    /// # Returns
    ///
    /// Buffer usage percentage (0-100)
    #[allow(clippy::arithmetic_side_effects)] // Division by non-zero is safe
    #[allow(clippy::integer_division)] // Integer precision is acceptable for percentage
    pub fn buffer_usage_percent(&self) -> u8 {
        if self.max_buffer_size == 0 {
            return 100;
        }

        let usage = self.buffer_size_bytes.saturating_mul(100) / self.max_buffer_size;

        // Clamp to u8 range
        #[allow(clippy::as_conversions)]
        // Safe: result is 0-100 after division by max_buffer_size
        {
            usage.min(100) as u8
        }
    }

    /// Get current buffer size in bytes.
    pub const fn buffer_size_bytes(&self) -> usize {
        self.buffer_size_bytes
    }

    /// Get number of buffered events.
    pub fn buffered_event_count(&self) -> usize {
        self.buffer.len()
    }

    // === Private Helper Methods ===

    /// Publish an event to the broker.
    async fn publish_to_broker(
        &self,
        event: &ProcessEvent,
        topic: &str,
    ) -> EventBusConnectorResult<()> {
        let client = self.client.as_ref().ok_or_else(|| {
            EventBusConnectorError::Connection("Not connected to broker".to_owned())
        })?;

        // Convert collector_core::ProcessEvent to eventbus ProcessEvent
        let eventbus_event = Self::convert_to_eventbus_event(event);
        let collection_event = EventBusCollectionEvent::Process(eventbus_event);

        // Generate correlation ID
        let correlation_id = uuid::Uuid::new_v4().to_string();

        client
            .publish(topic, collection_event, Some(correlation_id))
            .await
            .map_err(|e| EventBusConnectorError::EventBus(e.to_string()))?;

        debug!(topic = %topic, pid = event.pid, "Event published to broker");

        Ok(())
    }

    /// Convert collector_core ProcessEvent to eventbus ProcessEvent.
    fn convert_to_eventbus_event(event: &ProcessEvent) -> EventBusProcessEvent {
        use std::collections::HashMap;

        EventBusProcessEvent {
            pid: event.pid,
            name: event.name.clone(),
            command_line: event.command_line.join(" ").into(),
            executable_path: event.executable_path.clone(),
            ppid: event.ppid,
            start_time: event.start_time,
            metadata: HashMap::new(),
        }
    }

    /// Buffer an event when disconnected.
    fn buffer_event(
        &mut self,
        sequence: u64,
        event: ProcessEvent,
        topic: String,
    ) -> EventBusConnectorResult<()> {
        let buffered = BufferedEvent::new(sequence, event, topic);
        self.add_to_buffer(buffered)
    }

    /// Add a buffered event to the queue with overflow protection.
    fn add_to_buffer(&mut self, event: BufferedEvent) -> EventBusConnectorResult<()> {
        // Check if adding would exceed max buffer size
        let new_size = self.buffer_size_bytes.saturating_add(event.size_bytes);
        if new_size > self.max_buffer_size {
            error!(
                current_size = self.buffer_size_bytes,
                event_size = event.size_bytes,
                max_size = self.max_buffer_size,
                "Buffer overflow - rejecting event"
            );
            return Err(EventBusConnectorError::BufferOverflow);
        }

        // Track previous usage for backpressure detection
        let previous_usage = self.buffer_usage_percent();

        // Add to buffer
        self.buffer_size_bytes = new_size;
        self.buffer.push_back(event);

        // Check for backpressure threshold crossing
        let current_usage = self.buffer_usage_percent();
        self.check_backpressure(previous_usage, current_usage);

        debug!(
            buffered_events = self.buffer.len(),
            buffer_bytes = self.buffer_size_bytes,
            usage_percent = current_usage,
            "Event buffered"
        );

        Ok(())
    }

    /// Flush the in-memory buffer to the broker.
    async fn flush_buffer(&mut self) -> usize {
        if !self.connected || self.buffer.is_empty() {
            return 0;
        }

        let mut flushed = 0_usize;
        let previous_usage = self.buffer_usage_percent();

        while let Some(buffered) = self.buffer.pop_front() {
            match self
                .publish_to_broker(&buffered.event, &buffered.topic)
                .await
            {
                Ok(()) => {
                    self.buffer_size_bytes =
                        self.buffer_size_bytes.saturating_sub(buffered.size_bytes);
                    flushed = flushed.saturating_add(1);

                    // Mark as published in WAL
                    if let Err(e) = self.wal.mark_published(buffered.sequence).await {
                        warn!(
                            sequence = buffered.sequence,
                            error = %e,
                            "Failed to mark buffered event as published"
                        );
                    }
                }
                Err(e) => {
                    // Put event back and stop flushing
                    warn!(error = %e, "Failed to flush buffered event");
                    self.buffer.push_front(buffered);
                    self.connected = false;
                    break;
                }
            }
        }

        // Check for backpressure release
        let current_usage = self.buffer_usage_percent();
        self.check_backpressure(previous_usage, current_usage);

        flushed
    }

    /// Check and emit backpressure signals based on buffer usage.
    ///
    /// Signals are best-effort - if the receiver is dropped or the channel is full,
    /// the failure is logged at debug level and processing continues.
    fn check_backpressure(&self, previous_usage: u8, current_usage: u8) {
        const HIGH_WATER_MARK: u8 = 70;
        const LOW_WATER_MARK: u8 = 50;

        // Check for activation (crossing above high water mark)
        if previous_usage < HIGH_WATER_MARK && current_usage >= HIGH_WATER_MARK {
            if let Err(e) = self.backpressure_tx.try_send(BackpressureSignal::Activated) {
                debug!(
                    error = %e,
                    usage = current_usage,
                    "Failed to send backpressure activation signal (receiver may be dropped)"
                );
            }
            info!(usage = current_usage, "Backpressure activated");
        }

        // Check for release (crossing below low water mark)
        if previous_usage >= LOW_WATER_MARK && current_usage < LOW_WATER_MARK {
            if let Err(e) = self.backpressure_tx.try_send(BackpressureSignal::Released) {
                debug!(
                    error = %e,
                    usage = current_usage,
                    "Failed to send backpressure release signal (receiver may be dropped)"
                );
            }
            info!(usage = current_usage, "Backpressure released");
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::wildcard_enum_match_arm,
    clippy::equatable_if_let,
    clippy::integer_division,
    clippy::as_conversions
)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Create a test process event with specified PID.
    fn create_test_event(pid: u32) -> ProcessEvent {
        ProcessEvent {
            pid,
            ppid: Some(1),
            name: format!("test_process_{pid}"),
            executable_path: Some(format!("/usr/bin/test_{pid}")),
            command_line: vec!["test".to_owned(), "--arg".to_owned()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(5.0),
            memory_usage: Some(1024 * 1024),
            executable_hash: Some("abc123".to_owned()),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }
    }

    #[tokio::test]
    async fn test_connector_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        assert!(!connector.is_connected());
        assert_eq!(connector.buffer_usage_percent(), 0);
        assert_eq!(connector.buffered_event_count(), 0);
    }

    #[tokio::test]
    async fn test_connect_fails_when_env_not_set() {
        // This test verifies behavior when DAEMONEYE_BROKER_SOCKET is not set.
        // We check by looking up the env var - if it's not set, we expect EnvNotSet.
        // If it IS set (e.g., in CI), we expect a different error (Connection).

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let result = connector.connect().await;

        // Connect should fail either because env var is not set or because
        // there's no broker listening
        assert!(result.is_err());

        match result.unwrap_err() {
            EventBusConnectorError::EnvNotSet(var) => {
                // Expected when env var is not set
                assert!(var.contains(BROKER_SOCKET_ENV));
            }
            EventBusConnectorError::Connection(_) => {
                // Expected when env var IS set but no broker is running
                // This is also a valid test outcome
            }
            other => panic!("Expected EnvNotSet or Connection error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_publish_while_disconnected() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let event = create_test_event(1234);
        let result = connector.publish(event, ProcessEventType::Start).await;

        // Should succeed by writing to WAL and buffering
        assert!(result.is_ok());
        let sequence = result.unwrap();
        assert_eq!(sequence, 1);

        // Event should be buffered
        assert_eq!(connector.buffered_event_count(), 1);
        // Buffer size should be non-zero (percentage may round to 0 for small events
        // relative to 10MB max buffer)
        assert!(connector.buffer_size_bytes() > 0);
    }

    #[tokio::test]
    async fn test_buffer_overflow_protection() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Set a very small buffer for testing
        connector.max_buffer_size = 500;

        // First event should succeed
        let event1 = create_test_event(1);
        let result1 = connector.publish(event1, ProcessEventType::Start).await;
        assert!(result1.is_ok());

        // Keep adding events until overflow
        let mut overflow_occurred = false;
        for i in 2..=100 {
            let event = create_test_event(i);
            if let Err(EventBusConnectorError::BufferOverflow) =
                connector.publish(event, ProcessEventType::Start).await
            {
                overflow_occurred = true;
                break;
            }
        }

        assert!(overflow_occurred, "Buffer overflow should have occurred");
    }

    #[tokio::test]
    async fn test_backpressure_receiver() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // First call should succeed
        let rx = connector.take_backpressure_receiver();
        assert!(rx.is_some());

        // Second call should return None
        let rx2 = connector.take_backpressure_receiver();
        assert!(rx2.is_none());
    }

    #[tokio::test]
    async fn test_process_event_type_topics() {
        assert_eq!(ProcessEventType::Start.topic(), "events.process.start");
        assert_eq!(ProcessEventType::Stop.topic(), "events.process.stop");
        assert_eq!(ProcessEventType::Modify.topic(), "events.process.modify");
    }

    #[tokio::test]
    async fn test_buffered_event_size_estimation() {
        let event = create_test_event(1234);
        let topic = "events.process.start".to_owned();
        let buffered = BufferedEvent::new(1, event, topic);

        // Size should be reasonable (not zero, not huge)
        assert!(buffered.size_bytes > 50);
        assert!(buffered.size_bytes < 10000);
    }

    #[tokio::test]
    async fn test_buffer_usage_calculation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        assert_eq!(connector.buffer_usage_percent(), 0);

        // Set small buffer for predictable testing
        connector.max_buffer_size = 1000;

        // Add event to buffer directly for testing
        let event = create_test_event(1);
        let buffered = BufferedEvent::new(1, event, "test".to_owned());
        let event_size = buffered.size_bytes;
        connector.buffer.push_back(buffered);
        connector.buffer_size_bytes = event_size;

        let usage = connector.buffer_usage_percent();
        // Usage should be event_size * 100 / 1000
        let expected = (event_size * 100 / 1000).min(100);
        assert_eq!(usage, expected as u8);
    }

    #[tokio::test]
    async fn test_shutdown_while_disconnected() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Should succeed even when not connected
        let result = connector.shutdown().await;
        assert!(result.is_ok());
        assert!(!connector.is_connected());
    }

    #[tokio::test]
    async fn test_event_conversion() {
        let event = create_test_event(1234);
        let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event);

        assert_eq!(eventbus_event.pid, 1234);
        assert_eq!(eventbus_event.name, "test_process_1234");
        assert_eq!(eventbus_event.ppid, Some(1));
        assert!(eventbus_event.executable_path.is_some());
    }

    #[tokio::test]
    async fn test_wal_persistence_across_connector_instances() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // First instance - write some events
        {
            let mut connector = EventBusConnector::new(wal_path.clone())
                .await
                .expect("Failed to create connector");

            for i in 1..=5 {
                let event = create_test_event(i);
                connector
                    .publish(event, ProcessEventType::Start)
                    .await
                    .expect("Failed to publish");
            }
        } // Connector dropped

        // Second instance - should be able to replay events
        {
            let connector = EventBusConnector::new(wal_path.clone())
                .await
                .expect("Failed to create connector");

            // WAL should have events from first instance
            let events = connector.wal.replay().await.expect("Failed to replay WAL");
            assert_eq!(events.len(), 5);
        }
    }

    // ============================================================
    // Additional tests for comprehensive coverage
    // ============================================================

    // --- ProcessEventType tests ---

    #[test]
    fn test_process_event_type_to_type_string() {
        assert_eq!(ProcessEventType::Start.to_type_string(), "start");
        assert_eq!(ProcessEventType::Stop.to_type_string(), "stop");
        assert_eq!(ProcessEventType::Modify.to_type_string(), "modify");
    }

    #[test]
    fn test_process_event_type_from_type_string() {
        assert_eq!(
            ProcessEventType::from_type_string("start"),
            ProcessEventType::Start
        );
        assert_eq!(
            ProcessEventType::from_type_string("stop"),
            ProcessEventType::Stop
        );
        assert_eq!(
            ProcessEventType::from_type_string("modify"),
            ProcessEventType::Modify
        );
    }

    #[test]
    fn test_process_event_type_from_type_string_unknown() {
        // Unknown strings should default to Start
        assert_eq!(
            ProcessEventType::from_type_string("unknown"),
            ProcessEventType::Start
        );
        assert_eq!(
            ProcessEventType::from_type_string(""),
            ProcessEventType::Start
        );
        assert_eq!(
            ProcessEventType::from_type_string("START"),
            ProcessEventType::Start
        );
    }

    #[test]
    fn test_process_event_type_debug_clone_copy() {
        let event_type = ProcessEventType::Start;
        let cloned = event_type;
        assert_eq!(event_type, cloned);
        assert_eq!(format!("{:?}", event_type), "Start");
    }

    #[test]
    fn test_backpressure_signal_debug_clone_copy() {
        let signal = BackpressureSignal::Activated;
        let cloned = signal;
        assert_eq!(signal, cloned);
        assert_eq!(format!("{:?}", signal), "Activated");

        let signal2 = BackpressureSignal::Released;
        assert_eq!(format!("{:?}", signal2), "Released");
    }

    // --- BufferedEvent tests ---

    #[test]
    fn test_buffered_event_with_minimal_event() {
        let event = ProcessEvent {
            pid: 1,
            ppid: None,
            name: "min".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let buffered = BufferedEvent::new(1, event, "test".to_owned());

        // Size should still include base overhead
        assert!(buffered.size_bytes >= 64);
        assert_eq!(buffered.sequence, 1);
        assert_eq!(buffered.topic, "test");
    }

    #[test]
    fn test_buffered_event_with_platform_metadata() {
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "key1".to_owned(),
            serde_json::Value::String("value1".to_owned()),
        );
        metadata.insert("key2".to_owned(), serde_json::Value::Number(42.into()));

        let event = ProcessEvent {
            pid: 1,
            ppid: Some(1),
            name: "test".to_owned(),
            executable_path: Some("/bin/test".to_owned()),
            command_line: vec!["test".to_owned()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024),
            executable_hash: Some("hash".to_owned()),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: Some(serde_json::Value::Object(metadata)),
        };

        let buffered = BufferedEvent::new(1, event, "test".to_owned());

        // Size should include metadata string length
        assert!(buffered.size_bytes > 100);
    }

    #[test]
    fn test_buffered_event_debug() {
        let event = create_test_event(123);
        let buffered = BufferedEvent::new(5, event, "topic".to_owned());

        let debug_str = format!("{:?}", buffered);
        assert!(debug_str.contains("BufferedEvent"));
        assert!(debug_str.contains("sequence: 5"));
    }

    // --- Buffer management tests ---

    #[tokio::test]
    async fn test_buffer_usage_percent_with_zero_max() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Set zero max buffer (edge case)
        connector.max_buffer_size = 0;

        // Should return 100% when max is zero
        assert_eq!(connector.buffer_usage_percent(), 100);
    }

    #[tokio::test]
    async fn test_buffer_usage_percent_exact_100() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Set buffer size equal to max
        connector.max_buffer_size = 1000;
        connector.buffer_size_bytes = 1000;

        assert_eq!(connector.buffer_usage_percent(), 100);
    }

    #[tokio::test]
    async fn test_buffer_usage_percent_over_100_clamped() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Set buffer size greater than max (shouldn't happen normally, but test clamping)
        connector.max_buffer_size = 1000;
        connector.buffer_size_bytes = 2000;

        // Should be clamped to 100
        assert_eq!(connector.buffer_usage_percent(), 100);
    }

    // --- Backpressure tests ---

    #[tokio::test]
    async fn test_backpressure_activation_at_70_percent() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let mut rx = connector.take_backpressure_receiver().unwrap();

        // Set small buffer for predictable percentages
        connector.max_buffer_size = 1000;

        // Simulate crossing 70% threshold
        connector.check_backpressure(69, 70);

        // Should receive activation signal
        let signal = rx.try_recv();
        assert!(signal.is_ok());
        assert_eq!(signal.unwrap(), BackpressureSignal::Activated);
    }

    #[tokio::test]
    async fn test_backpressure_release_at_50_percent() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let mut rx = connector.take_backpressure_receiver().unwrap();

        // Simulate crossing below 50% threshold
        connector.check_backpressure(50, 49);

        // Should receive release signal
        let signal = rx.try_recv();
        assert!(signal.is_ok());
        assert_eq!(signal.unwrap(), BackpressureSignal::Released);
    }

    #[tokio::test]
    async fn test_backpressure_no_signal_when_not_crossing_threshold() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let mut rx = connector.take_backpressure_receiver().unwrap();

        // Stay below 70% - no activation
        connector.check_backpressure(60, 65);
        assert!(rx.try_recv().is_err());

        // Stay above 50% - no release
        connector.check_backpressure(55, 60);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_backpressure_signal_with_dropped_receiver() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Take and immediately drop the receiver
        let rx = connector.take_backpressure_receiver().unwrap();
        drop(rx);

        // Should not panic when trying to send signal with dropped receiver
        connector.check_backpressure(69, 70);
        connector.check_backpressure(50, 49);
    }

    #[tokio::test]
    async fn test_backpressure_integration_with_publish() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let mut rx = connector.take_backpressure_receiver().unwrap();

        // Calculate event size to set appropriate buffer limit
        let test_event = create_test_event(1);
        let event_size = BufferedEvent::estimate_size(&test_event, "events.process.start");

        // Set buffer such that 2 events = ~70% (so 3 events crosses threshold)
        // If 70% = 2 events, then 100% = 2/0.7 = ~2.86 events
        // So max_buffer_size = event_size * 3 should mean 2 events = 66%, 3 events = 100%
        // For 2 events to be exactly 70%, max = 2 * event_size / 0.7 = 2.86 * event_size
        let max_buffer = (event_size * 100) / 70 * 2 + 1; // About 2.86 events
        connector.max_buffer_size = max_buffer;

        // Publish events until we cross 70%
        let mut activation_received = false;
        for i in 1..=10 {
            let event = create_test_event(i);
            let result = connector.publish(event, ProcessEventType::Start).await;

            if result.is_err() {
                break; // Buffer overflow
            }

            // Check for backpressure signal
            if let Ok(signal) = rx.try_recv() {
                if signal == BackpressureSignal::Activated {
                    activation_received = true;
                    break;
                }
            }
        }

        // Should have received activation signal or exceeded threshold
        let usage = connector.buffer_usage_percent();
        assert!(
            activation_received || usage >= 70,
            "Expected activation signal or usage >= 70%, got: activation={}, usage={}%",
            activation_received,
            usage
        );
    }

    // --- Publish with different event types ---

    #[tokio::test]
    async fn test_publish_stop_event() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let event = create_test_event(1);
        let result = connector.publish(event, ProcessEventType::Stop).await;

        assert!(result.is_ok());
        assert_eq!(connector.buffered_event_count(), 1);

        // Check that the buffered event has the correct topic
        let buffered = &connector.buffer[0];
        assert_eq!(buffered.topic, "events.process.stop");
    }

    #[tokio::test]
    async fn test_publish_modify_event() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let event = create_test_event(1);
        let result = connector.publish(event, ProcessEventType::Modify).await;

        assert!(result.is_ok());
        assert_eq!(connector.buffered_event_count(), 1);

        // Check that the buffered event has the correct topic
        let buffered = &connector.buffer[0];
        assert_eq!(buffered.topic, "events.process.modify");
    }

    // --- Sequence numbering tests ---

    #[tokio::test]
    async fn test_publish_sequence_numbering() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        for i in 1..=5 {
            let event = create_test_event(i);
            let seq = connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Failed to publish");

            assert_eq!(seq, u64::from(i));
        }
    }

    // --- add_to_buffer tests ---

    #[tokio::test]
    async fn test_add_to_buffer_tracks_size() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        connector.max_buffer_size = 10000;

        let event1 = create_test_event(1);
        let buffered1 = BufferedEvent::new(1, event1, "test".to_owned());
        let size1 = buffered1.size_bytes;

        connector.add_to_buffer(buffered1).expect("Should succeed");

        assert_eq!(connector.buffer_size_bytes, size1);
        assert_eq!(connector.buffered_event_count(), 1);

        let event2 = create_test_event(2);
        let buffered2 = BufferedEvent::new(2, event2, "test".to_owned());
        let size2 = buffered2.size_bytes;

        connector.add_to_buffer(buffered2).expect("Should succeed");

        assert_eq!(connector.buffer_size_bytes, size1 + size2);
        assert_eq!(connector.buffered_event_count(), 2);
    }

    #[tokio::test]
    async fn test_add_to_buffer_rejects_when_full() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Create event and measure its size
        let event = create_test_event(1);
        let size = BufferedEvent::estimate_size(&event, "test");

        // Set max to slightly less than one event
        connector.max_buffer_size = size - 1;

        let buffered = BufferedEvent::new(1, event, "test".to_owned());
        let result = connector.add_to_buffer(buffered);

        assert!(matches!(
            result,
            Err(EventBusConnectorError::BufferOverflow)
        ));
    }

    // --- try_reconnect tests ---

    #[tokio::test]
    async fn test_try_reconnect_without_socket_config() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Without calling connect(), there's no socket_config
        let result = connector.try_reconnect().await;

        // Should return Ok(false) when no socket config
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_try_reconnect_backoff_skips_early_attempts() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Manually set up reconnection state
        connector.socket_config = Some(SocketConfig {
            unix_path: "/nonexistent/socket".to_owned(),
            windows_pipe: DEFAULT_WINDOWS_PIPE.to_owned(),
            connection_limit: 1,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: MAX_BUFFER_SIZE,
            rate_limit_config: None,
        });
        connector.last_reconnect_attempt = Some(std::time::Instant::now());
        connector.reconnect_attempts = 1;

        // Immediate retry should be skipped due to backoff
        let result = connector.try_reconnect().await;

        // Should return Ok(false) because backoff period hasn't elapsed
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_try_reconnect_increments_attempts() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Set up socket config with invalid path
        connector.socket_config = Some(SocketConfig {
            unix_path: "/nonexistent/socket/path/that/wont/work".to_owned(),
            windows_pipe: DEFAULT_WINDOWS_PIPE.to_owned(),
            connection_limit: 1,
            #[cfg(target_os = "freebsd")]
            freebsd_path: None,
            auth_token: None,
            per_client_byte_limit: MAX_BUFFER_SIZE,
            rate_limit_config: None,
        });

        assert_eq!(connector.reconnect_attempts, 0);

        // First attempt should increment counter
        let _ = connector.try_reconnect().await;

        assert_eq!(connector.reconnect_attempts, 1);
        assert!(connector.last_reconnect_attempt.is_some());
    }

    // --- replay_wal tests ---

    #[tokio::test]
    async fn test_replay_wal_empty() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Replay on empty WAL should return 0
        let result = connector.replay_wal().await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_replay_wal_with_events_while_disconnected() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Write some events while disconnected (they go to buffer)
        for i in 1..=3 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Failed to publish");
        }

        // Replay WAL while still disconnected - events should be buffered
        let replayed = connector.replay_wal().await.expect("Failed to replay");

        // Since we're disconnected, events should be buffered, not replayed to broker
        // The replay count might be 0 (nothing published) + buffer flush (0 since disconnected)
        assert_eq!(replayed, 0);
    }

    #[tokio::test]
    async fn test_replay_wal_preserves_event_types() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // First instance - write events with different types
        {
            let mut connector = EventBusConnector::new(wal_path.clone())
                .await
                .expect("Failed to create connector");

            let event1 = create_test_event(1);
            connector
                .publish(event1, ProcessEventType::Start)
                .await
                .expect("Failed to publish");

            let event2 = create_test_event(2);
            connector
                .publish(event2, ProcessEventType::Stop)
                .await
                .expect("Failed to publish");

            let event3 = create_test_event(3);
            connector
                .publish(event3, ProcessEventType::Modify)
                .await
                .expect("Failed to publish");
        }

        // Second instance - verify event types are preserved in WAL
        {
            let connector = EventBusConnector::new(wal_path.clone())
                .await
                .expect("Failed to create connector");

            let entries = connector
                .wal
                .replay_entries()
                .await
                .expect("Failed to replay");
            assert_eq!(entries.len(), 3);

            assert_eq!(entries[0].event_type.as_deref(), Some("start"));
            assert_eq!(entries[1].event_type.as_deref(), Some("stop"));
            assert_eq!(entries[2].event_type.as_deref(), Some("modify"));
        }
    }

    // --- flush_buffer tests ---

    #[tokio::test]
    async fn test_flush_buffer_when_empty() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Pretend we're connected
        connector.connected = true;

        // Flush empty buffer should return 0
        let flushed = connector.flush_buffer().await;
        assert_eq!(flushed, 0);
    }

    #[tokio::test]
    async fn test_flush_buffer_when_disconnected() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Add events to buffer
        let event = create_test_event(1);
        let buffered = BufferedEvent::new(1, event, "test".to_owned());
        connector.buffer.push_back(buffered);
        connector.buffer_size_bytes = 100;

        // Flush while disconnected should return 0
        let flushed = connector.flush_buffer().await;
        assert_eq!(flushed, 0);
        assert_eq!(connector.buffered_event_count(), 1); // Event still in buffer
    }

    // --- Error type tests ---

    #[test]
    fn test_error_display() {
        let wal_err = EventBusConnectorError::Wal(WalError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "test",
        )));
        assert!(format!("{}", wal_err).contains("WAL error"));

        let eventbus_err = EventBusConnectorError::EventBus("test error".to_owned());
        assert!(format!("{}", eventbus_err).contains("EventBus error"));

        let conn_err = EventBusConnectorError::Connection("test conn".to_owned());
        assert!(format!("{}", conn_err).contains("Connection failed"));

        let overflow_err = EventBusConnectorError::BufferOverflow;
        assert!(format!("{}", overflow_err).contains("Buffer overflow"));

        let env_err = EventBusConnectorError::EnvNotSet("TEST_VAR".to_owned());
        assert!(format!("{}", env_err).contains("Environment variable not set"));

        let ser_err = EventBusConnectorError::Serialization("test ser".to_owned());
        assert!(format!("{}", ser_err).contains("Serialization error"));
    }

    #[test]
    fn test_error_debug() {
        let err = EventBusConnectorError::BufferOverflow;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("BufferOverflow"));
    }

    // --- Event conversion tests ---

    #[test]
    fn test_convert_to_eventbus_event_with_all_fields() {
        let event = ProcessEvent {
            pid: 1234,
            ppid: Some(1),
            name: "full_test".to_owned(),
            executable_path: Some("/usr/bin/full".to_owned()),
            command_line: vec!["full".to_owned(), "--opt1".to_owned(), "--opt2".to_owned()],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(50.5),
            memory_usage: Some(1024 * 1024 * 100),
            executable_hash: Some("sha256:abc".to_owned()),
            user_id: Some("root".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event);

        assert_eq!(eventbus_event.pid, 1234);
        assert_eq!(eventbus_event.name, "full_test");
        assert_eq!(eventbus_event.ppid, Some(1));
        assert_eq!(
            eventbus_event.executable_path,
            Some("/usr/bin/full".to_owned())
        );
        // Command line is joined with spaces
        assert_eq!(
            eventbus_event.command_line,
            Some("full --opt1 --opt2".to_owned())
        );
        assert!(eventbus_event.start_time.is_some());
    }

    #[test]
    fn test_convert_to_eventbus_event_minimal() {
        let event = ProcessEvent {
            pid: 1,
            ppid: None,
            name: "min".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: false,
            file_exists: false,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let eventbus_event = EventBusConnector::convert_to_eventbus_event(&event);

        assert_eq!(eventbus_event.pid, 1);
        assert_eq!(eventbus_event.name, "min");
        assert_eq!(eventbus_event.ppid, None);
        assert_eq!(eventbus_event.executable_path, None);
        assert_eq!(eventbus_event.command_line, Some("".to_owned()));
        assert_eq!(eventbus_event.start_time, None);
    }

    // --- Concurrent operations tests ---

    #[tokio::test]
    async fn test_multiple_publishes_preserve_order() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Publish multiple events
        for i in 1..=10 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Failed to publish");
        }

        // Verify order is preserved in buffer
        for (idx, buffered) in connector.buffer.iter().enumerate() {
            let expected_pid = (idx + 1) as u32;
            assert_eq!(buffered.event.pid, expected_pid);
            assert_eq!(buffered.sequence, (idx + 1) as u64);
        }
    }

    // --- Large event handling tests ---

    #[tokio::test]
    async fn test_large_command_line_event() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Create event with very long command line
        let long_args: Vec<String> = (0..1000).map(|i| format!("--arg{i}=value{i}")).collect();
        let event = ProcessEvent {
            pid: 1,
            ppid: Some(1),
            name: "large_cmd".to_owned(),
            executable_path: Some("/usr/bin/large".to_owned()),
            command_line: long_args,
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024),
            executable_hash: Some("hash".to_owned()),
            user_id: Some("1000".to_owned()),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let result = connector.publish(event, ProcessEventType::Start).await;
        assert!(result.is_ok());

        // Size should reflect the large command line
        assert!(connector.buffer_size_bytes > 10000);
    }

    #[tokio::test]
    async fn test_event_near_buffer_limit() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Create a normal event and measure its size
        let test_event = create_test_event(1);
        let event_size = BufferedEvent::estimate_size(&test_event, "events.process.start");

        // Set buffer to exactly 2 events + 1 byte margin
        connector.max_buffer_size = event_size * 2 + 1;

        // First two events should succeed
        let event1 = create_test_event(1);
        assert!(
            connector
                .publish(event1, ProcessEventType::Start)
                .await
                .is_ok()
        );

        let event2 = create_test_event(2);
        assert!(
            connector
                .publish(event2, ProcessEventType::Start)
                .await
                .is_ok()
        );

        // Third event should overflow
        let event3 = create_test_event(3);
        let result = connector.publish(event3, ProcessEventType::Start).await;
        assert!(matches!(
            result,
            Err(EventBusConnectorError::BufferOverflow)
        ));
    }

    // --- Connection state tests ---

    #[tokio::test]
    async fn test_is_connected_initial_state() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        assert!(!connector.is_connected());
    }

    #[tokio::test]
    async fn test_buffer_size_bytes_initial_state() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        assert_eq!(connector.buffer_size_bytes(), 0);
    }

    // --- Shutdown tests ---

    #[tokio::test]
    async fn test_shutdown_clears_connected_flag() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Manually set connected to true
        connector.connected = true;

        connector.shutdown().await.expect("Shutdown failed");

        assert!(!connector.is_connected());
    }

    #[tokio::test]
    async fn test_shutdown_with_buffered_events() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut connector = EventBusConnector::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        // Publish some events (they will be buffered since disconnected)
        for i in 1..=5 {
            let event = create_test_event(i);
            connector
                .publish(event, ProcessEventType::Start)
                .await
                .expect("Failed to publish");
        }

        assert_eq!(connector.buffered_event_count(), 5);

        // Shutdown should succeed with events in buffer
        connector.shutdown().await.expect("Shutdown failed");

        // Buffer is preserved for potential recovery
        assert_eq!(connector.buffered_event_count(), 5);
    }

    // --- Size estimation accuracy tests ---

    #[test]
    fn test_estimate_size_consistency() {
        let event = create_test_event(123);
        let topic = "events.process.start";

        // Multiple calls should return the same size
        let size1 = BufferedEvent::estimate_size(&event, topic);
        let size2 = BufferedEvent::estimate_size(&event, topic);
        let size3 = BufferedEvent::estimate_size(&event, topic);

        assert_eq!(size1, size2);
        assert_eq!(size2, size3);
    }

    #[test]
    fn test_estimate_size_increases_with_content() {
        let small_event = ProcessEvent {
            pid: 1,
            ppid: None,
            name: "a".to_owned(),
            executable_path: None,
            command_line: vec![],
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let large_event = ProcessEvent {
            pid: 1,
            ppid: Some(1),
            name: "a".repeat(100),
            executable_path: Some("b".repeat(200)),
            command_line: vec!["c".repeat(50); 10],
            start_time: Some(SystemTime::now()),
            cpu_usage: Some(1.0),
            memory_usage: Some(1024),
            executable_hash: Some("d".repeat(64)),
            user_id: Some("e".repeat(32)),
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let small_size = BufferedEvent::estimate_size(&small_event, "t");
        let large_size = BufferedEvent::estimate_size(&large_event, "t");

        assert!(large_size > small_size);
    }

    // --- WAL integration tests ---

    #[tokio::test]
    async fn test_wal_write_before_buffer() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        let mut connector = EventBusConnector::new(wal_path.clone())
            .await
            .expect("Failed to create connector");

        // Publish an event
        let event = create_test_event(42);
        let seq = connector
            .publish(event, ProcessEventType::Start)
            .await
            .expect("Failed to publish");

        assert_eq!(seq, 1);

        // Verify event is in WAL (even though also in buffer)
        let wal_events = connector.wal.replay().await.expect("Failed to replay WAL");
        assert_eq!(wal_events.len(), 1);
        assert_eq!(wal_events[0].pid, 42);

        // Also verify it's in buffer
        assert_eq!(connector.buffered_event_count(), 1);
        assert_eq!(connector.buffer[0].event.pid, 42);
    }

    // --- Client ID generation test ---

    #[tokio::test]
    async fn test_client_id_is_unique() {
        let temp_dir1 = TempDir::new().expect("Failed to create temp dir");
        let temp_dir2 = TempDir::new().expect("Failed to create temp dir");

        let connector1 = EventBusConnector::new(temp_dir1.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        let connector2 = EventBusConnector::new(temp_dir2.path().to_path_buf())
            .await
            .expect("Failed to create connector");

        assert_ne!(connector1.client_id, connector2.client_id);
        assert!(connector1.client_id.starts_with("procmond-"));
        assert!(connector2.client_id.starts_with("procmond-"));
    }
}
