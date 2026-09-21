//! Driving the pushed-task clock from the agent's periodic loop (R16).
//!
//! The renewal decisions themselves live in [`daemoneye_lib::detection::task_renewal`]; this
//! module is the thin part that turns them into IPC sends and reports which ones landed. It runs
//! on a clock and evaluates nothing.
//!
//! The engine is borrowed only *around* the sends, never across them: the cycle is taken first,
//! the due tasks are moved out of it, and the results are applied afterwards.

use std::time::SystemTime;

use daemoneye_lib::detection::DetectionEngine;
use daemoneye_lib::proto::DetectionTask;
use tracing::{debug, warn};

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
    engine: &mut DetectionEngine,
    dispatch: &D,
    now: SystemTime,
) -> RenewalOutcome
where
    D: TaskDispatch + ?Sized + Sync,
{
    let cycle = engine.renewal_cycle(now);
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

    for pending in cycle.into_due() {
        let task_id = pending.task_id().to_owned();
        let collector_id = pending.collector_id().to_owned();
        let sent = dispatch.send_task(&collector_id, pending.into_task()).await;
        match sent {
            Ok(()) => outcome.landed.push(task_id),
            Err(error) => {
                warn!(
                    task_id = %task_id,
                    collector_id = %collector_id,
                    error = %error,
                    "Pushdown task renewal did not reach its collector"
                );
                outcome.failed.push(task_id);
            }
        }
    }

    for task_id in &outcome.landed {
        let _tracked = engine.record_renewal(task_id, now);
    }
    if !outcome.landed.is_empty() {
        debug!(
            renewed = outcome.landed.len(),
            "Pushdown task renewals landed"
        );
    }
    outcome
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
