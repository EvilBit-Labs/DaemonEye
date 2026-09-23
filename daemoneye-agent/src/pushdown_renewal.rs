//! Driving the pushed-task clock from the agent's periodic loop (R16).
//!
//! The renewal decisions themselves live in [`daemoneye_lib::detection::task_renewal`]; this
//! module is the thin part that turns them into IPC sends and reports which ones landed. It runs
//! on a clock and evaluates nothing.
//!
//! The engine lock is taken only *around* the sends, never across them: the cycle is taken under
//! the lock, the lock is released, the due tasks are sent, and the results are applied under the
//! lock again. The engine is shared with the admission gate, which takes it while holding the
//! registry's write lock, so a send-length hold here would serialize every registration behind
//! collector IPC.

use std::time::SystemTime;

use daemoneye_lib::detection::{DetectionEngine, PendingRenewal};
use daemoneye_lib::proto::DetectionTask;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::collector_admission::CollectorAdmission;

/// Delivers a pushed task to the collector that owns it.
///
/// The live implementation is the RPC broker; tests substitute a recording fake, which is what
/// makes the renewal cadence assertable without a running collector.
#[async_trait::async_trait]
pub trait TaskDispatch {
    /// Send `task` to `collector_id`, reporting why it did not arrive.
    async fn send_task(&self, collector_id: &str, task: DetectionTask) -> Result<(), String>;
}

/// What one renewal pass did, for the caller to log or assert on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenewalOutcome {
    /// Rules whose pushed half expired while the rule was still enabled (R16 into R12).
    pub expired_rules: Vec<String>,
    /// Tasks that reached their collector and had their lifetime extended.
    pub landed: Vec<String>,
    /// Tasks whose send failed. Two more attempts remain before the TTL elapses.
    pub failed: Vec<String>,
}

/// Run one renewal pass against `now`: retire what lapsed, send what is due, record what landed.
///
/// A send that fails is deliberately not retried here. The renewal interval is a third of the
/// TTL, so the next two passes are the retry, and a task that never lands expires and marks its
/// rule unhealthy — which is the observable outcome R16 asks for.
pub async fn run_renewal_cycle<D>(
    engine: &Mutex<DetectionEngine>,
    dispatch: &D,
    now: SystemTime,
) -> RenewalOutcome
where
    D: TaskDispatch + ?Sized + Sync,
{
    let cycle = {
        let mut guard = engine.lock().await;
        guard.renewal_cycle(now)
    };
    let mut outcome = RenewalOutcome {
        expired_rules: cycle.expired_rules().to_vec(),
        ..RenewalOutcome::default()
    };
    for rule_id in &outcome.expired_rules {
        warn!(
            rule_id = %rule_id,
            "Pushed half expired without a renewal landing; rule marked unhealthy"
        );
    }

    send_all(dispatch, cycle.into_due(), &mut outcome).await;

    if !outcome.landed.is_empty() {
        let mut guard = engine.lock().await;
        for task_id in &outcome.landed {
            let _tracked = guard.record_renewal(task_id, now);
        }
        drop(guard);
        debug!(
            renewed = outcome.landed.len(),
            "Pushdown task renewals landed"
        );
    }
    outcome
}

/// Re-issue the active task set of every collector that registered since the last pass (R16).
///
/// A collector that registers again has lost its accepted-task map, so every task addressed to it
/// is sent anew. Identifiers are derived from the rule and the collector, so a re-issue overwrites
/// in place on both sides rather than accumulating duplicates.
///
/// Called before [`run_renewal_cycle`] on the same tick: the re-issue refreshes each task's
/// confirmation time, so the renewal pass that follows sees them as not yet due and does not send
/// them a second time.
pub async fn reissue_registered_collectors<D>(
    engine: &Mutex<DetectionEngine>,
    admission: &CollectorAdmission,
    dispatch: &D,
    now: SystemTime,
) -> RenewalOutcome
where
    D: TaskDispatch + ?Sized + Sync,
{
    let mut outcome = RenewalOutcome::default();
    for collector_id in admission.drain_pending_reissue().await {
        let due = {
            let mut guard = engine.lock().await;
            guard.issue_tasks_for_collector(&collector_id, now)
        };
        if due.is_empty() {
            continue;
        }
        let issued = due.len();
        debug!(
            collector_id = %collector_id,
            tasks = issued,
            "Re-issuing the active task set after a collector registration"
        );
        send_all(dispatch, due, &mut outcome).await;
    }
    outcome
}

/// Send every pending task, recording which landed and which did not.
///
/// The engine lock is deliberately not held here, and no guard is in scope.
async fn send_all<D>(dispatch: &D, pending: Vec<PendingRenewal>, outcome: &mut RenewalOutcome)
where
    D: TaskDispatch + ?Sized + Sync,
{
    for task in pending {
        let task_id = task.task_id().to_owned();
        let collector_id = task.collector_id().to_owned();
        let sent = dispatch.send_task(&collector_id, task.into_task()).await;
        match sent {
            Ok(()) => outcome.landed.push(task_id),
            Err(error) => {
                warn!(
                    task_id = %task_id,
                    collector_id = %collector_id,
                    error = %error,
                    "Pushdown task did not reach its collector"
                );
                outcome.failed.push(task_id);
            }
        }
    }
}

#[async_trait::async_trait]
impl TaskDispatch for crate::broker_manager::BrokerManager {
    /// Deliver the task over the collector's RPC channel.
    ///
    /// A collector that answers with `success == false` refused the plan — it is advertising
    /// something the plan does not match — so the renewal is reported as not landed rather than
    /// silently counted.
    async fn send_task(&self, collector_id: &str, task: DetectionTask) -> Result<(), String> {
        let result = self
            .execute_task_rpc(collector_id, task)
            .await
            .map_err(|error| error.to_string())?;
        if result.success {
            return Ok(());
        }
        Err(result
            .error_message
            .unwrap_or_else(|| "collector refused the pushdown task".to_owned()))
    }
}
