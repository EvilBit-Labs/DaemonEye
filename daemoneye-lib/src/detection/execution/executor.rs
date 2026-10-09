//! The rule executor: evaluates each runnable rule over the event store with `DataFusion`
//! (R1, R5, R7, R8; KTD3, KTD6, KTD7).
//!
//! [`RuleExecutor::evaluate`] takes a snapshot of [`RunnableRule`]s and never touches the engine:
//! the agent clones the snapshot under a short lock, runs it here with no lock held, and applies
//! the outcome under a second short lock. Two latency mechanisms follow from that and are not one:
//!
//! 1. **In `evaluate`, same cycle.** The rule's [`LatencySink`] carries its
//!    `pattern_latency_threshold`. The `regexp` UDF runs once per *scan* batch, so when a batch
//!    breaches, the UDF finishes it and refuses the next with a `LatencyAbort`, which `evaluate`
//!    turns into `stopped_on_latency`. This holds for selective rules, whose filter yields nothing
//!    between scan batches. The executor also checks after every *yielded* batch, as a second line
//!    for rules that yield (ADR-0011: bounded by one batch, not by one scan).
//! 2. **After `evaluate`, by the caller.** The [`LatencyReport`]s in [`CycleOutcome`] are applied
//!    through `DetectionEngine::observe_pattern_latency`, which disables the rule for future cycles.
//!
//! The `LatencySink` and the postings cache take synchronous locks. Neither guard is ever held
//! across an `.await`: the sink is drained into an owned map in one statement, and the cache is
//! only reached through the provider's own scan.

use std::sync::Arc;
use std::time::Duration;

use datafusion::arrow::array::{Array, RecordBatch};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::common::cast::{as_string_array, as_uint64_array};
use datafusion::error::{DataFusionError, Result as DfResult};
use datafusion::execution::context::SessionContext;
use futures_util::StreamExt;
use tracing::{Instrument, info_span, warn};

use crate::config::DetectionConfig;
use crate::detection::execution::completeness::{
    CompletenessTracker, CycleSignals, execution_reasons,
};
use crate::detection::execution::derive::{CycleWindow, derive_from_parts};
use crate::detection::execution::regexp::LatencyAbort;
use crate::detection::execution::session::{
    ExecutorRuntime, LatencySink, session_state_from_config,
};
use crate::detection::{Generation, RegexCache, RunnableRule};
use crate::models::{Alert, Completeness, DetectionRule, ProcessRecord};
use crate::proto::PushdownPlan;
use crate::storage::EventStore;
use crate::storage::postings_cache::PostingsCache;
use crate::storage::provider::{EventStoreTableProvider, ScanCounters, ScanLimits};

/// One pattern's worst batch latency, as measured during one evaluation.
///
/// Built at exactly one site, `take_reports`, so the generation is always the one on the
/// [`RunnableRule`] the plan came from and is never cached or recomputed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyReport {
    /// The rule the pattern belongs to.
    pub rule_id: String,
    /// The generation the evaluation was issued for; the caller's `observe_pattern_latency`
    /// ignores the report if the rule has since been reloaded.
    pub generation: Generation,
    /// The pattern text.
    pub pattern: String,
    /// The worst latency of one `RecordBatch` since the previous report for this rule.
    pub observed: Duration,
}

/// What a rule's scan read, copied from the provider's `ScanCounters` once the stream is done.
///
/// A non-zero `oversized_rows` becomes `CompletenessReason::ResourceLimit` naming `table`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTotals {
    /// The catalog table scanned.
    pub table: String,
    /// Rows decoded from the store.
    pub rows_read: u64,
    /// Rows excluded because one row alone exceeded the batch byte bound.
    pub oversized_rows: u64,
    /// `RecordBatch`es the scan sent.
    pub batches: u64,
}

impl ScanTotals {
    fn empty(table: &str) -> Self {
        Self {
            table: table.to_owned(),
            rows_read: 0,
            oversized_rows: 0,
            batches: 0,
        }
    }

    fn from_counters(counters: &ScanCounters) -> Self {
        Self {
            table: counters.table().to_owned(),
            rows_read: counters.rows_read(),
            oversized_rows: counters.oversized_rows(),
            batches: counters.batches(),
        }
    }
}

/// Why an evaluation did not finish; becomes `ExecutionError` or `ResourceLimit`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvaluationFailure {
    /// The plan could not be built or the stream failed.
    Execution(String),
    /// The engine's memory pool refused an allocation.
    ResourceLimit(String),
}

