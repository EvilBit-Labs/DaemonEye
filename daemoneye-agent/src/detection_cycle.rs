//! The agent's half of a detection cycle: ingest what was collected, load the
//! rules that were persisted, evaluate them through the executor, enqueue the alerts for
//! delivery.
//!
//! These are free functions over their dependencies so tests build them over a `tempfile` store.
//!
//! # `source_seq`
//!
//! A cycle's rows carry `source_seq = (cycle_ordinal << 32) | row_index`. The ingest watermark
//! treats the first row of the next ordinal as contiguous with the last row of this one, so a
//! cycle boundary is not a gap, while a skipped ordinal or a skipped row is. The ordinal is not
//! stored on its own: it is the high half of the watermark the event store commits with the rows,
//! so it can never drift from the data it protects.

use daemoneye_lib::alerting::{AlertManager, EnqueueOutcome, EnrichContext, enrich};
use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::detection::execution::completeness::{
    CollectorHealth, CycleSignals, EvaluationSummary, IngestSnapshot,
};
use daemoneye_lib::detection::execution::derive::CycleWindow;
use daemoneye_lib::detection::execution::executor::{LatencyReport, RuleEvaluation, RuleExecutor};
use daemoneye_lib::models::{Alert, CompletenessStatus, ProcessRecord};
use daemoneye_lib::storage::ingest::{
    EPOCH_SHIFT, IngestError, IngestHandle, IngestRecord, SequenceGap,
};
use daemoneye_lib::storage::{EventStore, StorageError};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{debug, info, warn};

/// The collector id the agent files procmond's rows under.
pub const PROCMOND_COLLECTOR_ID: &str = "procmond";

/// What one cycle's ingest did, whether or not it finished.
///
/// The bounds cover every row submitted before a failure too: a batch committed before a later
/// flush failure holds rows the next window must still reach.
#[derive(Debug)]
pub struct IngestOutcome {
    /// The largest `collection_time` (ms) submitted; `0` when nothing was.
    pub high_water_ms: u64,
    /// The smallest `collection_time` (ms) submitted; `u64::MAX` when nothing was.
    pub low_water_ms: u64,
    /// How many rows were submitted (before the watermark discarded any).
    pub submitted: usize,
    /// Sequence gaps the writer saw since the previous flush; empty after a failure.
    pub gaps: Vec<SequenceGap>,
    /// Why the cycle's rows are not all durable, when they are not.
    pub failure: Option<CycleIngestError>,
}

impl IngestOutcome {
    const fn empty() -> Self {
        Self {
            high_water_ms: 0,
            low_water_ms: u64::MAX,
            submitted: 0,
            gaps: Vec::new(),
            failure: None,
        }
    }

    fn failed(mut self, error: CycleIngestError) -> Self {
        self.failure = Some(error);
        self
    }

    /// The outcome as a `Result`, for a caller that has no use for a failed cycle's bounds.
    ///
    /// # Errors
    ///
    /// The failure the outcome carries.
    pub fn into_result(self) -> Result<Self, CycleIngestError> {
        match self.failure {
            Some(error) => Err(error),
            None => Ok(self),
        }
    }
}

