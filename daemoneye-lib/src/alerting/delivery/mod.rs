//! Outbox-driven alert delivery: the [`AlertManager`] enqueues, one worker per sink delivers.
//!
//! Every store call runs under `spawn_blocking` (redb's single writer blocks), and each sink has
//! its own task, so a hung sink costs one attempt per timeout and nothing else.

mod bucket;
mod manager;
mod worker;

pub use manager::{AlertManager, EnqueueOutcome};

use super::AlertSink;
use super::health::{GlobalStats, SinkStats};
use crate::config::AlertingConfig;
use crate::models::Alert;
use crate::storage::{EventStore, StorageError};
use async_trait::async_trait;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Longest a worker sleeps before looking at its rows again.
pub(super) const WORKER_TICK: Duration = Duration::from_secs(5);
/// Longest shutdown waits for in-flight sends by default.
pub const SHUTDOWN_DRAIN: Duration = Duration::from_secs(10);
/// Dead letters moved back to `Pending` per automatic redrive.
pub const REDRIVE_BATCH: usize = 100;

/// Whole milliseconds in `duration`, saturating.
pub fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// `count` as a saturating `u64` counter.
pub(super) fn to_u64(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// A publisher could not hand an alert on.
#[derive(Debug, Error)]
#[error("alert publish failed: {0}")]
pub struct PublishError(String);

impl PublishError {
    /// An error carrying `reason`, which is logged and never parsed.
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// Announces a persisted alert to subscribers. A failure is counted and logged, never a delivery
/// failure: the alert is already durable.
#[async_trait]
pub trait AlertPublisher: Send + Sync {
    /// Publish `alert`.
    ///
    /// # Errors
    ///
    /// [`PublishError`] when the alert could not be handed on.
    async fn publish(&self, alert: &Alert) -> Result<(), PublishError>;
}

/// Publisher that drops every alert.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPublisher;

#[async_trait]
impl AlertPublisher for NoopPublisher {
    async fn publish(&self, _alert: &Alert) -> Result<(), PublishError> {
        Ok(())
    }
}

/// A store call that did not return a value.
#[derive(Debug, Error)]
pub(super) enum StoreCallError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("store task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl StoreCallError {
    pub(super) const fn is_stale(&self) -> bool {
        matches!(*self, Self::Storage(StorageError::DeliveryStale { .. }))
    }
}

/// Wall time that follows `tokio::time`, so paused-time tests move row timestamps too.
#[derive(Debug)]
pub(super) struct Clock {
    epoch_ms: u64,
    origin: Instant,
}

impl Clock {
    pub(super) fn start() -> Self {
        Self {
            epoch_ms: u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
            origin: Instant::now(),
        }
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.epoch_ms.saturating_add(millis(self.origin.elapsed()))
    }
}

/// Tunables every worker and the manager read, fixed at start.
#[derive(Debug, Clone)]
pub(super) struct Settings {
    pub(super) max_attempts: u32,
    pub(super) initial_backoff: Duration,
    pub(super) max_backoff: Duration,
    pub(super) attempt_timeout: Duration,
    pub(super) breaker_threshold: u32,
    pub(super) breaker_cooldown: Duration,
    pub(super) dlq_max_rows: usize,
    pub(super) dlq_max_redrives: u32,
    pub(super) max_open_rows: usize,
    pub(super) max_alerts_per_minute: Option<u32>,
    pub(super) dedup_window: Duration,
}

impl Settings {
    pub(super) const fn from_config(config: &AlertingConfig) -> Self {
        let delivery = &config.delivery;
        Self {
            max_attempts: delivery.max_attempts,
            initial_backoff: Duration::from_millis(delivery.initial_backoff_ms),
            max_backoff: Duration::from_secs(delivery.max_backoff_secs),
            attempt_timeout: Duration::from_secs(delivery.attempt_timeout_secs),
            breaker_threshold: delivery.breaker_failure_threshold,
            breaker_cooldown: Duration::from_secs(delivery.breaker_cooldown_secs),
            dlq_max_rows: delivery.dlq_max_rows_per_sink,
            dlq_max_redrives: delivery.dlq_max_redrives,
            max_open_rows: delivery.max_open_rows_per_sink,
            max_alerts_per_minute: config.max_alerts_per_minute,
            dedup_window: Duration::from_secs(config.dedup_window_seconds),
        }
    }
}

/// State shared by the manager and every worker.
pub(super) struct Shared {
    pub(super) store: Arc<EventStore>,
    pub(super) clock: Clock,
    pub(super) settings: Settings,
    pub(super) global: Mutex<GlobalStats>,
}

impl Shared {
    /// Run a store call off the runtime threads and track whether the store is working.
    pub(super) async fn blocking<T, F>(&self, call: F) -> Result<T, StoreCallError>
    where
        T: Send + 'static,
        F: FnOnce(&EventStore) -> Result<T, StorageError> + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let joined = tokio::task::spawn_blocking(move || call(&store)).await;
        let result = match joined {
            Ok(inner) => inner.map_err(StoreCallError::from),
            Err(join) => Err(StoreCallError::from(join)),
        };
        if let Err(ref error) = result
            && !error.is_stale()
        {
            lock(&self.global).store_failed();
        }
        result
    }
}

/// One sink as the manager and its worker both see it.
pub(super) struct SinkRuntime {
    pub(super) name: String,
    pub(super) is_local: bool,
    pub(super) fire_and_forget: bool,
    pub(super) sink: Box<dyn AlertSink>,
    pub(super) wake: Notify,
    pub(super) stats: Mutex<SinkStats>,
}

/// Lock a std mutex, ignoring poison: the counters it guards stay usable.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