impl EvaluationFailure {
    fn from_error(error: &DataFusionError) -> Self {
        let message = error.to_string();
        if matches!(*error.find_root(), DataFusionError::ResourcesExhausted(_)) {
            Self::ResourceLimit(message)
        } else {
            Self::Execution(message)
        }
    }
}

/// The result of evaluating one rule for one cycle.
///
/// `completeness` is folded from the cycle's signals and this run's own `result_capped`, `scan`,
/// `stopped_on_latency` and `failure`; every alert carries a clone of it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RuleEvaluation {
    /// The rule evaluated.
    pub rule_id: String,
    /// The generation it was evaluated under; the agent drops the result unless the engine's
    /// `is_runnable(rule_id, generation)` still holds (R8).
    pub generation: Generation,
    /// One alert per matching row, at most the configured cap.
    pub alerts: Vec<Alert>,
    /// `Some(cap)` when more than `cap` rows matched; the alerts are the first `cap`. Exactly
    /// `cap` matches leave it `None`.
    pub result_capped: Option<u32>,
    /// What the scan read.
    pub scan: ScanTotals,
    /// A pattern breached the rule's latency threshold and the remaining batches were not read.
    pub stopped_on_latency: bool,
    /// Why the evaluation ended early, if it did.
    pub failure: Option<EvaluationFailure>,
    /// Whether this evaluation saw everything it was meant to, and if not, why (R14).
    pub completeness: Completeness,
}

/// Everything one cycle's evaluation produced.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CycleOutcome {
    /// One entry per rule, in the order given.
    pub evaluations: Vec<RuleEvaluation>,
    /// Every latency report, for the caller to apply with `observe_pattern_latency`.
    pub reports: Vec<LatencyReport>,
}

/// Runs rules against the event store; holds the `RuntimeEnv` (KTD3) and the postings cache.
#[derive(Debug)]
pub struct RuleExecutor {
    store: Arc<EventStore>,
    regex_cache: Arc<RegexCache>,
    postings: Arc<PostingsCache>,
    runtime: ExecutorRuntime,
    config: DetectionConfig,
}

impl RuleExecutor {
    /// An executor over `store`, sized by `config`.
    ///
    /// # Errors
    ///
    /// The `DataFusion` error if the runtime cannot be built.
    pub fn new(
        store: Arc<EventStore>,
        regex_cache: Arc<RegexCache>,
        config: &DetectionConfig,
    ) -> DfResult<Self> {
        Ok(Self {
            store,
            regex_cache,
            postings: Arc::new(PostingsCache::from_config(config)),
            runtime: ExecutorRuntime::from_config(config)?,
            config: config.clone(),
        })
    }

    /// Evaluate `rules` in order over the rows stored in `window`.
    ///
    /// Takes no engine lock and never calls the engine. `signals` is what the agent observed this
    /// cycle; each evaluation's `completeness` folds it for the rule's own collector and table.
    pub async fn evaluate(
        &self,
        rules: &[RunnableRule],
        window: CycleWindow,
        signals: &CycleSignals,
    ) -> CycleOutcome {
        let tracker = CompletenessTracker::new(signals);
        let mut outcome = CycleOutcome::default();
        for runnable in rules {
            let span = info_span!(
                "evaluate_rule",
                rule_id = runnable.rule.id.raw(),
                generation = %runnable.generation,
            );
            let (evaluation, reports) = self
                .evaluate_rule(runnable, window, &tracker)
                .instrument(span)
                .await;
            outcome.evaluations.push(evaluation);
            outcome.reports.extend(reports);
        }
        outcome
    }

    /// The `DataFusion` physical plan `evaluate` would run for `rule` over `window`, as text (R23).
    ///
    /// Built by the same `prepare` as `evaluate`, so it describes the plan that runs and cannot
    /// drift from it. It plans only; no row is read.
    ///
    /// # Errors
    ///
    /// The `DataFusion` error if the plan cannot be built or explained.
    pub async fn explain(&self, rule: &RunnableRule, window: CycleWindow) -> DfResult<String> {
        let sink = Arc::new(LatencySink::with_threshold(rule.pattern_latency_threshold));
        let (frame, _counters) = self.prepare(rule, window, &sink)?;
        let batches = frame.explain(false, false)?.collect().await?;
        Ok(pretty_format_batches(&batches)?.to_string())
    }

