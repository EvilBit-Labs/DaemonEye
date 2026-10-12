//! One worker per sink: claims rows, sends, writes the outcome back with compare-and-set.

use super::bucket::TokenBucket;
use super::{REDRIVE_BATCH, Shared, SinkRuntime, StoreCallError, WORKER_TICK, lock};
use crate::alerting::reliability::{
    BreakerState, CircuitBreaker, DeliveryError, DeliveryErrorClass, Permit, RetryPolicy,
};
use crate::alerting::{AlertingError, DeliveryResult};
use crate::models::{DeliveryReason, DeliveryRecord, DeliveryState, Uuid};
use crate::storage::{Claimed, CompleteOutcome};
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

    pub(super) async fn run(mut self) {
        let runtime = Arc::clone(&self.runtime);
        self.redrive_pending = self.redrive().await == REDRIVE_BATCH && !runtime.is_local;
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
        self.redrive_when_recovered().await;
        let Some(rows) = self.read_open().await else {
            return WORKER_TICK;
        };
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
            let flow = self.attempt(id, probe).await;
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
        lock(&self.runtime.stats).breaker = self.breaker.state();
        permit
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
                stats.corrupt = u64::try_from(open.corrupt).unwrap_or(u64::MAX);
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

    /// Move one batch of dead letters back to `Pending`; returns how many moved.
    async fn redrive(&self) -> usize {
        let name = self.runtime.name.clone();
        let now = self.shared.clock.now_ms();
        let max_redrives = self.shared.settings.dlq_max_redrives;
        let call = move |store: &crate::storage::EventStore| {
            store.redrive_batch(&name, REDRIVE_BATCH, max_redrives, now)
        };
        match self.shared.blocking(call).await {
            Ok(moved) => moved,
            Err(error) => {
                error!(sink = %self.runtime.name, %error, "redrive failed");
                0
            }
        }
    }

    /// Redrive once the breaker has stayed closed a full cool-down.
    async fn redrive_when_recovered(&mut self) {
        if !self.redrive_pending || self.runtime.is_local {
            return;
        }
        let Some(since) = self.breaker.closed_since() else {
            return;
        };
        if since.elapsed() >= self.shared.settings.breaker_cooldown {
            self.redrive_pending = self.redrive().await == REDRIVE_BATCH;
        }
    }

    async fn attempt(&mut self, id: Uuid, probe: bool) -> Flow {
        let name = self.runtime.name.clone();
        let now = self.shared.clock.now_ms();
        let claim_name = name.clone();
        let claimed = match self
            .shared
            .blocking(move |store| store.claim(&claim_name, id, now))
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => return self.unclaimed(probe, &error),
        };
        let alert = match self
            .shared
            .blocking(move |store| store.get_alert(&id.to_string()))
            .await
        {
            Ok(Some(alert)) => alert,
            Ok(None) => return self.alert_missing(id, claimed, probe).await,
            Err(error) => {
                error!(sink = %name, %error, "loading the alert failed");
                return Flow::Stop;
            }
        };
        let sent = tokio::time::timeout(
            self.shared.settings.attempt_timeout,
            self.runtime.sink.send(&alert),
        )
        .await;
        self.settle(id, claimed, classify(sent)).await
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
        self.breaker.record_failure(&DeliveryError::Transient {
            class: DeliveryErrorClass::Timeout,
            retry_after: None,
        });
        lock(&self.runtime.stats).breaker = self.breaker.state();
    }

    async fn alert_missing(&mut self, id: Uuid, claimed: Claimed, probe: bool) -> Flow {
        self.release_probe(probe);
        let now = self.shared.clock.now_ms();
        let mut row = claimed.record;
        dead_letter(&mut row, DeliveryReason::AlertMissing, now);
        warn!(sink = %self.runtime.name, "alert row missing; dead-lettering its delivery");
        self.write_back(id, claimed.expected, row, true).await
    }

    async fn settle(
        &mut self,
        id: Uuid,
        claimed: Claimed,
        verdict: Result<(), DeliveryError>,
    ) -> Flow {
        let now = self.shared.clock.now_ms();
        let mut row = claimed.record;
        let is_dead = self.apply(&mut row, &verdict, now);
        self.record_outcome(&verdict, now);
        self.write_back(id, claimed.expected, row, is_dead).await
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
                row.state = DeliveryState::Delivered;
                row.reason = None;
                row.terminal_at_ms = Some(now);
                return false;
            }
            Err(ref error) => error,
        };
        row.last_error = Some(error.class().clone());
        match *error {
            DeliveryError::Permanent { .. } => dead_letter(row, DeliveryReason::Permanent, now),
            DeliveryError::Misconfigured { .. } => {
                dead_letter(row, DeliveryReason::Misconfigured, now);
            }
            DeliveryError::Transient { retry_after, .. } => {
                if row.attempts >= self.policy.max_attempts {
                    dead_letter(row, DeliveryReason::AttemptsExhausted, now);
                    return true;
                }
                let delay = retry_after.map_or_else(
                    || {
                        self.policy
                            .delay_for(row.attempts.saturating_sub(1), &mut self.rng)
                    },
                    |hint| hint.min(self.policy.cap),
                );
                row.next_attempt_ms = now.saturating_add(millis(delay));
                return false;
            }
        }
        true
    }

    /// Breaker and health bookkeeping for one finished send.
    fn record_outcome(&mut self, verdict: &Result<(), DeliveryError>, now: u64) {
        let is_local = self.runtime.is_local;
        let mut stats = lock(&self.runtime.stats);
        stats.record_attempt(verdict.is_ok());
        match *verdict {
            Ok(()) => {
                if !is_local {
                    self.breaker.record_success();
                }
                stats.delivered = stats.delivered.saturating_add(1);
                stats.consecutive_failures = 0;
                stats.last_success_ms = Some(now);
                stats.misconfigured = false;
            }
            Err(ref error) => {
                stats.consecutive_failures = stats.consecutive_failures.saturating_add(1);
                stats.last_error = Some(error.class().clone());
                if let DeliveryError::Misconfigured { .. } = *error {
                    stats.misconfigured = true;
                }
                if !is_local {
                    // A permanent rejection is still an answer: it must not strand a probe.
                    if matches!(*error, DeliveryError::Permanent { .. })
                        && self.breaker.state() == BreakerState::HalfOpen
                    {
                        self.breaker.record_success();
                    } else {
                        self.breaker.record_failure(error);
                    }
                }
            }
        }
        stats.breaker = if is_local {
            BreakerState::Closed
        } else {
            self.breaker.state()
        };
        // Dead letters are redriven only once a tripped breaker has recovered.
        if stats.breaker == BreakerState::Open {
            self.redrive_pending = true;
        }
    }

    /// Compare-and-set the row; evict after a dead-letter transition.
    async fn write_back(
        &self,
        id: Uuid,
        expected: crate::storage::DeliveryExpectation,
        row: DeliveryRecord,
        is_dead: bool,
    ) -> Flow {
        let name = self.runtime.name.clone();
        let outcome = self
            .shared
            .blocking(move |store| store.complete(&name, id, expected, &row))
            .await;
        match outcome {
            Ok(CompleteOutcome::Written) => {}
            Ok(_stale) => {
                debug!(sink = %self.runtime.name, "delivery row changed during the send; outcome dropped");
                return Flow::Continue;
            }
            Err(error) => {
                error!(sink = %self.runtime.name, %error, "writing a delivery outcome failed");
                return Flow::Stop;
            }
        }
        if is_dead {
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
            Ok(removed) => {
                let mut stats = lock(&self.runtime.stats);
                stats.evicted = stats
                    .evicted
                    .saturating_add(u64::try_from(removed).unwrap_or(0));
            }
            Err(error) => error!(sink = %self.runtime.name, %error, "evicting dead letters failed"),
        }
    }
}

