//! One worker per sink: loads the alert, claims the row, sends, writes the outcome back with
//! compare-and-set.

use super::bucket::TokenBucket;
use super::{
    REDRIVE_BATCH, Shared, SinkRuntime, StoreCallError, WORKER_TICK, lock, millis, to_u64,
};
use crate::alerting::AlertingError;
use crate::alerting::reliability::{
    BreakerState, CircuitBreaker, DeliveryError, DeliveryErrorClass, Permit, RetryPolicy,
};
use crate::models::{Alert, DeliveryReason, DeliveryRecord, Uuid};
use crate::storage::{CompleteOutcome, DeliveryExpectation, RedriveReport};
use rand::rngs::StdRng;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::error::Elapsed;
use tracing::{debug, error, warn};

/// Floor on the sleep between passes, so a zero retry delay cannot spin.
const MIN_WAIT: Duration = Duration::from_millis(1);

/// Whether a pass should keep going after one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

pub(super) struct SinkWorker {
    shared: Arc<Shared>,
    runtime: Arc<SinkRuntime>,
    policy: RetryPolicy,
    breaker: CircuitBreaker,
    bucket: Option<TokenBucket>,
    rng: StdRng,
    cancel: watch::Receiver<bool>,
    /// Dead letters may exist that a redrive has not reached.
    redrive_pending: bool,
}

impl SinkWorker {
    pub(super) fn new(
        shared: Arc<Shared>,
        runtime: Arc<SinkRuntime>,
        rng: StdRng,
        cancel: watch::Receiver<bool>,
    ) -> Self {
        let settings = &shared.settings;
        Self {
            policy: RetryPolicy::new(
                settings.max_attempts,
                settings.initial_backoff,
                settings.max_backoff,
            ),
            breaker: CircuitBreaker::new(settings.breaker_threshold, settings.breaker_cooldown),
            bucket: TokenBucket::new(settings.max_alerts_per_minute),
            shared,
            runtime,
            rng,
            cancel,
            redrive_pending: false,
        }
    }