    async fn evaluate_rule(
        &self,
        runnable: &RunnableRule,
        window: CycleWindow,
        tracker: &CompletenessTracker,
    ) -> (RuleEvaluation, Vec<LatencyReport>) {
        let sink = Arc::new(LatencySink::with_threshold(
            runnable.pattern_latency_threshold,
        ));
        let mut run = Run::new(runnable);
        let prepared = self.prepare(runnable, window, &sink);
        let (frame, counters) = match prepared {
            Ok(parts) => parts,
            Err(error) => return run.finish_failed(&error, tracker),
        };
        run.scan = ScanTotals::from_counters(&counters);
        let mut stream = match frame.execute_stream().await {
            Ok(stream) => stream,
            Err(error) => return run.finish_failed(&error, tracker),
        };
        while let Some(next) = stream.next().await {
            match next.and_then(|batch| run.take_rows(&batch, self.config.max_matches_per_rule)) {
                Ok(()) => {}
                Err(error) if LatencyAbort::is_in(&error) => {
                    run.stopped_on_latency = true;
                    break;
                }
                Err(error) => {
                    run.failure = Some(EvaluationFailure::from_error(&error));
                    break;
                }
            }
            run.note_batch(take_reports(&sink, runnable));
            if run.result_capped.is_some() || run.stopped_on_latency {
                break;
            }
        }
        drop(stream);
        run.reports.extend(take_reports(&sink, runnable));
        run.scan = ScanTotals::from_counters(&counters);
        run.finish(tracker)
    }

    /// The derived frame and the counters of the provider behind it.
    fn prepare(
        &self,
        runnable: &RunnableRule,
        window: CycleWindow,
        sink: &Arc<LatencySink>,
    ) -> DfResult<(datafusion::prelude::DataFrame, Arc<ScanCounters>)> {
        let state = session_state_from_config(
            self.runtime.env(),
            Arc::clone(sink),
            Arc::clone(&self.regex_cache),
            &self.config,
        )?;
        let provider = Arc::new(
            EventStoreTableProvider::new(
                Arc::clone(&self.store),
                Arc::clone(&self.postings),
                &runnable.descriptor,
                ScanLimits::from(&self.config),
            )
            .map_err(|error| DataFusionError::Plan(error.to_string()))?,
        );
        let counters = Arc::clone(provider.counters());
        let ctx = SessionContext::new_with_state(state);
        let frame = derive_from_parts(
            &ctx,
            provider,
            &plan_with_pid(runnable),
            runnable.compiled.residual(),
            window,
            self.config.max_matches_per_rule,
        )
        .map_err(|error| DataFusionError::Plan(error.to_string()))?;
        Ok((frame, counters))
    }
}

/// The rule's pushed plan with `pid` added to a non-empty projection that lacks it.
///
/// An alert names the process it fired on, and the derived projection keeps only the columns a
/// rule selected and filtered on, so `SELECT name ... WHERE name = 'nc'` would otherwise reach the
/// alert with no pid at all. `pid` is the process's identifier and costs one column of decode.
/// An empty projection already means every column, and a table without a `pid` is left alone.
fn plan_with_pid(runnable: &RunnableRule) -> PushdownPlan {
    let mut plan = runnable.compiled.plan().clone();
    let has_pid_column = runnable
        .descriptor
        .columns
        .iter()
        .any(|column| column.name == PID_COLUMN);
    let selected = plan.projection.iter().any(|name| name == PID_COLUMN);
    if has_pid_column && !plan.projection.is_empty() && !selected {
        plan.projection.push(PID_COLUMN.to_owned());
    }
    plan
}

/// Drain `sink` into reports. The only place a [`LatencyReport`] is constructed.
fn take_reports(sink: &LatencySink, runnable: &RunnableRule) -> Vec<LatencyReport> {
    sink.drain()
        .into_iter()
        .map(|(pattern, observed)| LatencyReport {
            rule_id: runnable.rule.id.raw().to_owned(),
            generation: runnable.generation,
            pattern,
            observed,
        })
        .collect()
}

/// The column an alert's process is identified by.
const PID_COLUMN: &str = "pid";

/// One matching row, reduced to what an alert names.
struct Hit {
    pid: u32,
    name: String,
}

/// One rule's evaluation while it is in progress.
struct Run<'a> {
    runnable: &'a RunnableRule,
    hits: Vec<Hit>,
    reports: Vec<LatencyReport>,
    result_capped: Option<u32>,
    scan: ScanTotals,
    stopped_on_latency: bool,
    failure: Option<EvaluationFailure>,
}

impl<'a> Run<'a> {
    fn new(runnable: &'a RunnableRule) -> Self {
        Self {
            runnable,
            hits: Vec::new(),
            reports: Vec::new(),
            result_capped: None,
            scan: ScanTotals::empty(&runnable.descriptor.name),
            stopped_on_latency: false,
            failure: None,
        }
    }

