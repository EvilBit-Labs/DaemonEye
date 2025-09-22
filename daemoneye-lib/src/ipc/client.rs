//! Advanced IPC client implementation with connection management and resilience.
//!
//! This module provides a robust client implementation for daemoneye-agent that includes
//! automatic reconnection, connection pooling, circuit breaker patterns, load balancing,
//! failover capabilities, and comprehensive metrics collection for reliable communication
//! with multiple collector-core components.

use crate::ipc::IpcConfig;
use crate::ipc::codec::{IpcCodec, IpcError, IpcResult};
use crate::proto::{DetectionResult, DetectionTask};
use interprocess::local_socket::Name;
use interprocess::local_socket::tokio::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Connection state for the IPC client
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionState {
    /// Client is disconnected
    Disconnected,
    /// Client is attempting to connect
    Connecting,
    /// Client is connected and ready
    Connected,
    /// Client is in a failed state and will retry
    Failed,
    /// Client is in circuit breaker state (temporarily disabled)
    CircuitBreakerOpen,
}

/// Comprehensive metrics for IPC client performance monitoring
#[derive(Debug, Default)]
pub struct IpcClientMetrics {
    /// Total number of tasks sent
    pub tasks_sent_total: AtomicU64,
    /// Total number of successful task completions
    pub tasks_completed_total: AtomicU64,
    /// Total number of task failures
    pub tasks_failed_total: AtomicU64,
    /// Total number of connection attempts
    pub connection_attempts_total: AtomicU64,
    /// Total number of successful connections
    pub connections_established_total: AtomicU64,
    /// Total number of connection failures
    pub connection_failures_total: AtomicU64,
    /// Total number of circuit breaker activations
    pub circuit_breaker_activations_total: AtomicU64,
    /// Total bytes sent
    pub bytes_sent_total: AtomicU64,
    /// Total bytes received
    pub bytes_received_total: AtomicU64,
    /// Current number of active connections
    pub active_connections_current: AtomicU64,
    /// Current number of pooled connections
    pub pooled_connections_current: AtomicU64,
    /// Total task execution time in milliseconds
    pub task_duration_ms_total: AtomicU64,
    /// Total connection establishment time in milliseconds
    pub connection_duration_ms_total: AtomicU64,
}