/// Why a cycle's rows could not be ingested.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CycleIngestError {
    /// The pipeline refused a row or failed the flush barrier.
    #[error("ingest failed: {0}")]
    Ingest(#[from] IngestError),
    /// More rows than a cycle's 32-bit row index can number. Wrapping would alias the next
    /// ordinal's sequence space and get fresh rows discarded as duplicates, so it is refused.
    #[error("cycle has more rows than a 32-bit row index can number")]
    TooManyRows,
    /// The cycle ordinal space is exhausted.
    #[error("cycle ordinal space exhausted")]
    OrdinalExhausted,
    /// The stored watermark could not be read.
    #[error("reading the ingest watermark failed: {0}")]
    Storage(#[from] StorageError),
}

fn source_seq(cycle_ordinal: u32, row_index: u32) -> u64 {
    (u64::from(cycle_ordinal) << EPOCH_SHIFT) | u64::from(row_index)
}

/// Submit one cycle's rows and wait for them to commit.
///
/// Returns only after the group commit, so a caller that evaluates rules next never races the
/// writer. The key time of each row is its own `collection_time`; see `IngestRecord`.
///
/// # Errors
///
/// [`CycleIngestError`] if a row cannot be keyed or numbered, the pipeline is closed, or a commit
/// failed.
pub async fn ingest_cycle(
    handle: &IngestHandle,
    collector_id: &str,
    cycle_ordinal: u32,
    records: &[ProcessRecord],
) -> IngestOutcome {
    let mut outcome = IngestOutcome::empty();
    for (index, record) in records.iter().enumerate() {
        let row = match ingest_row(collector_id, cycle_ordinal, index, record) {
            Ok(row) => row,
            Err(error) => return outcome.failed(error),
        };
        outcome.high_water_ms = outcome.high_water_ms.max(row.ts_ms());
        outcome.low_water_ms = outcome.low_water_ms.min(row.ts_ms());
        outcome.submitted = outcome.submitted.saturating_add(1);
        if let Err(error) = handle.submit(row).await {
            return outcome.failed(error.into());
        }
    }
    match handle.flush().await {
        Ok(report) => outcome.gaps = report.gaps,
        Err(error) => outcome.failure = Some(error.into()),
    }
    outcome
}

fn ingest_row(
    collector_id: &str,
    cycle_ordinal: u32,
    index: usize,
    record: &ProcessRecord,
) -> Result<IngestRecord, CycleIngestError> {
    let row_index = u32::try_from(index).map_err(|_overflow| CycleIngestError::TooManyRows)?;
    Ok(IngestRecord::new(
        collector_id,
        source_seq(cycle_ordinal, row_index),
        row_index,
        record.clone(),
    )?)
}

/// What a cycle carries forward from its ingest attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestStep {
    /// The ordinal the next cycle uses. A failed cycle still consumes its ordinal: a batch of it
    /// may have committed and raised the watermark, and a retry under the same ordinal would have
    /// its fresh rows discarded as duplicates.
    pub next_ordinal: Option<u32>,
    /// The window this cycle evaluates. Its `through_ms` is the highest `collection_time` now
    /// committed, unchanged by a failure. Its `after_ms` is the previous mark, unless a row was
    /// stamped at or before it (a clock step-back): then the window reaches back to that row, and
    /// the rows in the overlap are evaluated again rather than never (ADR-0014). A failed cycle
    /// reaches back too, because a batch committed before the failure may hold that row.
    pub window: CycleWindow,
    /// Gaps the writer saw since the previous flush.
    pub gaps: Vec<SequenceGap>,
    /// Why this cycle's rows are not durable, when they are not.
    pub failure: Option<String>,
    /// The furthest `collection_time` this attempt may have committed, whether or not it finished:
    /// what a [`CarriedFailure`] must outlast.
    pub rows_through_ms: u64,
}

/// An ingest failure that outlives its cycle.
///
/// A flush can fail after earlier batches of the same attempt committed. The mark does not move,
/// so a later window evaluates those rows, and that window is as incomplete as the failed cycle
/// was: rows of the attempt that never committed are missing from it. The failure is therefore
/// carried, and every window that starts before the attempt's furthest row is degraded by it,
/// until one reaches that row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedFailure {
    /// The furthest `collection_time` the failed attempt may have committed.
    pub rows_through_ms: u64,
    /// The failure as the cycle reported it.
    pub error: String,
}

impl CarriedFailure {
    /// The failure `step` leaves behind, if it failed; otherwise what was carried before.
    #[must_use]
    pub fn after(previous: Option<Self>, step: &IngestStep) -> Option<Self> {
        step.failure.as_ref().map_or(previous, |error| {
            Some(Self {
                rows_through_ms: step.rows_through_ms,
                error: error.clone(),
            })
        })
    }

    /// Whether `window` may hold rows of the failed attempt, so its evaluations are degraded.
    #[must_use]
    pub const fn degrades(&self, window: CycleWindow) -> bool {
        window.after_ms < self.rows_through_ms
    }

    /// Whether `window` reaches the attempt's furthest row, after which nothing is left to carry.
    #[must_use]
    pub const fn is_settled_by(&self, window: CycleWindow) -> bool {
        window.through_ms >= self.rows_through_ms
    }
}

