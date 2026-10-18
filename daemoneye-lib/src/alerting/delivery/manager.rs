//! The handle the agent holds: enqueue, dedup window, health and shutdown.

use super::worker::{SinkWorker, wait_cancelled};
use super::{
    AlertPublisher, Clock, REDRIVE_BATCH, Settings, Shared, SinkRuntime, lock, millis, to_u64,
};
use crate::alerting::health::{
    DeliveryHealth, GlobalStats, SinkHealthSnapshot, SinkStats, aggregate,
};
use crate::alerting::{AlertSink, AlertingError};
use crate::config::AlertingConfig;
use crate::models::{Alert, DeliveryRecord, DeliveryState, Uuid};
use crate::storage::{EnqueueReport, EventStore};
use futures::FutureExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{error, warn};

/// Dedup entries kept before expired ones are swept on insert.
const WINDOW_SWEEP_AT: usize = 1024;

/// Alerts waiting for the background publisher; a full queue drops the announcement, not the alert.
const PUBLISH_QUEUE_CAPACITY: usize = 256;

/// What happened to one alert handed to [`AlertManager::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnqueueOutcome {
    /// The alert and its delivery rows are durable.
    Enqueued,
    /// The alert was already enqueued; nothing was written.
    AlreadyEnqueued,
    /// A duplicate of `by` inside the dedup window; rows were written as suppressed.
    SuppressedDuplicate {
        /// The alert that holds the window.
        by: Uuid,
    },
    /// The store failed; the alert is not durable.
    NotPersisted,
}

/// Deduplication keys of recently enqueued alerts: key to (enqueue time, alert id).
#[derive(Debug, Default)]
struct DedupWindow {
    entries: HashMap<String, (u64, Uuid)>,
}

impl DedupWindow {
    fn holder(&self, key: &str, now_ms: u64, window_ms: u64) -> Option<Uuid> {
        self.entries
            .get(key)
            .filter(|&&(at_ms, _)| now_ms.saturating_sub(at_ms) < window_ms)
            .map(|&(_, id)| id)
    }

    fn record(&mut self, key: &str, at_ms: u64, id: Uuid, now_ms: u64, window_ms: u64) {
        if self.entries.len() >= WINDOW_SWEEP_AT {
            self.entries
                .retain(|_, &mut (at, _)| now_ms.saturating_sub(at) < window_ms);
        }
        self.entries.insert(key.to_owned(), (at_ms, id));
    }
}

/// Owns one delivery worker per sink and the dedup window.
pub struct AlertManager {
    shared: Arc<Shared>,
    sinks: Vec<Arc<SinkRuntime>>,
    publish_tx: mpsc::Sender<Alert>,
    window: Mutex<DedupWindow>,
    enqueue_gate: tokio::sync::Mutex<()>,
    stop: Arc<watch::Sender<bool>>,
    tasks: Mutex<JoinSet<()>>,
}