    /// Redrives every batch of dead letters the open-row cap admits, then loops: one pass over
    /// the sink's rows, then sleep until a wake, the tick or cancellation. A local sink has no
    /// breaker, so the startup backlog is its only automatic redrive; it keeps draining as open
    /// rows free up. A network sink also redrives when its breaker recovers.
    pub(super) async fn run(mut self) {
        let runtime = Arc::clone(&self.runtime);
        let mut report = self.redrive().await;
        while report.moved > 0 && report.remaining > 0 && !self.is_cancelled() {
            report = self.redrive().await;
        }
        self.redrive_pending = report.remaining > 0;
        while !self.is_cancelled() {
            let wait = self.pass().await;
            tokio::select! {
                () = runtime.wake.notified() => {}
                () = tokio::time::sleep(wait) => {}
                () = wait_cancelled(&mut self.cancel) => break,
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    /// One look at the sink's rows; returns how long to sleep before the next.
    async fn pass(&mut self) -> Duration {
        self.redrive_when_able().await;
        let Some(rows) = self.read_open().await else {
            return WORKER_TICK;
        };
        if rows.is_empty() && self.probe_health().await {
            return MIN_WAIT;
        }
        let now = self.shared.clock.now_ms();
        let max_backoff_ms = millis(self.shared.settings.max_backoff);
        let mut next_due: Option<u64> = None;
        let mut probed = false;
        for (id, row) in rows {
            if self.is_cancelled() {
                break;
            }
            if !is_due(&row, now, max_backoff_ms) {
                next_due =
                    Some(next_due.map_or(row.next_attempt_ms, |due| due.min(row.next_attempt_ms)));
                continue;
            }
            let probe = match self.permit() {
                Permit::Deny => break,
                Permit::AllowProbe => true,
                Permit::Allow => false,
            };
            if !self.pace().await {
                break;
            }
            let flow = self.attempt(id, &row, probe).await;
            probed = probe;
            if flow == Flow::Stop || probe {
                break;
            }
        }
        if probed {
            return MIN_WAIT;
        }
        let until_due = next_due
            .map(|due| Duration::from_millis(due.saturating_sub(self.shared.clock.now_ms())));
        until_due
            .map_or(WORKER_TICK, |wait| wait.min(WORKER_TICK))
            .max(MIN_WAIT)
    }

    fn permit(&mut self) -> Permit {
        if self.runtime.is_local {
            return Permit::Allow;
        }
        let permit = self.breaker.permit();
        self.sync_breaker();
        permit
    }

    /// Mirror the breaker into the sink's stats, logging every transition.
    fn sync_breaker(&self) {
        let state = self.breaker.state();
        let mut stats = lock(&self.runtime.stats);
        if stats.breaker != state {
            warn!(
                sink = %self.runtime.name,
                from = ?stats.breaker,
                to = ?state,
                "sink breaker changed state"
            );
            stats.breaker = state;
        }
    }

    /// With no open row to probe on, the breaker would stay open and strand the dead letters;
    /// probe the sink's health once the cool-down is over. `true` when a probe ran.
    async fn probe_health(&mut self) -> bool {
        if self.permit() != Permit::AllowProbe {
            return false;
        }
        let checked = tokio::time::timeout(
            self.shared.settings.attempt_timeout,
            self.runtime.sink.health_check(),
        )
        .await;
        match classify(checked) {
            Ok(()) => self.breaker.record_success(),
            Err(ref error) => {
                lock(&self.runtime.stats).last_error = Some(error.class().clone());
                self.breaker.record_failure(error);
            }
        }
        self.sync_breaker();
        true
    }

    /// Wait for a pacing token; `false` when cancelled first.
    async fn pace(&mut self) -> bool {
        loop {
            let wait = match self.bucket.as_mut().map(TokenBucket::try_take) {
                None | Some(Ok(())) => return true,
                Some(Err(wait)) => wait,
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wait_cancelled(&mut self.cancel) => return false,
            }
        }
    }

    async fn read_open(&self) -> Option<Vec<(Uuid, DeliveryRecord)>> {
        let name = self.runtime.name.clone();
        match self
            .shared
            .blocking(move |store| store.open_deliveries(&name))
            .await
        {
            Ok(open) => {
                let mut stats = lock(&self.runtime.stats);
                let corrupt = to_u64(open.corrupt);
                if corrupt != stats.corrupt {
                    warn!(sink = %self.runtime.name, corrupt, "undecodable delivery rows are being skipped");
                }
                stats.corrupt = corrupt;
                // The store is the truth for the depth; the counters only keep it current
                // between passes.
                stats.dead_letters = to_u64(open.dead_letters);
                let now = self.shared.clock.now_ms();
                stats.oldest_pending_enqueue_ms = open
                    .rows
                    .iter()
                    .map(|entry| entry.1.enqueued_at_ms)
                    .min()
                    .filter(|&at| at <= now);
                drop(stats);
                Some(open.rows)
            }
            Err(error) => {
                error!(sink = %self.runtime.name, %error, "reading open deliveries failed");
                None
            }
        }
    }

    /// Move one batch of dead letters back to `Pending`; the report says how many are left.
    async fn redrive(&self) -> RedriveReport {
        let name = self.runtime.name.clone();
        let now = self.shared.clock.now_ms();
        let max_redrives = self.shared.settings.dlq_max_redrives;
        let max_open_rows = self.shared.settings.max_open_rows;
        let call = move |store: &crate::storage::EventStore| {
            store.redrive_batch(&name, REDRIVE_BATCH, max_open_rows, max_redrives, now)
        };
        match self.shared.blocking(call).await {
            Ok(report) => {
                lock(&self.runtime.stats).left_dead_letters(to_u64(report.moved));
                report
            }
            Err(error) => {
                error!(sink = %self.runtime.name, %error, "redrive failed");
                RedriveReport {
                    moved: 0,
                    remaining: 0,
                }
            }
        }
    }

    /// Redrive what is pending: a local sink as soon as open rows free up, a network sink once
    /// its breaker has stayed closed a full cool-down.
    async fn redrive_when_able(&mut self) {
        if !self.redrive_pending {
            return;
        }
        if !self.runtime.is_local {
            let Some(since) = self.breaker.closed_since() else {
                return;
            };
            if since.elapsed() < self.shared.settings.breaker_cooldown {
                return;
            }
        }
        self.redrive_pending = self.redrive().await.remaining > 0;
    }

    /// Load the alert, claim the row, send. The load comes first so a store fault costs no
    /// attempt.
    async fn attempt(&mut self, id: Uuid, row: &DeliveryRecord, probe: bool) -> Flow {
        let name = self.runtime.name.clone();
        let alert = match self
            .shared
            .blocking(move |store| store.get_alert(&id.to_string()))
            .await
        {
            Ok(Some(alert)) => alert,
            Ok(None) => return self.alert_missing(id, row, probe).await,
            Err(error) => {
                error!(sink = %name, alert_id = %id, %error, "loading the alert failed");
                self.release_probe(probe);
                return Flow::Stop;
            }
        };
        let now = self.shared.clock.now_ms();
        let claimed = match self
            .shared
            .blocking(move |store| store.claim(&name, id, now))
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => return self.unclaimed(probe, &error),
        };
        self.send(id, claimed.expected, claimed.record, &alert)
            .await
    }

    async fn send(
        &mut self,
        id: Uuid,
        expected: DeliveryExpectation,
        mut row: DeliveryRecord,
        alert: &Alert,
    ) -> Flow {
        let sent = tokio::time::timeout(
            self.shared.settings.attempt_timeout,
            self.runtime.sink.send(alert),
        )
        .await;
        let verdict = classify(sent);
        let now = self.shared.clock.now_ms();
        let is_dead = self.apply(&mut row, &verdict, now);
        self.record_outcome(&verdict, now);
        self.write_back(id, expected, row, is_dead).await
    }

    /// A claim that did not happen: stale rows are skipped, store failures stop the pass.
    fn unclaimed(&mut self, probe: bool, error: &StoreCallError) -> Flow {
        self.release_probe(probe);
        if error.is_stale() {
            debug!(sink = %self.runtime.name, "delivery row changed before the claim");
            return Flow::Continue;
        }
        error!(sink = %self.runtime.name, %error, "claiming a delivery failed");
        Flow::Stop
    }

    /// A probe that sent nothing must not leave the breaker half-open forever.
    fn release_probe(&mut self, probe: bool) {
        if !probe {
            return;
        }
        self.breaker.abandon_probe();
        self.sync_breaker();
    }

    async fn alert_missing(&mut self, id: Uuid, row: &DeliveryRecord, probe: bool) -> Flow {
        self.release_probe(probe);
        let now = self.shared.clock.now_ms();
        let expected = (row.state, row.attempts, row.redrives);
        let mut dead = row.clone();
        dead.dead_letter(DeliveryReason::AlertMissing, now);
        self.write_back(id, expected, dead, true).await
    }

    /// Fold the verdict into the row; `true` when the row is now dead-lettered.
    fn apply(
        &mut self,
        row: &mut DeliveryRecord,
        verdict: &Result<(), DeliveryError>,
        now: u64,
    ) -> bool {
        let error = match *verdict {
            Ok(()) => {
                row.deliver(now);
                return false;
            }
            Err(ref error) => error,
        };
        row.last_error = Some(error.class().clone());
        match *error {
            DeliveryError::Permanent { .. } => row.dead_letter(DeliveryReason::Permanent, now),
            DeliveryError::Misconfigured { .. } => {
                row.dead_letter(DeliveryReason::Misconfigured, now);
            }
            DeliveryError::Transient { retry_after, .. } => {
                if row.attempts >= self.policy.max_attempts() {
                    row.dead_letter(DeliveryReason::AttemptsExhausted, now);
                    return true;
                }
                let delay = retry_after.map_or_else(
                    || {
                        self.policy
                            .delay_for(row.attempts.saturating_sub(1), &mut self.rng)
                    },
                    |hint| hint.min(self.policy.cap()),
                );
                row.retry_at(now.saturating_add(millis(delay)));
                return false;
            }
        }
        true
    }

    /// Breaker and health bookkeeping for one finished send.
    fn record_outcome(&mut self, verdict: &Result<(), DeliveryError>, now: u64) {
        let is_local = self.runtime.is_local;
        let failing_at = if is_local {
            1
        } else {
            u64::from(self.shared.settings.breaker_threshold)
        };
        let mut stats = lock(&self.runtime.stats);
        stats.record_attempt(verdict.is_ok());
        match *verdict {
            Ok(()) => {
                if !is_local {
                    self.breaker.record_success();
                }
                stats.delivered = stats.delivered.saturating_add(1);
                stats.consecutive_failures = 0;
                stats.failing = false;
                stats.last_success_ms = Some(now);
                stats.misconfigured = false;
            }
            Err(ref error) => {
                stats.consecutive_failures = stats.consecutive_failures.saturating_add(1);
                stats.failing = stats.consecutive_failures >= failing_at;
                stats.last_error = Some(error.class().clone());
                if let DeliveryError::Misconfigured { .. } = *error {
                    stats.misconfigured = true;
                }
                if !is_local {
                    self.breaker.record_failure(error);
                }
            }
        }
        drop(stats);
        if !is_local {
            self.sync_breaker();
            // Dead letters are redriven only once a tripped breaker has recovered.
            if self.breaker.state() == BreakerState::Open {
                self.redrive_pending = true;
            }
        }
    }

    /// Compare-and-set the row; evict after a dead-letter transition.
    async fn write_back(
        &self,
        id: Uuid,
        expected: DeliveryExpectation,
        row: DeliveryRecord,
        is_dead: bool,
    ) -> Flow {
        let name = self.runtime.name.clone();
        let (reason, class, attempts) = (row.reason, row.last_error.clone(), row.attempts);
        let outcome = self
            .shared
            .blocking(move |store| store.complete(&name, id, expected, &row))
            .await;
        match outcome {
            Ok(CompleteOutcome::Written) => lock(&self.shared.global).store_ok(),
            Ok(_stale) => {
                warn!(sink = %self.runtime.name, alert_id = %id, "delivery row changed during the send; outcome dropped");
                return Flow::Continue;
            }
            Err(error) => {
                error!(sink = %self.runtime.name, alert_id = %id, %error, "writing a delivery outcome failed");
                return Flow::Stop;
            }
        }
        if is_dead {
            warn!(
                sink = %self.runtime.name,
                alert_id = %id,
                reason = ?reason,
                class = ?class,
                attempts,
                "alert delivery dead-lettered"
            );
            lock(&self.runtime.stats).dead_lettered();
            self.evict().await;
        }
        Flow::Continue
    }

    async fn evict(&self) {
        let name = self.runtime.name.clone();
        let keep = self.shared.settings.dlq_max_rows;
        match self
            .shared
            .blocking(move |store| store.evict_dead_letters(&name, keep))
            .await
        {
            Ok(0) => {}
            Ok(removed) => {
                warn!(sink = %self.runtime.name, removed, "dead letters evicted to hold the cap");
                let mut stats = lock(&self.runtime.stats);
                stats.evicted = stats.evicted.saturating_add(to_u64(removed));
                stats.left_dead_letters(to_u64(removed));
            }
            Err(error) => error!(sink = %self.runtime.name, %error, "evicting dead letters failed"),
        }
    }
}

/// Due now, or scheduled so far ahead that it can only be a clock or config change.
const fn is_due(row: &DeliveryRecord, now: u64, max_backoff_ms: u64) -> bool {
    row.next_attempt_ms <= now || row.next_attempt_ms > now.saturating_add(max_backoff_ms)
}

/// The verdict of a timed send or health check. A sink steers retry, breaker and dead-letter
/// handling by returning [`AlertingError::Delivery`]; the other variants are classified here.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "AlertingError is non_exhaustive; an unclassified failure is a transient io error"
)]
fn classify<T>(sent: Result<Result<T, AlertingError>, Elapsed>) -> Result<(), DeliveryError> {
    let transient = DeliveryError::transient;
    match sent {
        Err(_elapsed) => Err(transient(DeliveryErrorClass::Timeout)),
        Ok(Ok(_delivered)) => Ok(()),
        Ok(Err(AlertingError::Delivery(error))) => Err(error),
        Ok(Err(AlertingError::Timeout(_))) => Err(transient(DeliveryErrorClass::Timeout)),
        Ok(Err(AlertingError::IoError(error))) => Err(DeliveryError::from(error)),
        Ok(Err(AlertingError::SerializationError(_))) => Err(DeliveryError::Permanent {
            class: DeliveryErrorClass::Payload,
        }),
        Ok(Err(AlertingError::ConfigurationError(_))) => Err(DeliveryError::Misconfigured {
            class: DeliveryErrorClass::Config,
        }),
        Ok(Err(other)) => {
            warn!(error = %other, "unclassified sink error treated as a transient io failure");
            Err(transient(DeliveryErrorClass::Io))
        }
    }
}

/// Resolves once the cancellation is set or its sender is gone.
pub(super) async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    match cancel.wait_for(|&cancelled| cancelled).await {
        Ok(_) | Err(_) => {}
    }
}