    /// Turn up to the remaining room's worth of `batch` rows into alerts; flag the cap when rows
    /// are left over. The plan fetches `cap + 1`, so one surplus row is what tells "more than
    /// `cap`" from "exactly `cap`".
    fn take_rows(&mut self, batch: &RecordBatch, cap: u32) -> DfResult<()> {
        let room = usize::try_from(cap)
            .unwrap_or(usize::MAX)
            .saturating_sub(self.hits.len());
        if batch.num_rows() > room {
            self.result_capped = Some(cap);
        }
        self.hits.extend(hits_from_batch(batch, room)?);
        Ok(())
    }

    /// Record a batch's drained reports and decide whether any pattern breached the threshold.
    fn note_batch(&mut self, reports: Vec<LatencyReport>) {
        let threshold = self.runnable.pattern_latency_threshold;
        for report in &reports {
            if report.observed > threshold {
                self.stopped_on_latency = true;
                warn!(
                    pattern = %report.pattern,
                    observed = ?report.observed,
                    threshold = ?threshold,
                    "pattern latency breached; no further batches read for this rule this cycle",
                );
            }
        }
        self.reports.extend(reports);
    }

    fn finish_failed(
        mut self,
        error: &DataFusionError,
        tracker: &CompletenessTracker,
    ) -> (RuleEvaluation, Vec<LatencyReport>) {
        self.failure = Some(EvaluationFailure::from_error(error));
        self.finish(tracker)
    }

    /// Fold the cycle's signals and this run's own shortfalls into the completeness, then build
    /// the alerts carrying it. Alerts are built here, not as rows arrive, because the reasons are
    /// not all known until the stream ends.
    fn finish(self, tracker: &CompletenessTracker) -> (RuleEvaluation, Vec<LatencyReport>) {
        let own_reasons = execution_reasons(
            self.failure.as_ref(),
            self.stopped_on_latency,
            &self.scan,
            self.result_capped,
        );
        let completeness = tracker.for_rule(
            self.runnable.compiled.collector_id(),
            &self.scan.table,
            own_reasons,
        );
        let alerts = self
            .hits
            .iter()
            .map(|hit| alert_for(&self.runnable.rule, hit, &completeness))
            .collect();
        let evaluation = RuleEvaluation {
            rule_id: self.runnable.rule.id.raw().to_owned(),
            generation: self.runnable.generation,
            alerts,
            result_capped: self.result_capped,
            scan: self.scan,
            stopped_on_latency: self.stopped_on_latency,
            failure: self.failure,
            completeness,
        };
        (evaluation, self.reports)
    }
}

/// The first `take` rows of `batch` as hits.
///
/// The row carries the columns the rule projected plus those it filtered on, and `pid` always
/// (`plan_with_pid`). A table with no `pid` column yields pid 0, the one case left where an alert
/// cannot name its process; a null `name` is the empty string.
fn hits_from_batch(batch: &RecordBatch, take: usize) -> DfResult<Vec<Hit>> {
    let pids = batch
        .column_by_name(PID_COLUMN)
        .map(|column| as_uint64_array(column.as_ref()))
        .transpose()?;
    let names = batch
        .column_by_name("name")
        .map(|column| as_string_array(column.as_ref()))
        .transpose()?;
    let mut hits = Vec::with_capacity(take.min(batch.num_rows()));
    for row in 0..batch.num_rows().min(take) {
        let pid = pids
            .filter(|column| column.is_valid(row))
            .map_or(0, |column| {
                u32::try_from(column.value(row)).unwrap_or(u32::MAX)
            });
        let name = names
            .filter(|column| column.is_valid(row))
            .map_or_else(String::new, |column| column.value(row).to_owned());
        hits.push(Hit { pid, name });
    }
    Ok(hits)
}

/// One alert for `hit`. The title names the process, because the deduplication key is built from
/// it and one title for every match would collapse a rule's matches into one alert downstream.
fn alert_for(rule: &DetectionRule, hit: &Hit, completeness: &Completeness) -> Alert {
    let (pid, name) = (hit.pid, &hit.name);
    Alert::new(
        rule.severity,
        format!("{}: {name} (pid {pid})", rule.name),
        format!("Process {name} (pid {pid}) matched rule {}", rule.name),
        rule.id.raw().to_owned(),
        ProcessRecord::new(pid, name.clone()),
        completeness.clone(),
    )
}