impl IpcClientMetrics {
    /// Create new metrics instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a task attempt
    pub fn record_task_sent(&self) {
        self.tasks_sent_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful task completion
    pub fn record_task_completed(&self, duration_ms: u64) {
        self.tasks_completed_total.fetch_add(1, Ordering::Relaxed);
        self.task_duration_ms_total
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Record a task failure
    pub fn record_task_failed(&self) {
        self.tasks_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a connection attempt
    pub fn record_connection_attempt(&self) {
        self.connection_attempts_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful connection
    pub fn record_connection_established(&self, duration_ms: u64) {
        self.connections_established_total
            .fetch_add(1, Ordering::Relaxed);
        self.connection_duration_ms_total
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Record a connection failure
    pub fn record_connection_failed(&self) {
        self.connection_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record circuit breaker activation
    pub fn record_circuit_breaker_activation(&self) {
        self.circuit_breaker_activations_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record bytes sent
    pub fn record_bytes_sent(&self, bytes: u64) {
        self.bytes_sent_total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes received
    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received_total
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Update active connections count
    pub fn set_active_connections(&self, count: u64) {
        self.active_connections_current
            .store(count, Ordering::Relaxed);
    }

    /// Update pooled connections count
    pub fn set_pooled_connections(&self, count: u64) {
        self.pooled_connections_current
            .store(count, Ordering::Relaxed);
    }

    /// Get current success rate (0.0 to 1.0)
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn success_rate(&self) -> f64 {
        let total_tasks = self.tasks_sent_total.load(Ordering::Relaxed);
        if total_tasks == 0 {
            return 1.0; // No tasks sent yet, assume healthy
        }

        let completed_tasks = self.tasks_completed_total.load(Ordering::Relaxed);
        completed_tasks as f64 / total_tasks as f64
    }

    /// Get average task duration in milliseconds
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn average_task_duration_ms(&self) -> f64 {
        let total_duration = self.task_duration_ms_total.load(Ordering::Relaxed);
        let completed_tasks = self.tasks_completed_total.load(Ordering::Relaxed);

        if completed_tasks == 0 {
            return 0.0;
        }

        total_duration as f64 / completed_tasks as f64
    }

    /// Get average connection establishment time in milliseconds
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn average_connection_duration_ms(&self) -> f64 {
        let total_duration = self.connection_duration_ms_total.load(Ordering::Relaxed);
        let established_connections = self.connections_established_total.load(Ordering::Relaxed);

        if established_connections == 0 {
            return 0.0;
        }

        total_duration as f64 / established_connections as f64
    }

    /// Get connection success rate (0.0 to 1.0)
    #[allow(clippy::as_conversions)] // Safe: u64 to f64 conversion for metrics
    pub fn connection_success_rate(&self) -> f64 {
        let total_attempts = self.connection_attempts_total.load(Ordering::Relaxed);
        if total_attempts == 0 {
            return 1.0; // No attempts yet, assume healthy
        }

        let successful_connections = self.connections_established_total.load(Ordering::Relaxed);
        successful_connections as f64 / total_attempts as f64
    }
}

/// Circuit breaker state for failure handling with enhanced metrics
#[derive(Debug, Clone)]
struct CircuitBreaker {
    failure_count: u32,
    last_failure_time: Option<Instant>,
    failure_threshold: u32,
    recovery_timeout: Duration,
    is_open: bool,
    /// Half-open state for testing recovery
    is_half_open: bool,
    /// Number of successful calls in half-open state needed to close
    half_open_success_threshold: u32,
    /// Current success count in half-open state
    half_open_success_count: u32,
    /// Metrics reference for recording activations
    metrics: Option<Arc<IpcClientMetrics>>,
}

impl CircuitBreaker {
    const fn new(
        failure_threshold: u32,
        recovery_timeout: Duration,
        metrics: Option<Arc<IpcClientMetrics>>,
    ) -> Self {
        Self {
            failure_count: 0,
            last_failure_time: None,
            failure_threshold,
            recovery_timeout,
            is_open: false,
            is_half_open: false,
            half_open_success_threshold: 3, // Need 3 successes to close
            half_open_success_count: 0,
            metrics,
        }
    }

    fn record_success(&mut self) {
        if self.is_half_open {
            self.half_open_success_count = self.half_open_success_count.saturating_add(1);

            if self.half_open_success_count >= self.half_open_success_threshold {
                // Close the circuit breaker
                self.failure_count = 0;
                self.last_failure_time = None;
                self.is_open = false;
                self.is_half_open = false;
                self.half_open_success_count = 0;

                info!("Circuit breaker closed after successful recovery");
            }
        } else if self.is_open {
            // Shouldn't happen, but reset if it does
            self.failure_count = 0;
            self.last_failure_time = None;
            self.is_open = false;
        } else {
            // Normal operation - reset failure count
            self.failure_count = 0;
            self.last_failure_time = None;
        }
    }

    fn record_failure(&mut self) {
        if self.is_half_open {
            // Failure in half-open state - go back to open
            self.is_half_open = false;
            self.half_open_success_count = 0;
            self.last_failure_time = Some(Instant::now());

            warn!("Circuit breaker reopened due to failure during recovery");
        } else {
            self.failure_count = self.failure_count.saturating_add(1);
            self.last_failure_time = Some(Instant::now());

            if self.failure_count >= self.failure_threshold && !self.is_open {
                self.is_open = true;

                // Record metrics
                if let Some(ref metrics) = self.metrics {
                    metrics.record_circuit_breaker_activation();
                }

                warn!(
                    failure_count = self.failure_count,
                    threshold = self.failure_threshold,
                    "Circuit breaker opened due to failure threshold"
                );
            }
        }
    }

    fn should_attempt_connection(&mut self) -> bool {
        if !self.is_open {
            return true;
        }

        if let Some(last_failure) = self.last_failure_time {
            if last_failure.elapsed() >= self.recovery_timeout && !self.is_half_open {
                // Enter half-open state
                self.is_half_open = true;
                self.half_open_success_count = 0;

                debug!("Circuit breaker entering half-open state for recovery testing");
                return true;
            }
        }

        false
    }

    #[allow(dead_code)]
    const fn is_open(&self) -> bool {
        self.is_open
    }

    #[allow(dead_code)]
    const fn is_half_open(&self) -> bool {
        self.is_half_open
    }

    /// Get circuit breaker state for monitoring
    pub const fn state(&self) -> CircuitBreakerState {
        if self.is_open {
            if self.is_half_open {
                CircuitBreakerState::HalfOpen
            } else {
                CircuitBreakerState::Open
            }
        } else {
            CircuitBreakerState::Closed
        }
    }
}

/// Circuit breaker state for external monitoring
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitBreakerState {
    /// Circuit breaker is closed (normal operation)
    Closed,
    /// Circuit breaker is open (blocking requests)
    Open,
    /// Circuit breaker is half-open (testing recovery)
    HalfOpen,
}

/// Collector endpoint configuration for load balancing and failover
#[derive(Debug, Clone)]
pub struct CollectorEndpoint {
    /// Unique identifier for this collector
    pub id: String,
    /// Endpoint path (Unix socket path or Windows pipe name)
    pub endpoint_path: String,
    /// Priority for load balancing (lower = higher priority)
    pub priority: u32,
    /// Whether this endpoint is currently healthy
    pub is_healthy: bool,
    /// Last successful connection time
    pub last_success: Option<Instant>,
    /// Last failure time
    pub last_failure: Option<Instant>,
}

impl CollectorEndpoint {
    /// Create a new collector endpoint
    pub const fn new(id: String, endpoint_path: String, priority: u32) -> Self {
        Self {
            id,
            endpoint_path,
            priority,
            is_healthy: true,
            last_success: None,
            last_failure: None,
        }
    }

    /// Mark endpoint as healthy after successful operation
    pub fn mark_healthy(&mut self) {
        self.is_healthy = true;
        self.last_success = Some(Instant::now());
    }

    /// Mark endpoint as unhealthy after failure
    pub fn mark_unhealthy(&mut self) {
        self.is_healthy = false;
        self.last_failure = Some(Instant::now());
    }

    /// Calculate health score for load balancing (higher = better)
    pub fn health_score(&self) -> f64 {
        if !self.is_healthy {
            return 0.0;
        }

        let base_score = 1.0 / f64::from(self.priority.max(1));

        // Boost score based on recent success
        self.last_success.map_or(base_score, |last_success| {
            let seconds_since_success = last_success.elapsed().as_secs_f64();
            let recency_boost = (-seconds_since_success / 300.0).exp(); // 5-minute decay
            base_score * (1.0 + recency_boost)
        })
    }
}

/// Connection pool entry with metadata
#[derive(Debug)]
struct PooledConnection {
    stream: LocalSocketStream,
    endpoint_id: String,
    created_at: Instant,
    last_used: Instant,
    use_count: u64,
}

impl PooledConnection {
    fn new(stream: LocalSocketStream, endpoint_id: String) -> Self {
        let now = Instant::now();
        Self {
            stream,
            endpoint_id,
            created_at: now,
            last_used: now,
            use_count: 0,
        }
    }

    fn mark_used(&mut self) {
        self.last_used = Instant::now();
        self.use_count = self.use_count.saturating_add(1);
    }

    fn is_stale(&self, max_idle_time: Duration, max_lifetime: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time || self.created_at.elapsed() > max_lifetime
    }
}

/// Advanced connection pool for managing multiple collector connections
#[derive(Debug)]
struct ConnectionPool {
    /// Available connections by endpoint ID
    connections: HashMap<String, VecDeque<PooledConnection>>,
    /// Maximum connections per endpoint
    max_connections_per_endpoint: usize,
    /// Maximum total connections
    #[allow(dead_code)]
    max_total_connections: usize,
    /// Connection idle timeout
    max_idle_time: Duration,
    /// Connection maximum lifetime
    max_lifetime: Duration,
    /// Semaphore to limit total connections
    connection_semaphore: Arc<Semaphore>,
}

impl ConnectionPool {
    fn new(max_connections_per_endpoint: usize, max_total_connections: usize) -> Self {
        Self {
            connections: HashMap::new(),
            max_connections_per_endpoint,
            max_total_connections,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            max_lifetime: Duration::from_secs(3600), // 1 hour
            connection_semaphore: Arc::new(Semaphore::new(max_total_connections)),
        }
    }

    /// Get a connection from the pool or None if not available
    fn get_connection(&mut self, endpoint_id: &str) -> Option<PooledConnection> {
        let endpoint_connections = self.connections.get_mut(endpoint_id)?;

        // Remove stale connections
        endpoint_connections.retain(|conn| !conn.is_stale(self.max_idle_time, self.max_lifetime));

        // Get the most recently used connection
        endpoint_connections.pop_back()
    }

    /// Return a connection to the pool
    fn return_connection(&mut self, mut connection: PooledConnection) {
        // Check if connection is still valid
        if connection.is_stale(self.max_idle_time, self.max_lifetime) {
            debug!(
                endpoint_id = %connection.endpoint_id,
                age_secs = connection.created_at.elapsed().as_secs(),
                idle_secs = connection.last_used.elapsed().as_secs(),
                "Discarding stale connection"
            );
            return;
        }

        let endpoint_connections = self
            .connections
            .entry(connection.endpoint_id.clone())
            .or_default();

        // Limit connections per endpoint
        if endpoint_connections.len() >= self.max_connections_per_endpoint {
            debug!(
                endpoint_id = %connection.endpoint_id,
                pool_size = endpoint_connections.len(),
                "Connection pool full for endpoint, discarding connection"
            );
            return;
        }

        connection.mark_used();
        endpoint_connections.push_back(connection);
    }

    /// Get total number of pooled connections
    fn total_connections(&self) -> usize {
        self.connections
            .values()
            .map(std::collections::VecDeque::len)
            .sum()
    }

    /// Clean up stale connections across all endpoints
    fn cleanup_stale_connections(&mut self) {
        let mut total_removed: usize = 0;

        for (endpoint_id, connections) in &mut self.connections {
            let initial_count = connections.len();
            connections.retain(|conn| !conn.is_stale(self.max_idle_time, self.max_lifetime));
            let removed_count = initial_count.saturating_sub(connections.len());

            if removed_count > 0 {
                debug!(
                    endpoint_id = %endpoint_id,
                    removed = removed_count,
                    remaining = connections.len(),
                    "Cleaned up stale connections"
                );
                total_removed = total_removed.saturating_add(removed_count);
            }
        }

        // Remove empty endpoint entries
        self.connections
            .retain(|_, connections| !connections.is_empty());

        if total_removed > 0 {
            info!(
                total_removed = total_removed,
                total_remaining = self.total_connections(),
                "Connection pool cleanup completed"
            );
        }
    }

    /// Get connection semaphore for limiting total connections
    fn connection_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.connection_semaphore)
    }
}

/// Load balancing strategy for multiple collectors
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadBalancingStrategy {
    /// Round-robin selection
    RoundRobin,
    /// Weighted selection based on priority and health
    Weighted,
    /// Always prefer the highest priority healthy endpoint
    Priority,
}

/// Advanced IPC client with connection management, load balancing, and resilience
pub struct ResilientIpcClient {
    config: IpcConfig,
    codec: IpcCodec,
    /// Available collector endpoints
    endpoints: Arc<RwLock<Vec<CollectorEndpoint>>>,
    /// Per-endpoint circuit breakers
    circuit_breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
    /// Connection pool for all endpoints
    connection_pool: Arc<Mutex<ConnectionPool>>,
    /// Load balancing strategy
    load_balancing_strategy: LoadBalancingStrategy,
    /// Round-robin counter for load balancing
    round_robin_counter: Arc<AtomicU64>,
    /// Comprehensive metrics
    metrics: Arc<IpcClientMetrics>,
    /// Reconnection configuration
    max_reconnect_attempts: u32,
    base_reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

impl ResilientIpcClient {
    /// Create a new resilient IPC client with a single endpoint
    pub fn new(config: &IpcConfig) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes);
        let metrics = Arc::new(IpcClientMetrics::new());

        // Create default endpoint from config (initially unhealthy until tested)
        let mut default_endpoint = CollectorEndpoint::new(
            "default".to_owned(),
            config.endpoint_path.clone(),
            1, // Default priority
        );
        default_endpoint.is_healthy = false; // Start as unhealthy until proven otherwise

        Self {
            config: config.clone(),
            codec,
            endpoints: Arc::new(RwLock::new(vec![default_endpoint])),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            connection_pool: Arc::new(Mutex::new(ConnectionPool::new(
                4,                      // max connections per endpoint
                config.max_connections, // max total connections
            ))),
            load_balancing_strategy: LoadBalancingStrategy::Weighted,
            round_robin_counter: Arc::new(AtomicU64::new(0)),
            metrics,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }

    /// Create a new resilient IPC client with multiple collector endpoints
    pub fn new_with_endpoints(
        config: &IpcConfig,
        endpoints: Vec<CollectorEndpoint>,
        load_balancing_strategy: LoadBalancingStrategy,
    ) -> Self {
        let codec = IpcCodec::new(config.max_frame_bytes);
        let metrics = Arc::new(IpcClientMetrics::new());

        assert!(
            !endpoints.is_empty(),
            "At least one collector endpoint must be provided"
        );

        Self {
            config: config.clone(),
            codec,
            endpoints: Arc::new(RwLock::new(endpoints)),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            connection_pool: Arc::new(Mutex::new(ConnectionPool::new(
                4,                      // max connections per endpoint
                config.max_connections, // max total connections
            ))),
            load_balancing_strategy,
            round_robin_counter: Arc::new(AtomicU64::new(0)),
            metrics,
            max_reconnect_attempts: 10,
            base_reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }

    /// Add a new collector endpoint for load balancing and failover
    pub async fn add_endpoint(&self, endpoint: CollectorEndpoint) {
        {
            let mut endpoints = self.endpoints.write().await;

            // Check if endpoint already exists
            if endpoints.iter().any(|e| e.id == endpoint.id) {
                warn!(endpoint_id = %endpoint.id, "Endpoint already exists, skipping");
                return;
            }

            endpoints.push(endpoint.clone());
        };
        info!(
            endpoint_id = %endpoint.id,
            endpoint_path = %endpoint.endpoint_path,
            priority = endpoint.priority,
            "Added new collector endpoint"
        );
    }

    /// Remove a collector endpoint
    pub async fn remove_endpoint(&self, endpoint_id: &str) -> bool {
        let mut endpoints = self.endpoints.write().await;
        let initial_len = endpoints.len();

        endpoints.retain(|e| e.id != endpoint_id);

        if endpoints.len() < initial_len {
            info!(endpoint_id = %endpoint_id, "Removed collector endpoint");

            // Clean up circuit breaker
            let mut circuit_breakers = self.circuit_breakers.write().await;
            circuit_breakers.remove(endpoint_id);

            true
        } else {
            false
        }
    }

    /// Get metrics for monitoring
    pub fn metrics(&self) -> Arc<IpcClientMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Select the best endpoint based on load balancing strategy
    #[allow(clippy::significant_drop_tightening)] // More readable with current structure
    async fn select_endpoint(&self) -> Option<CollectorEndpoint> {
        let endpoints = self.endpoints.read().await;

        if endpoints.is_empty() {
            return None;
        }

        // Filter to healthy endpoints first
        let healthy_endpoints: Vec<&CollectorEndpoint> =
            endpoints.iter().filter(|e| e.is_healthy).collect();

        if healthy_endpoints.is_empty() {
            // No healthy endpoints, try the least recently failed one
            warn!("No healthy endpoints available, selecting least recently failed");
            return endpoints
                .iter()
                .min_by_key(|e| e.last_failure.unwrap_or_else(Instant::now))
                .cloned();
        }

        match self.load_balancing_strategy {
            LoadBalancingStrategy::RoundRobin => {
                let counter = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
                #[allow(clippy::arithmetic_side_effects)] // Safe: wrapping_rem prevents overflow
                let index = usize::try_from(counter)
                    .unwrap_or(0)
                    .wrapping_rem(healthy_endpoints.len());
                healthy_endpoints.get(index).map(|&e| e.clone())
            }
            LoadBalancingStrategy::Weighted => {
                // Select based on health score (higher is better)
                healthy_endpoints
                    .iter()
                    .max_by(|a, b| {
                        a.health_score()
                            .partial_cmp(&b.health_score())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|&e| e.clone())
            }
            LoadBalancingStrategy::Priority => {
                // Select highest priority (lowest number) healthy endpoint
                healthy_endpoints
                    .iter()
                    .min_by_key(|e| e.priority)
                    .map(|&e| e.clone())
            }
        }
    }

    /// Create socket name from endpoint path
    fn create_socket_name_for_endpoint(endpoint_path: &str) -> IpcResult<Name<'_>> {
        #[cfg(unix)]
        {
            use std::path::Path;
            let path = Path::new(endpoint_path);
            path.to_fs_name::<GenericFilePath>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
        #[cfg(windows)]
        {
            endpoint_path
                .to_ns_name::<GenericNamespaced>()
                .map_err(|e| IpcError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))
        }
    }

    /// Get or create circuit breaker for endpoint
    async fn get_circuit_breaker(&self, endpoint_id: &str) -> CircuitBreaker {
        let mut circuit_breakers = self.circuit_breakers.write().await;

        circuit_breakers
            .entry(endpoint_id.to_owned())
            .or_insert_with(|| {
                CircuitBreaker::new(
                    5,                       // failure threshold
                    Duration::from_secs(30), // recovery timeout
                    Some(Arc::clone(&self.metrics)),
                )
            })
            .clone()
    }

    /// Update circuit breaker state
    async fn update_circuit_breaker(&self, endpoint_id: &str, success: bool) {
        let mut circuit_breakers = self.circuit_breakers.write().await;

        if let Some(breaker) = circuit_breakers.get_mut(endpoint_id) {
            if success {
                breaker.record_success();
            } else {
                breaker.record_failure();
            }
        }
    }

    /// Mark endpoint as healthy or unhealthy
    async fn update_endpoint_health(&self, endpoint_id: &str, is_healthy: bool) {
        let mut endpoints = self.endpoints.write().await;

        if let Some(endpoint) = endpoints.iter_mut().find(|e| e.id == endpoint_id) {
            if is_healthy {
                endpoint.mark_healthy();
            } else {
                endpoint.mark_unhealthy();
            }
        }
    }

    /// Establish a new connection to the selected endpoint
    async fn establish_connection(&self) -> IpcResult<(LocalSocketStream, String)> {
        let start_time = Instant::now();
        self.metrics.record_connection_attempt();

        // Select best endpoint
        let endpoint = self.select_endpoint().await.ok_or_else(|| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No collector endpoints available",
            ))
        })?;

        // Check circuit breaker for this endpoint
        let mut circuit_breaker = self.get_circuit_breaker(&endpoint.id).await;
        if !circuit_breaker.should_attempt_connection() {
            return Err(IpcError::CircuitBreakerOpen);
        }

        // Acquire connection semaphore
        let semaphore = {
            let pool = self.connection_pool.lock().await;
            pool.connection_semaphore()
        };

        let _permit = semaphore.acquire().await.map_err(|_ignored| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "Connection pool exhausted",
            ))
        })?;

        let name = Self::create_socket_name_for_endpoint(&endpoint.endpoint_path)?;

        // Attempt connection with timeout
        let connect_timeout = Duration::from_millis(self.config.accept_timeout_ms);
        let stream = timeout(connect_timeout, LocalSocketStream::connect(name))
            .await
            .map_err(|_err| IpcError::ConnectionTimeout {
                endpoint: endpoint.endpoint_path.clone(),
            })?
            .map_err(|err| {
                // Convert OS errors to user-friendly IPC errors
                #[allow(clippy::wildcard_enum_match_arm)]
                match err.kind() {
                    std::io::ErrorKind::NotFound => IpcError::ServerNotFound {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::ConnectionRefused => IpcError::ConnectionRefused {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::PermissionDenied => IpcError::PermissionDenied {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    std::io::ErrorKind::TimedOut => IpcError::ConnectionTimeout {
                        endpoint: endpoint.endpoint_path.clone(),
                    },
                    _ => IpcError::Io(err),
                }
            })?;

        // Record successful connection
        let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(0);
        self.metrics.record_connection_established(duration_ms);
        self.update_circuit_breaker(&endpoint.id, true).await;
        self.update_endpoint_health(&endpoint.id, true).await;

        debug!(
            endpoint_id = %endpoint.id,
            endpoint_path = %endpoint.endpoint_path,
            duration_ms = duration_ms,
            "Successfully established IPC connection"
        );

        Ok((stream, endpoint.id))
    }

    /// Get a connection from the pool or create a new one
    async fn get_connection(&self) -> IpcResult<(LocalSocketStream, String)> {
        // Try to get from pool first
        let endpoint = self.select_endpoint().await.ok_or_else(|| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No collector endpoints available",
            ))
        })?;

        {
            let mut pool = self.connection_pool.lock().await;
            if let Some(pooled_conn) = pool.get_connection(&endpoint.id) {
                debug!(
                    endpoint_id = %endpoint.id,
                    use_count = pooled_conn.use_count,
                    "Reusing pooled connection"
                );
                return Ok((pooled_conn.stream, pooled_conn.endpoint_id));
            }
        }

        // No pooled connection available, create new one
        self.establish_connection().await
    }

    /// Release a connection back to the pool
    async fn release_connection(&self, stream: LocalSocketStream, endpoint_id: String) {
        let pooled_conn = PooledConnection::new(stream, endpoint_id);

        {
            let mut pool = self.connection_pool.lock().await;
            pool.return_connection(pooled_conn);

            // Update metrics
            self.metrics
                .set_pooled_connections(u64::try_from(pool.total_connections()).unwrap_or(0));
        }
    }

    /// Cleanup stale connections periodically
    pub async fn cleanup_connections(&self) {
        let mut pool = self.connection_pool.lock().await;
        pool.cleanup_stale_connections();

        // Update metrics
        self.metrics
            .set_pooled_connections(u64::try_from(pool.total_connections()).unwrap_or(0));
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff_delay(&self, attempt: u32) -> Duration {
        let delay_ms = u64::try_from(self.base_reconnect_delay.as_millis())
            .unwrap_or(100)
            .saturating_mul(2_u64.pow(attempt.min(10))); // Cap at 2^10 to prevent overflow

        let delay = Duration::from_millis(delay_ms);
        std::cmp::min(delay, self.max_reconnect_delay)
    }

    /// Send a detection task with automatic reconnection and retry logic
    pub async fn send_task(&mut self, task: DetectionTask) -> IpcResult<DetectionResult> {
        let task_id = task.task_id.clone();
        let start_time = Instant::now();
        let mut last_error = None;
        let mut reconnect_attempts: u32 = 0;

        self.metrics.record_task_sent();

        for attempt in 0..=self.max_reconnect_attempts {
            match self.attempt_send_task(&task).await {
                Ok(result) => {
                    let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(0);
                    self.metrics.record_task_completed(duration_ms);
                    return Ok(result);
                }
                Err(e) => {
                    last_error = Some(e);
                    reconnect_attempts = reconnect_attempts.saturating_add(1);

                    if attempt < self.max_reconnect_attempts {
                        let backoff_delay = self.calculate_backoff_delay(reconnect_attempts);

                        warn!(
                            task_id = %task_id,
                            attempt = attempt.saturating_add(1),
                            max_attempts = self.max_reconnect_attempts,
                            backoff_ms = backoff_delay.as_millis(),
                            "Task send failed, retrying with backoff"
                        );

                        sleep(backoff_delay).await;
                    }
                }
            }
        }

        // All retry attempts failed
        self.metrics.record_task_failed();
        error!(
            task_id = %task_id,
            attempts = self.max_reconnect_attempts.saturating_add(1),
            "Task send failed after all retry attempts"
        );

        Err(last_error
            .unwrap_or_else(|| IpcError::Io(std::io::Error::other("All retry attempts failed"))))
    }

    /// Attempt to send a task with a single connection attempt
    async fn attempt_send_task(&mut self, task: &DetectionTask) -> IpcResult<DetectionResult> {
        let (mut stream, endpoint_id) = self.get_connection().await.inspect_err(|_e| {
            self.metrics.record_connection_failed();
        })?;

        let read_timeout = Duration::from_millis(self.config.read_timeout_ms);
        let write_timeout = Duration::from_millis(self.config.write_timeout_ms);

        // Estimate message size for metrics
        let estimated_size = task.task_id.len().saturating_add(100); // Rough estimate

        // Send task with timeout
        let write_result = timeout(
            write_timeout,
            self.codec.write_message(&mut stream, task, write_timeout),
        )
        .await
        .map_err(|_err| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Write timeout",
            ))
        })?;