impl AlertManager {
    /// Seed the dedup window from `store` and start one worker per sink.
    ///
    /// `cancel` flips to `true` to stop the workers; [`shutdown`](Self::shutdown) does the same
    /// and waits.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::alerting::{
    ///     AlertManager, EnqueueOutcome, NoopPublisher, OutputFormat, StdoutSink,
    /// };
    /// use daemoneye_lib::config::AlertingConfig;
    /// use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessRecord};
    /// use daemoneye_lib::storage::EventStore;
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// # let rt = tokio::runtime::Runtime::new().unwrap();
    /// # rt.block_on(async {
    /// let dir = tempfile::tempdir().unwrap();
    /// let store = Arc::new(EventStore::new(dir.path().join("events.redb")).unwrap());
    /// let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
    /// let manager = AlertManager::start(
    ///     store,
    ///     vec![Box::new(StdoutSink::new("stdout".to_owned(), OutputFormat::Json))],
    ///     &AlertingConfig::default(),
    ///     Arc::new(NoopPublisher),
    ///     cancel_rx,
    /// )
    /// .await
    /// .unwrap();
    ///
    /// let alert = Alert::new(
    ///     AlertSeverity::High, "Title", "Desc", "rule-1",
    ///     ProcessRecord::new(1, "proc".to_owned()), Completeness::complete(),
    /// );
    /// assert_eq!(manager.enqueue(&alert).await, EnqueueOutcome::Enqueued);
    /// // The same deduplication key inside the window is stored but not sent again.
    /// let again = Alert::new(
    ///     AlertSeverity::High, "Title", "Desc", "rule-1",
    ///     ProcessRecord::new(1, "proc".to_owned()), Completeness::complete(),
    /// );
    /// assert!(matches!(
    ///     manager.enqueue(&again).await,
    ///     EnqueueOutcome::SuppressedDuplicate { .. }
    /// ));
    /// manager.shutdown(Duration::from_secs(1)).await;
    /// # });
    /// ```
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] for a delivery setting out of range, or a duplicate
    /// or unusable sink name; [`AlertingError::DeduplicationError`] when the window cannot be
    /// seeded.
    pub async fn start(
        store: Arc<EventStore>,
        sinks: Vec<Box<dyn AlertSink>>,
        alerting: &AlertingConfig,
        publisher: Arc<dyn AlertPublisher>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Self, AlertingError> {
        Self::start_seeded(store, sinks, alerting, publisher, cancel, None).await
    }

    /// [`start`](Self::start) with a fixed retry-jitter seed, for reproducible tests.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    #[doc(hidden)]
    pub async fn start_with_rng_seed(
        store: Arc<EventStore>,
        sinks: Vec<Box<dyn AlertSink>>,
        alerting: &AlertingConfig,
        publisher: Arc<dyn AlertPublisher>,
        cancel: watch::Receiver<bool>,
        seed: u64,
    ) -> Result<Self, AlertingError> {
        Self::start_seeded(store, sinks, alerting, publisher, cancel, Some(seed)).await
    }

    async fn start_seeded(
        store: Arc<EventStore>,
        sinks: Vec<Box<dyn AlertSink>>,
        alerting: &AlertingConfig,
        publisher: Arc<dyn AlertPublisher>,
        cancel: watch::Receiver<bool>,
        seed: Option<u64>,
    ) -> Result<Self, AlertingError> {
        // The loader validates a config file; a config built in code gets the same check here.
        alerting
            .delivery
            .validate()
            .map_err(|error| AlertingError::ConfigurationError(error.to_string()))?;
        let runtimes = build_runtimes(sinks)?;
        let shared = Arc::new(Shared {
            store,
            clock: Clock::start(),
            settings: Settings::from_config(alerting),
            global: Mutex::new(GlobalStats::default()),
        });
        let window = seed_window(&shared).await?;
        let (stop_tx, stop_rx) = watch::channel(false);
        let stop = Arc::new(stop_tx);
        let mut tasks = JoinSet::new();
        tasks.spawn(forward_cancel(cancel, stop_rx.clone(), Arc::clone(&stop)));
        for (index, runtime) in runtimes.iter().enumerate() {
            let rng = seed.map_or_else(
                || StdRng::from_rng(&mut rand::rng()),
                |base| StdRng::seed_from_u64(base.wrapping_add(u64::try_from(index).unwrap_or(0))),
            );
            tasks.spawn(supervise(
                Arc::clone(&shared),
                Arc::clone(runtime),
                rng,
                stop_rx.clone(),
            ));
        }
        let (publish_tx, publish_rx) = mpsc::channel(PUBLISH_QUEUE_CAPACITY);
        tasks.spawn(run_publisher(
            Arc::clone(&shared),
            publisher,
            publish_rx,
            stop_rx,
        ));
        Ok(Self {
            shared,
            sinks: runtimes,
            publish_tx,
            window: Mutex::new(window),
            enqueue_gate: tokio::sync::Mutex::new(()),
            stop,
            tasks: Mutex::new(tasks),
        })
    }

    /// Persist `alert` with one delivery row per sink, then announce it on the publish queue,
    /// unless it is a suppressed or already-enqueued duplicate.
    ///
    /// An alert inside the dedup window of the same `deduplication_key` is stored with its rows
    /// marked as suppressed duplicates, and does not extend the window.
    ///
    /// Concurrent calls are serialized from the dedup check through recording the window, so two
    /// alerts with one key cannot both see no holder. The window is still recorded only after the
    /// commit (ADR-0015).
    pub async fn enqueue(&self, alert: &Alert) -> EnqueueOutcome {
        let gate = self.enqueue_gate.lock().await;
        let settings = &self.shared.settings;
        let window_ms = millis(settings.dedup_window);
        let now = self.shared.clock.now_ms();
        let suppressor = lock(&self.window).holder(&alert.deduplication_key, now, window_ms);
        let names: Vec<String> = self.sinks.iter().map(|sink| sink.name.clone()).collect();
        let max_open = settings.max_open_rows;
        let keep = settings.dlq_max_rows;
        let owned = alert.clone();
        let stored = self
            .shared
            .blocking(move |store| {
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                store.enqueue_alert(&owned, &refs, suppressor, max_open, now)
            })
            .await;
        let report = match stored {
            Ok(done) => {
                // Only a persisted alert proves the store is taking writes again; a worker's
                // successful read says nothing about a broken alerts table.
                lock(&self.shared.global).store_ok();
                done
            }
            Err(error) => {
                error!(alert_id = %alert.id, %error, "alert was not persisted");
                return EnqueueOutcome::NotPersisted;
            }
        };
        let rows = match report {
            EnqueueReport::Enqueued { rows } => rows,
            EnqueueReport::AlreadyEnqueued => return EnqueueOutcome::AlreadyEnqueued,
        };
        // The alert is durable now: a trim failure is a store-health problem, not a lost alert.
        let evicted = self.evict_overflow(alert.id, &rows, keep).await;
        self.count_queue_full(alert.id, &rows, &evicted);
        if let Some(by) = suppressor {
            return EnqueueOutcome::SuppressedDuplicate { by };
        }
        lock(&self.window).record(&alert.deduplication_key, now, alert.id, now, window_ms);
        drop(gate);
        self.wake_all();
        if let Err(error) = self.publish_tx.try_send(alert.clone()) {
            warn!(alert_id = %alert.id, %error, "alert publish queue refused the alert");
            lock(&self.shared.global).publish_failed();
        }
        EnqueueOutcome::Enqueued
    }

    /// Trim the dead letters of every sink that just dropped or displaced a row; each sink's
    /// trim is its own store call, so one failure does not hide another sink's count.
    async fn evict_overflow(
        &self,
        alert_id: Uuid,
        rows: &[PlacedRow],
        keep: usize,
    ) -> Vec<(String, usize)> {
        let mut evicted = Vec::new();
        for &(ref name, state, displaced) in rows {
            if displaced.is_none() && !is_refused(state) {
                continue;
            }
            let sink = name.clone();
            match self
                .shared
                .blocking(move |store| store.evict_dead_letters(&sink, keep))
                .await
            {
                Ok(removed) => evicted.push((name.clone(), removed)),
                Err(error) => {
                    error!(sink = %name, %alert_id, %error, "dead-letter eviction failed after commit");
                }
            }
        }
        evicted
    }

    fn count_queue_full(&self, alert_id: Uuid, rows: &[PlacedRow], evicted: &[(String, usize)]) {
        for sink in &self.sinks {
            let dropped = rows
                .iter()
                .filter(|&&(ref name, state, displaced)| {
                    *name == sink.name && (displaced.is_some() || is_refused(state))
                })
                .count();
            let removed: usize = evicted
                .iter()
                .filter(|entry| entry.0 == sink.name)
                .map(|entry| entry.1)
                .sum();
            if dropped > 0 || removed > 0 {
                warn!(
                    sink = %sink.name,
                    %alert_id,
                    dropped,
                    evicted = removed,
                    "open-row cap dead-lettered a row; dead letters evicted to hold the cap"
                );
            }
            let mut stats = lock(&sink.stats);
            stats.queue_full = stats.queue_full.saturating_add(to_u64(dropped));
            stats.dead_letters = stats.dead_letters.saturating_add(to_u64(dropped));
            stats.evicted = stats.evicted.saturating_add(to_u64(removed));
            stats.left_dead_letters(to_u64(removed));
        }
    }

    /// Send `alert` once, directly, to the local sinks only.
    ///
    /// For an alert the store would not take: it is marked `not_persisted`, counted in
    /// [`DeliveryHealth::unpersisted_alerts`], and never touches the dedup window or the outbox.
    pub async fn deliver_unpersisted(&self, alert: &Alert) {
        lock(&self.shared.global).unpersisted();
        let mut marked = alert.clone();
        marked
            .context
            .data
            .insert("not_persisted".to_owned(), "true".to_owned());
        let timeout = self.shared.settings.attempt_timeout;
        let (alert_id, rule_id) = (alert.id, alert.detection_rule_id.as_str());
        let locals: Vec<&Arc<SinkRuntime>> =
            self.sinks.iter().filter(|sink| sink.is_local).collect();
        if locals.is_empty() {
            warn!(%alert_id, rule_id, "unpersisted alert lost: no local sink is configured");
            return;
        }
        let sends = locals.into_iter().map(|sink| {
            let shared_alert = &marked;
            async move {
                match tokio::time::timeout(timeout, sink.sink.send(shared_alert)).await {
                    Ok(Ok(_delivered)) => {}
                    Ok(Err(error)) => {
                        warn!(sink = %sink.name, %alert_id, rule_id, %error, "unpersisted alert delivery failed");
                    }
                    Err(_elapsed) => {
                        warn!(sink = %sink.name, %alert_id, rule_id, "unpersisted alert delivery timed out");
                    }
                }
            }
        });
        futures::future::join_all(sends).await;
    }

    /// Wake every worker, for rows enqueued from outside this handle.
    pub fn wake_all(&self) {
        for sink in &self.sinks {
            sink.wake.notify_one();
        }
    }

    /// Delivery health of every sink and the store.
    #[must_use]
    pub fn health(&self) -> DeliveryHealth {
        let now = self.shared.clock.now_ms();
        let sinks: Vec<SinkHealthSnapshot> =
            self.sinks.iter().map(|sink| snapshot(sink, now)).collect();
        let global = lock(&self.shared.global);
        DeliveryHealth {
            aggregate: aggregate(&sinks, global.store_failing),
            sinks,
            store_failures: global.store_failures,
            publish_failures: global.publish_failures,
            unpersisted_alerts: global.unpersisted_alerts,
        }
    }

    /// Dead-lettered rows of `sink`, oldest first. A store failure is logged and reads as empty.
    pub async fn dead_letters(&self, sink: &str) -> Vec<(Uuid, DeliveryRecord)> {
        let name = sink.to_owned();
        match self
            .shared
            .blocking(move |store| store.dead_letters(&name))
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                error!(%sink, %error, "reading dead letters failed");
                Vec::new()
            }
        }
    }

    /// Move `sink`'s dead letters back to `Pending`; returns how many moved. An unknown sink
    /// name is logged and moves nothing.
    pub async fn redrive(&self, sink: &str) -> usize {
        let Some(runtime) = self.sinks.iter().find(|candidate| candidate.name == sink) else {
            warn!(%sink, "redrive requested for a sink that is not configured");
            return 0;
        };
        let name = sink.to_owned();
        let now = self.shared.clock.now_ms();
        let max_redrives = self.shared.settings.dlq_max_redrives;
        let limit = self.shared.settings.dlq_max_rows.max(REDRIVE_BATCH);
        let max_open_rows = self.shared.settings.max_open_rows;
        let moved = match self
            .shared
            .blocking(move |store| {
                store.redrive_batch(&name, limit, max_open_rows, max_redrives, now)
            })
            .await
        {
            Ok(report) => report.moved,
            Err(error) => {
                error!(%sink, %error, "redrive failed");
                0
            }
        };
        lock(&runtime.stats).left_dead_letters(to_u64(moved));
        runtime.wake.notify_one();
        moved
    }

    /// Stop the workers: no new sends start, in-flight ones get `drain` to finish, the rest are
    /// aborted and their rows stay `Retrying` for the next start. Announcements still queued for
    /// the publisher are dropped and counted; the alerts are durable and the workers pick their
    /// rows up by tick.
    pub async fn shutdown(&self, drain: Duration) {
        self.stop.send_replace(true);
        let mut tasks = std::mem::take(&mut *lock(&self.tasks));
        let drained = tokio::time::timeout(drain, join_all_logged(&mut tasks)).await;
        if drained.is_err() {
            warn!("alert workers did not drain in time; aborting in-flight sends");
            tasks.abort_all();
            join_all_logged(&mut tasks).await;
        }
    }

    /// Whether each sink answers its own health check within the attempt timeout.
    pub async fn health_check(&self) -> HashMap<String, Result<(), AlertingError>> {
        let timeout = self.shared.settings.attempt_timeout;
        let mut results = HashMap::new();
        for sink in &self.sinks {
            let result = match tokio::time::timeout(timeout, sink.sink.health_check()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_elapsed) => Err("health check timed out".to_owned()),
            }
            .map_err(|error| AlertingError::HealthCheckFailed {
                sink: sink.name.clone(),
                error,
            });
            results.insert(sink.name.clone(), result);
        }
        results
    }
}