/// Fold one cycle's ingest result into what the next cycle and this cycle's completeness need.
#[must_use]
pub fn ingest_step(
    ordinal: u32,
    previous_high_water_ms: u64,
    outcome: IngestOutcome,
) -> IngestStep {
    // The mark advances only over rows known to be committed.
    let through_ms = if outcome.failure.is_some() {
        previous_high_water_ms
    } else {
        previous_high_water_ms.max(outcome.high_water_ms)
    };
    IngestStep {
        next_ordinal: ordinal.checked_add(1),
        window: CycleWindow {
            after_ms: previous_high_water_ms.min(outcome.low_water_ms.saturating_sub(1)),
            through_ms,
        },
        gaps: outcome.gaps,
        failure: outcome.failure.map(|error| error.to_string()),
        rows_through_ms: previous_high_water_ms.max(outcome.high_water_ms),
    }
}

/// The ordinal the next cycle should use: one above the last one committed for `collector_id`,
/// or `0` for a store that has none.
///
/// # Errors
///
/// [`CycleIngestError::OrdinalExhausted`] at the last ordinal, [`CycleIngestError::Storage`] if
/// the watermark cannot be read.
pub fn next_cycle_ordinal(store: &EventStore, collector_id: &str) -> Result<u32, CycleIngestError> {
    let marks = store.ingest_watermarks()?;
    let Some(mark) = marks.get(collector_id) else {
        return Ok(0);
    };
    let committed = u32::try_from(mark.checked_shr(EPOCH_SHIFT).unwrap_or(0))
        .map_err(|_overflow| CycleIngestError::OrdinalExhausted)?;
    committed
        .checked_add(1)
        .ok_or(CycleIngestError::OrdinalExhausted)
}

/// Load every persisted rule into `engine`, logging each one the engine rejects with its id.
/// Returns how many loaded.
///
/// # Errors
///
/// [`StorageError`] if the persisted rules cannot be read.
pub fn load_persisted_rules(
    store: &EventStore,
    engine: &mut DetectionEngine,
) -> Result<usize, StorageError> {
    let mut loaded = 0_usize;
    for rule in store.get_all_rules()? {
        let rule_id = rule.id.raw().to_owned();
        match engine.load_rule(rule) {
            Ok(()) => loaded = loaded.saturating_add(1),
            Err(error) => {
                warn!(rule_id = %rule_id, error = %error, "persisted rule rejected at load")
            }
        }
    }
    Ok(loaded)
}

/// How often the agent sweeps expired buckets and terminal delivery rows.
pub const RETENTION_SWEEP: Duration = Duration::from_hours(1);