        match write_result {
            Ok(()) => {
                self.metrics
                    .record_bytes_sent(u64::try_from(estimated_size).unwrap_or(0));
            }
            Err(e) => {
                error!(endpoint_id = %endpoint_id, "Failed to write task: {}", e);
                self.update_circuit_breaker(&endpoint_id, false).await;
                self.update_endpoint_health(&endpoint_id, false).await;
                return Err(e);
            }
        }

        // Receive result with timeout
        let result: IpcResult<DetectionResult> = timeout(
            read_timeout,
            self.codec.read_message(&mut stream, read_timeout),
        )
        .await
        .map_err(|_err| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Read timeout",
            ))
        })?;

        match result {
            Ok(detection_result) => {
                // Estimate response size for metrics
                let response_size = detection_result.processes.len().saturating_mul(200); // Rough estimate
                self.metrics
                    .record_bytes_received(u64::try_from(response_size).unwrap_or(0));

                // Mark endpoint as healthy
                self.update_circuit_breaker(&endpoint_id, true).await;
                self.update_endpoint_health(&endpoint_id, true).await;

                // Release connection back to pool
                self.release_connection(stream, endpoint_id).await;

                Ok(detection_result)
            }
            Err(e) => {
                error!(endpoint_id = %endpoint_id, "Failed to read result: {}", e);
                self.update_circuit_breaker(&endpoint_id, false).await;
                self.update_endpoint_health(&endpoint_id, false).await;
                Err(e)
            }
        }
    }

    /// Check if the client is healthy (has at least one healthy endpoint)
    pub async fn health_check(&self) -> bool {
        let endpoints = self.endpoints.read().await;
        endpoints.iter().any(|e| e.is_healthy)
    }

    /// Get comprehensive connection statistics
    #[allow(clippy::significant_drop_tightening)] // More readable with current structure
    pub async fn get_stats(&self) -> AdvancedClientStats {
        let endpoints = self.endpoints.read().await;
        let circuit_breakers = self.circuit_breakers.read().await;
        let pool = self.connection_pool.lock().await;

        let endpoint_stats: Vec<EndpointStats> = endpoints
            .iter()
            .map(|endpoint| {
                let circuit_breaker_state = circuit_breakers
                    .get(&endpoint.id)
                    .map_or(CircuitBreakerState::Closed, CircuitBreaker::state);

                EndpointStats {
                    endpoint_id: endpoint.id.clone(),
                    endpoint_path: endpoint.endpoint_path.clone(),
                    priority: endpoint.priority,
                    is_healthy: endpoint.is_healthy,
                    health_score: endpoint.health_score(),
                    last_success: endpoint.last_success,
                    last_failure: endpoint.last_failure,
                    circuit_breaker_state,
                }
            })
            .collect();

        AdvancedClientStats {
            endpoint_stats,
            total_pooled_connections: pool.total_connections(),
            load_balancing_strategy: self.load_balancing_strategy.clone(),
            metrics: Arc::clone(&self.metrics),
        }
    }

    /// Get list of all endpoints
    pub async fn get_endpoints(&self) -> Vec<CollectorEndpoint> {
        let endpoints = self.endpoints.read().await;
        endpoints.clone()
    }

    /// Update endpoint priority for load balancing
    pub async fn update_endpoint_priority(&self, endpoint_id: &str, new_priority: u32) -> bool {
        let mut endpoints = self.endpoints.write().await;

        if let Some(endpoint) = endpoints.iter_mut().find(|e| e.id == endpoint_id) {
            endpoint.priority = new_priority;
            info!(
                endpoint_id = %endpoint_id,
                new_priority = new_priority,
                "Updated endpoint priority"
            );
            true
        } else {
            false
        }
    }

    /// Force health check on all endpoints
    #[allow(clippy::significant_drop_tightening)] // More readable with current structure
    pub async fn force_health_check(&self) -> Vec<(String, bool)> {
        let endpoints = self.endpoints.read().await;
        let mut results = Vec::new();

        for endpoint in endpoints.iter() {
            // Simple connection test
            let is_healthy = match Self::create_socket_name_for_endpoint(&endpoint.endpoint_path) {
                Ok(name) => {
                    let connect_timeout = Duration::from_millis(1000); // Short timeout for health check
                    timeout(connect_timeout, LocalSocketStream::connect(name))
                        .await
                        .is_ok()
                }
                Err(_) => false,
            };

            results.push((endpoint.id.clone(), is_healthy));
        }

        // Update endpoint health based on results
        {
            let mut endpoints_mut = self.endpoints.write().await;
            for &(ref endpoint_id, is_healthy) in &results {
                if let Some(endpoint) = endpoints_mut.iter_mut().find(|e| &e.id == endpoint_id) {
                    if is_healthy {
                        endpoint.mark_healthy();
                    } else {
                        endpoint.mark_unhealthy();
                    }
                }
            }
        }

        results
    }
}