/// Join every task, logging the ones that ended by panic.
async fn join_all_logged(tasks: &mut JoinSet<()>) {
    while let Some(joined) = tasks.join_next().await {
        if let Err(error) = joined
            && error.is_panic()
        {
            error!(%error, "alert delivery task panicked");
        }
    }
}

/// Run the sink's worker, restarting it after a tick if it panics; a sink's rows must not pile
/// up silently because one send blew up.
async fn supervise(
    shared: Arc<Shared>,
    runtime: Arc<SinkRuntime>,
    mut rng: StdRng,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let worker = SinkWorker::new(
            Arc::clone(&shared),
            Arc::clone(&runtime),
            StdRng::from_rng(&mut rng),
            cancel.clone(),
        );
        if AssertUnwindSafe(worker.run()).catch_unwind().await.is_ok() {
            return;
        }
        error!(sink = %runtime.name, "alert worker panicked; restarting it after a tick");
        tokio::select! {
            () = tokio::time::sleep(super::WORKER_TICK) => {}
            () = wait_cancelled(&mut cancel) => return,
        }
    }
}

type PlacedRow = (String, DeliveryState, Option<Uuid>);

const fn is_refused(state: DeliveryState) -> bool {
    matches!(state, DeliveryState::DeadLettered)
}

fn snapshot(sink: &SinkRuntime, now_ms: u64) -> SinkHealthSnapshot {
    let stats = lock(&sink.stats);
    SinkHealthSnapshot {
        name: sink.name.clone(),
        is_local: sink.is_local,
        breaker: stats.breaker,
        consecutive_failures: stats.consecutive_failures,
        failing: stats.failing,
        attempts: stats.attempts,
        delivered: stats.delivered,
        success_rate: stats.success_rate(),
        dead_letters: stats.dead_letters,
        evicted: stats.evicted,
        queue_full: stats.queue_full,
        corrupt: stats.corrupt,
        oldest_pending_age_ms: stats
            .oldest_pending_enqueue_ms
            .map(|at| now_ms.saturating_sub(at)),
        last_error: stats.last_error.clone(),
        last_success_ms: stats.last_success_ms,
        misconfigured: stats.misconfigured,
        fire_and_forget: sink.fire_and_forget,
    }
}