/// Enrich every alert, then enqueue it for delivery, returning how many are durable.
///
/// Enrichment reads the store synchronously, so the whole batch runs on one blocking thread;
/// the enqueue awaits stay on the runtime. An enqueued, already-enqueued or suppressed alert
/// counts as persisted. One the store would not take is delivered once to the local sinks only,
/// and does not count.
pub async fn enrich_and_enqueue(
    store: &Arc<EventStore>,
    manager: &AlertManager,
    alerts: &[Alert],
    ctx: &EnrichContext,
) -> usize {
    if alerts.is_empty() {
        return 0;
    }
    let (blocking_store, batch, context) = (Arc::clone(store), alerts.to_vec(), ctx.clone());
    let enriched = tokio::task::spawn_blocking(move || {
        batch
            .iter()
            .map(|alert| enrich(&blocking_store, alert, &context))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_else(|error| {
        warn!(%error, "alert enrichment task failed; enqueuing alerts unenriched");
        alerts.to_vec()
    });
    let mut persisted = 0_usize;
    for alert in &enriched {
        match manager.enqueue(alert).await {
            EnqueueOutcome::Enqueued
            | EnqueueOutcome::AlreadyEnqueued
            | EnqueueOutcome::SuppressedDuplicate { .. } => {
                persisted = persisted.saturating_add(1);
            }
            EnqueueOutcome::NotPersisted | _ => {
                warn!(alert_id = %alert.id, "alert not persisted; delivering to local sinks only");
                manager.deliver_unpersisted(alert).await;
            }
        }
    }
    persisted
}

/// Drop expired event buckets and prune terminal delivery rows older than `retention_ms`.
/// Returns `(buckets_dropped, delivery_rows_pruned)`. Synchronous: call it from `spawn_blocking`.
///
/// # Errors
///
/// The first [`StorageError`] either step hits; the other step still runs first.
pub fn run_retention_sweep(
    store: &EventStore,
    now_ms: u64,
    retention_ms: u64,
) -> anyhow::Result<(usize, usize)> {
    let buckets = store.drop_expired(now_ms);
    let rows = store.prune_deliveries(now_ms, retention_ms);
    Ok((buckets?, rows?))
}

/// Open the event store, turning a schema mismatch into an error that says how to recover.
///
/// # Errors
///
/// Any [`StorageError`] opening the store (redb's page cache is capped at
/// `database.page_cache_mb`); a schema mismatch names the rebuild path.
pub fn open_event_store(
    path: &Path,
    database: &daemoneye_lib::config::DatabaseConfig,
) -> Result<EventStore, anyhow::Error> {
    EventStore::new_configured(path, database).map_err(|error| {
        if let &StorageError::SchemaVersionMismatch { found, expected } = &error {
            return anyhow::anyhow!(
                "event store at {} has schema version {found}, this binary expects {expected}; \
                 rebuild it with the signed export and rebuild path (storage::schema::migrate), \
                 which requires a signed bundle",
                path.display()
            );
        }
        // Unwrapped: the storage errors already name the path, and the CLI test pins their text.
        anyhow::Error::new(error)
    })
}

/// Where the agent keeps its [`DetectionEngine`]: a lock it can be asked for.
///
/// The production implementation is the `tokio::sync::Mutex` itself. It is a trait so a test can
/// count acquisitions and interleave a reload between the two of them, which is the only way to
/// observe the lock scopes of [`run_detection_cycle`] from outside.
pub trait EngineCell: Sync {
    /// Wait for the engine.
    fn lock(&self) -> impl Future<Output = MutexGuard<'_, DetectionEngine>> + Send;
}

impl EngineCell for Mutex<DetectionEngine> {
    fn lock(&self) -> impl Future<Output = MutexGuard<'_, DetectionEngine>> + Send {
        Self::lock(self)
    }
}

/// What one detection cycle produced.
#[derive(Debug, Clone, Default)]
pub struct CycleResult {
    /// The alerts of every evaluation whose rule was still eligible afterwards.
    pub alerts: Vec<Alert>,
    /// Every evaluation the executor returned, kept or dropped, so a caller can tell a rule that
    /// found nothing from one whose findings were dropped.
    pub evaluations: Vec<RuleEvaluation>,
    /// How many evaluations were dropped because their rule was reloaded, disabled or latched
    /// while they ran (R8).
    pub dropped_after_reeligibility: usize,
}

/// Evaluate every eligible rule over `window` (R8, R20).
///
/// The engine lock is taken twice, around a lock-free evaluation, and never held across an
/// `.await` (`clippy::await_holding_lock` is the compile-time proof). Alerts are returned only for
/// evaluations whose rule and generation are still eligible after execution.
pub async fn run_detection_cycle(
    engine: &impl EngineCell,
    executor: &RuleExecutor,
    window: CycleWindow,
    signals: &CycleSignals,
) -> CycleResult {
    // Scope 1: clone what may run. It ends here because the executor awaits on I/O, and the
    // admission gate takes this same lock on every collector registration.
    let runnable = {
        let guard = engine.lock().await;
        guard.runnable_rules()
    };
    if runnable.is_empty() {
        return CycleResult::default();
    }

    // Scope 2 (no lock): the executor holds no guard, so a scan of any length blocks nothing.
    let started = Instant::now();
    let outcome = executor.evaluate(&runnable, window, signals).await;
    let elapsed = started.elapsed();
    let reports = outcome.reports;
    let evaluations = outcome.evaluations;

    // Scope 3 (the second lock): apply the latency reports, then ask the one eligibility predicate
    // again. It ends here because everything after is cloning alerts, which needs no engine. A
    // report is applied before the check so a rule that breached in this very cycle is dropped too.
    let keep = apply_outcome(&mut *engine.lock().await, &reports, &evaluations);

    log_cycle(&evaluations, &reports, elapsed);

    let mut alerts = Vec::new();
    let mut dropped = 0_usize;
    for (evaluation, kept) in evaluations.iter().zip(keep) {
        if kept {
            alerts.extend(evaluation.alerts.iter().cloned());
        } else {
            dropped = dropped.saturating_add(1);
        }
    }
    CycleResult {
        alerts,
        evaluations,
        dropped_after_reeligibility: dropped,
    }
}

/// Milliseconds in `duration`, saturating at `u64::MAX`.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Report what the cycle did (R22): one `info` event for the cycle, one `debug` per rule, and a
/// `warn` for each rule whose evaluation was degraded. Every value is extracted into a binding
/// before its macro, so no `.await` or borrow sits inside a `tracing` call.
fn log_cycle(evaluations: &[RuleEvaluation], reports: &[LatencyReport], elapsed: Duration) {
    let (mut matches, mut degraded_rules, mut rows_scanned, mut batches) =
        (0_usize, 0_usize, 0_u64, 0_u64);
    for evaluation in evaluations {
        let rule_matches = evaluation.alerts.len();
        let rule_degraded = evaluation.completeness.status() == CompletenessStatus::Degraded;
        let rule_id = evaluation.rule_id.as_str();
        let rule_batches = evaluation.scan.batches;
        let reasons = evaluation.completeness.reasons();
        debug!(
            rule_id,
            matches = rule_matches,
            batches = rule_batches,
            degraded = rule_degraded,
            reasons = ?reasons,
            "rule evaluated",
        );
        if rule_degraded {
            warn!(rule_id, reasons = ?reasons, "rule evaluation degraded");
            degraded_rules = degraded_rules.saturating_add(1);
        }
        matches = matches.saturating_add(rule_matches);
        rows_scanned = rows_scanned.saturating_add(evaluation.scan.rows_read);
        batches = batches.saturating_add(rule_batches);
    }
    let rules_evaluated = evaluations.len();
    let evaluation_ms = millis(elapsed);
    let max_pattern_latency_ms = reports
        .iter()
        .map(|report| millis(report.observed))
        .max()
        .unwrap_or(0);
    info!(
        rules_evaluated,
        matches,
        degraded_rules,
        rows_scanned,
        batches,
        evaluation_ms,
        max_pattern_latency_ms,
        "detection cycle evaluated",
    );
}

/// Apply one cycle's latency reports and decide which evaluations still count (synchronous, so
/// the guard its caller holds can never span an `.await`).
fn apply_outcome(
    engine: &mut DetectionEngine,
    reports: &[LatencyReport],
    evaluations: &[RuleEvaluation],
) -> Vec<bool> {
    for report in reports {
        let _breached =
            engine.observe_pattern_latency(&report.rule_id, report.generation, report.observed);
    }
    evaluations
        .iter()
        .map(|evaluation| {
            let still_runnable = engine.is_runnable(&evaluation.rule_id, evaluation.generation);
            if still_runnable {
                engine.record_evaluation(EvaluationSummary::of(evaluation));
            }
            still_runnable
        })
        .collect()
}

/// The window of the cycle that ingested rows up to `ingested_high_water_ms`, following one that
/// ended at `previous_high_water_ms`.
#[must_use]
pub const fn next_window(previous_high_water_ms: u64, ingested_high_water_ms: u64) -> CycleWindow {
    CycleWindow {
        after_ms: previous_high_water_ms,
        through_ms: ingested_high_water_ms,
    }
}

/// What the agent observed about one cycle, for the collector it owns.
#[must_use]
pub fn build_signals(
    collector_id: &str,
    collection: Result<(), String>,
    heartbeat: Option<CollectorHealth>,
    ingest: IngestSnapshot,
) -> CycleSignals {
    CycleSignals {
        collection: BTreeMap::from([(collector_id.to_owned(), collection)]),
        heartbeat: heartbeat
            .map(|health| (collector_id.to_owned(), health))
            .into_iter()
            .collect(),
        ingest,
    }
}