/// Statistics about individual endpoints
#[derive(Debug, Clone)]
pub struct EndpointStats {
    pub endpoint_id: String,
    pub endpoint_path: String,
    pub priority: u32,
    pub is_healthy: bool,
    pub health_score: f64,
    pub last_success: Option<Instant>,
    pub last_failure: Option<Instant>,
    pub circuit_breaker_state: CircuitBreakerState,
}

/// Comprehensive statistics about the advanced client's state
#[derive(Debug, Clone)]
pub struct AdvancedClientStats {
    pub endpoint_stats: Vec<EndpointStats>,
    pub total_pooled_connections: usize,
    pub load_balancing_strategy: LoadBalancingStrategy,
    pub metrics: Arc<IpcClientMetrics>,
}

/// Legacy statistics for backward compatibility
#[derive(Debug, Clone)]
pub struct ClientStats {
    pub connection_state: ConnectionState,
    pub failure_count: u32,
    pub is_circuit_breaker_open: bool,
    pub reconnect_attempts: u32,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> (IpcConfig, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory for test");
        let endpoint_path = create_test_endpoint(&temp_dir);

        let config = IpcConfig {
            transport: crate::ipc::TransportType::Interprocess,
            endpoint_path,
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 4,
            panic_strategy: crate::ipc::PanicStrategy::Unwind,
        };

        (config, temp_dir)
    }