fn build_runtimes(sinks: Vec<Box<dyn AlertSink>>) -> Result<Vec<Arc<SinkRuntime>>, AlertingError> {
    let mut runtimes: Vec<Arc<SinkRuntime>> = Vec::with_capacity(sinks.len());
    for sink in sinks {
        let name = sink.name().to_owned();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(AlertingError::ConfigurationError(format!(
                "sink name {name:?} is empty or has control characters"
            )));
        }
        if runtimes.iter().any(|existing| existing.name == name) {
            return Err(AlertingError::ConfigurationError(format!(
                "duplicate sink name {name:?}"
            )));
        }
        runtimes.push(Arc::new(SinkRuntime {
            is_local: sink.is_local(),
            fire_and_forget: sink.is_fire_and_forget(),
            name,
            sink,
            wake: tokio::sync::Notify::new(),
            stats: Mutex::new(SinkStats::new()),
        }));
    }
    Ok(runtimes)
}

async fn seed_window(shared: &Shared) -> Result<DedupWindow, AlertingError> {
    let now = shared.clock.now_ms();
    let window_ms = millis(shared.settings.dedup_window);
    let seeded = shared
        .blocking(move |store| store.seed_dedup_window(now, window_ms))
        .await
        .map_err(|error| AlertingError::DeduplicationError(error.to_string()))?;
    let entries = seeded
        .into_iter()
        .map(|(key, at_ms, id)| (key, (at_ms, id)))
        .collect();
    Ok(DedupWindow { entries })
}