const fn dead_letter(row: &mut DeliveryRecord, reason: DeliveryReason, now: u64) {
    row.state = DeliveryState::DeadLettered;
    row.reason = Some(reason);
    row.terminal_at_ms = Some(now);
}

/// Due now, or scheduled so far ahead that it can only be a clock or config change.
const fn is_due(row: &DeliveryRecord, now: u64, max_backoff_ms: u64) -> bool {
    row.next_attempt_ms <= now || row.next_attempt_ms > now.saturating_add(max_backoff_ms)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "AlertingError is non_exhaustive; an unclassified failure is a transient io error"
)]
fn classify(
    sent: Result<Result<DeliveryResult, AlertingError>, Elapsed>,
) -> Result<(), DeliveryError> {
    let transient = |class| DeliveryError::Transient {
        class,
        retry_after: None,
    };
    match sent {
        Err(_elapsed) => Err(transient(DeliveryErrorClass::Timeout)),
        Ok(Ok(_delivered)) => Ok(()),
        Ok(Err(AlertingError::Delivery(error))) => Err(error),
        Ok(Err(AlertingError::Timeout(_))) => Err(transient(DeliveryErrorClass::Timeout)),
        Ok(Err(_other)) => Err(transient(DeliveryErrorClass::Io)),
    }
}

/// Resolves once the cancellation is set or its sender is gone.
pub(super) async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    match cancel.wait_for(|&cancelled| cancelled).await {
        Ok(_) | Err(_) => {}
    }
}