    fn create_test_endpoint(temp_dir: &TempDir) -> String {
        #[cfg(unix)]
        {
            temp_dir
                .path()
                .join("test-resilient-client.sock")
                .to_string_lossy()
                .to_string()
        }
        #[cfg(windows)]
        {
            let dir_name = temp_dir
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("test");
            format!(r"\\.\pipe\daemoneye\test-resilient-{dir_name}")
        }
    }

    #[test]
    fn test_client_creation() {
        let (config, _temp_dir) = create_test_config();
        let _client = ResilientIpcClient::new(&config);
        // Client creation always succeeds
    }

    #[test]
    fn test_client_creation_with_multiple_endpoints() {
        let (config, _temp_dir) = create_test_config();

        let endpoints = vec![
            CollectorEndpoint::new(
                "collector1".to_owned(),
                "/tmp/collector1.sock".to_owned(),
                1,
            ),
            CollectorEndpoint::new(
                "collector2".to_owned(),
                "/tmp/collector2.sock".to_owned(),
                2,
            ),
        ];

        let _client = ResilientIpcClient::new_with_endpoints(
            &config,
            endpoints,
            LoadBalancingStrategy::Weighted,
        );
        // Client creation always succeeds
    }

    #[test]
    fn test_circuit_breaker_creation() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(10), None);
        assert_eq!(breaker.failure_count, 0);
        assert!(!breaker.is_open());
        assert_eq!(breaker.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_circuit_breaker_failure_threshold() {
        let mut breaker = CircuitBreaker::new(2, Duration::from_secs(1), None);

        // Record failures up to threshold
        breaker.record_failure();
        assert!(!breaker.is_open());
        assert_eq!(breaker.state(), CircuitBreakerState::Closed);

        breaker.record_failure();
        assert!(breaker.is_open());
        assert_eq!(breaker.state(), CircuitBreakerState::Open);
        assert!(!breaker.should_attempt_connection());
    }

    #[test]
    fn test_circuit_breaker_half_open_recovery() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(100), None);

        // Trigger circuit breaker
        breaker.record_failure();
        assert!(breaker.is_open());
        assert_eq!(breaker.state(), CircuitBreakerState::Open);

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(150));
        assert!(breaker.should_attempt_connection());
        assert_eq!(breaker.state(), CircuitBreakerState::HalfOpen);

        // Test successful recovery
        breaker.record_success();
        breaker.record_success();
        breaker.record_success(); // Need 3 successes to close
        assert!(!breaker.is_open());
        assert_eq!(breaker.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_backoff_calculation() {
        let config = IpcConfig::default();
        let client = ResilientIpcClient::new(&config);

        // Test exponential backoff
        assert_eq!(
            client.calculate_backoff_delay(0),
            Duration::from_millis(100)
        );
        assert_eq!(
            client.calculate_backoff_delay(1),
            Duration::from_millis(200)
        );
        assert_eq!(
            client.calculate_backoff_delay(2),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn test_endpoint_health_scoring() {
        let mut endpoint =
            CollectorEndpoint::new("test".to_owned(), "/tmp/test.sock".to_owned(), 2);

        // Initial score based on priority
        let initial_score = endpoint.health_score();
        assert!(initial_score > 0.0);

        // Mark as successful
        endpoint.mark_healthy();
        let success_score = endpoint.health_score();
        assert!(success_score >= initial_score);

        // Mark as unhealthy
        endpoint.mark_unhealthy();
        assert!((endpoint.health_score() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_connection_pool() {
        let mut pool = ConnectionPool::new(2, 10);
        assert_eq!(pool.total_connections(), 0);

        // Pool starts empty
        assert!(pool.get_connection("test").is_none());
    }

    #[test]
    fn test_metrics_collection() {
        let metrics = IpcClientMetrics::new();

        // Test task metrics
        metrics.record_task_sent();
        metrics.record_task_completed(100);
        assert_eq!(metrics.tasks_sent_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.tasks_completed_total.load(Ordering::Relaxed), 1);
        assert!((metrics.average_task_duration_ms() - 100.0).abs() < f64::EPSILON);

        // Test connection metrics
        metrics.record_connection_attempt();
        metrics.record_connection_established(50);
        assert_eq!(metrics.connection_attempts_total.load(Ordering::Relaxed), 1);
        assert_eq!(
            metrics
                .connections_established_total
                .load(Ordering::Relaxed),
            1
        );
        assert!((metrics.average_connection_duration_ms() - 50.0).abs() < f64::EPSILON);

        // Test success rate
        assert!((metrics.success_rate() - 1.0).abs() < f64::EPSILON);
        assert!((metrics.connection_success_rate() - 1.0).abs() < f64::EPSILON);
    }
}