/// Turn the caller's cancellation into the workers' own, then end.
async fn forward_cancel(
    mut external: watch::Receiver<bool>,
    mut internal: watch::Receiver<bool>,
    stop: Arc<watch::Sender<bool>>,
) {
    tokio::select! {
        flagged = external.wait_for(|&cancelled| cancelled) => {
            // A dropped sender is not a request to stop.
            if flagged.is_ok() {
                stop.send_replace(true);
            }
        }
        () = wait_cancelled(&mut internal) => {}
    }
}

/// Announce queued alerts until stopped; a failed publish is counted, never retried, and so is
/// every announcement still queued when the stop arrives.
async fn run_publisher(
    shared: Arc<Shared>,
    publisher: Arc<dyn AlertPublisher>,
    mut queue: mpsc::Receiver<Alert>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let alert = tokio::select! {
            next = queue.recv() => match next {
                Some(alert) => alert,
                None => return,
            },
            () = wait_cancelled(&mut stop) => break,
        };
        if let Err(error) = publisher.publish(&alert).await {
            warn!(alert_id = %alert.id, %error, "alert publish failed");
            lock(&shared.global).publish_failed();
        }
    }
    queue.close();
    let mut dropped = 0_u64;
    while queue.try_recv().is_ok() {
        dropped = dropped.saturating_add(1);
        lock(&shared.global).publish_failed();
    }
    if dropped > 0 {
        warn!(
            dropped,
            "alert announcements still queued at shutdown were dropped"
        );
    }
}
