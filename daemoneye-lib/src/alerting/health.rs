//! Per-sink delivery health, published by the workers and read through
//! [`AlertManager::health`](super::AlertManager::health).

use super::reliability::{BreakerState, DeliveryErrorClass};
use serde::Serialize;
use std::collections::VecDeque;

/// Attempts the success rate is computed over.
pub(super) const SUCCESS_WINDOW: usize = 100;

/// Point-in-time health of one sink.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "a read-only snapshot of four independent flags; none is a state of another"
)]
pub struct SinkHealthSnapshot {
    /// Sink name.
    pub name: String,
    /// Whether the sink stays on the host (stdout, file).
    pub is_local: bool,
    /// Circuit breaker state; always closed for a local sink.
    pub breaker: BreakerState,
    /// Failed attempts of any class since the last success.
    pub consecutive_failures: u64,
    /// The failures since the last success reached the breaker threshold (one failure, for a
    /// local sink). Unlike the breaker, this also counts permanent rejections, so a sink that
    /// answers every alert with a refusal shows as failing.
    pub failing: bool,
    /// Send attempts since start.
    pub attempts: u64,
    /// Rows delivered since start.
    pub delivered: u64,
    /// Fraction of the last `SUCCESS_WINDOW` attempts that succeeded; `1.0` before any attempt.
    pub success_rate: f64,
    /// Current dead-letter depth for the sink (seeded from the store at start).
    pub dead_letters: u64,
    /// Dead letters removed to hold the per-sink cap.
    pub evicted: u64,
    /// Rows refused or displaced by the open-row cap.
    pub queue_full: u64,
    /// Rows found undecodable on the last read.
    pub corrupt: u64,
    /// Age of the oldest undelivered row at the last pass.
    pub oldest_pending_age_ms: Option<u64>,
    /// Class of the most recent failure.
    pub last_error: Option<DeliveryErrorClass>,
    /// Epoch milliseconds of the most recent delivery.
    pub last_success_ms: Option<u64>,
    /// The sink reported a configuration it can never satisfy; cleared by a success.
    pub misconfigured: bool,
    /// The sink cannot confirm receipt, so a delivery is only a send.
    pub fire_and_forget: bool,
}

impl SinkHealthSnapshot {
    /// An open or probing breaker, a misconfiguration, a run of failures (see
    /// [`failing`](Self::failing)), or undecodable rows the worker has to skip.
    #[must_use]
    pub const fn is_unhealthy(&self) -> bool {
        !matches!(self.breaker, BreakerState::Closed)
            || self.misconfigured
            || self.failing
            || self.corrupt > 0
    }
}

/// Roll-up of every sink and the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum DeliveryAggregate {
    /// Every sink is healthy and the store works.
    Healthy,
    /// A network sink is unhealthy; every local sink still delivers.
    Degraded,
    /// A local sink is unhealthy or the store is failing.
    Unhealthy,
}

/// Delivery health across the manager.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeliveryHealth {
    /// One snapshot per sink, in start order.
    pub sinks: Vec<SinkHealthSnapshot>,
    /// Store calls that failed since start.
    pub store_failures: u64,
    /// Publisher calls that failed since start.
    pub publish_failures: u64,
    /// Alerts the store refused, each sent once to the local sinks only.
    pub unpersisted_alerts: u64,
    /// Roll-up of the above.
    pub aggregate: DeliveryAggregate,
}

/// Counters shared by the manager and every worker.
#[derive(Debug, Default)]
pub(super) struct GlobalStats {
    pub(super) store_failures: u64,
    pub(super) publish_failures: u64,
    pub(super) unpersisted_alerts: u64,
    /// A store call failed and nothing has been written since.
    pub(super) store_failing: bool,
}

impl GlobalStats {
    pub(super) const fn store_failed(&mut self) {
        self.store_failures = self.store_failures.saturating_add(1);
        self.store_failing = true;
    }

    /// A write landed, so the store is taking writes again.
    pub(super) const fn store_ok(&mut self) {
        self.store_failing = false;
    }

    pub(super) const fn publish_failed(&mut self) {
        self.publish_failures = self.publish_failures.saturating_add(1);
    }

    pub(super) const fn unpersisted(&mut self) {
        self.unpersisted_alerts = self.unpersisted_alerts.saturating_add(1);
    }
}

/// Mutable counters of one sink, updated by its worker between awaits.
#[derive(Debug)]
pub(super) struct SinkStats {
    pub(super) breaker: BreakerState,
    pub(super) consecutive_failures: u64,
    pub(super) failing: bool,
    pub(super) attempts: u64,
    pub(super) delivered: u64,
    recent: VecDeque<bool>,
    pub(super) dead_letters: u64,
    pub(super) evicted: u64,
    pub(super) queue_full: u64,
    pub(super) corrupt: u64,
    pub(super) oldest_pending_enqueue_ms: Option<u64>,
    pub(super) last_error: Option<DeliveryErrorClass>,
    pub(super) last_success_ms: Option<u64>,
    pub(super) misconfigured: bool,
}

impl SinkStats {
    pub(super) const fn new() -> Self {
        Self {
            breaker: BreakerState::Closed,
            consecutive_failures: 0,
            failing: false,
            attempts: 0,
            delivered: 0,
            recent: VecDeque::new(),
            dead_letters: 0,
            evicted: 0,
            queue_full: 0,
            corrupt: 0,
            oldest_pending_enqueue_ms: None,
            last_error: None,
            last_success_ms: None,
            misconfigured: false,
        }
    }

    pub(super) fn record_attempt(&mut self, succeeded: bool) {
        self.attempts = self.attempts.saturating_add(1);
        if self.recent.len() >= SUCCESS_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(succeeded);
    }

    pub(super) const fn dead_lettered(&mut self) {
        self.dead_letters = self.dead_letters.saturating_add(1);
    }

    /// `rows` left the dead-letter queue (redriven or evicted).
    pub(super) const fn left_dead_letters(&mut self, rows: u64) {
        self.dead_letters = self.dead_letters.saturating_sub(rows);
    }

    pub(super) fn success_rate(&self) -> f64 {
        if self.recent.is_empty() {
            return 1.0;
        }
        let ok_count = self.recent.iter().filter(|&&ok| ok).count();
        let ok_rows = f64::from(u32::try_from(ok_count).unwrap_or(u32::MAX));
        let total = f64::from(u32::try_from(self.recent.len()).unwrap_or(u32::MAX));
        ok_rows / total
    }
}

/// Fold the snapshots and global flags into the roll-up.
pub(super) fn aggregate(sinks: &[SinkHealthSnapshot], store_failing: bool) -> DeliveryAggregate {
    if store_failing
        || sinks
            .iter()
            .any(|sink| sink.is_local && sink.is_unhealthy())
    {
        DeliveryAggregate::Unhealthy
    } else if sinks.iter().any(SinkHealthSnapshot::is_unhealthy) {
        DeliveryAggregate::Degraded
    } else {
        DeliveryAggregate::Healthy
    }
}
