//! Alert enrichment (R19): process ancestry and host context, added before an alert is stored.
//!
//! Enrichment is best-effort. A lookup that cannot complete marks the alert
//! `enrichment=partial`; it never drops the alert or touches its `deduplication_key` or
//! `completeness`.

use crate::models::{Alert, ProcessRecord};
use crate::storage::{EventStore, StorageError, effective_start_ms};

/// Most ancestors written into `ancestry`.
const MAX_ANCESTORS: usize = 16;
/// Byte cap on every value written into the alert context.
const MAX_VALUE_BYTES: usize = 256;
const TAG_ENRICHED: &str = "enriched";
const TAG_PARTIAL: &str = "enrichment=partial";

/// Host facts stamped onto every enriched alert.
#[derive(Debug, Clone)]
pub struct EnrichContext {
    /// Name of the host running the agent.
    pub hostname: String,
    /// Version of the running agent.
    pub agent_version: String,
}

/// Return `alert` with ancestry, host and agent version written into its context.
///
/// `ancestry` lists up to 16 ancestors nearest-first as `pid:name` joined by `>`. Each ancestor
/// is the latest stored record for its pid that did not start after its child, so a recycled pid
/// cannot name an unrelated process. Synchronous and blocking: call it from `spawn_blocking`.
#[must_use]
pub fn enrich(store: &EventStore, alert: &Alert, ctx: &EnrichContext) -> Alert {
    enrich_with(alert, ctx, |pid, not_after_ms| {
        store.latest_by_pid(pid, not_after_ms)
    })
}

fn enrich_with<F>(alert: &Alert, ctx: &EnrichContext, lookup: F) -> Alert
where
    F: Fn(u32, u64) -> Result<Option<ProcessRecord>, StorageError>,
{
    let (ancestors, is_complete) = walk_ancestors(&alert.process_record, lookup);
    let mut out = alert
        .clone()
        .with_context_data("host", capped(&ctx.hostname))
        .with_context_data("agent_version", capped(&ctx.agent_version))
        .with_tag(TAG_ENRICHED);
    if !ancestors.is_empty() {
        out = out.with_context_data("ancestry", capped(&ancestors.join(">")));
    }
    if let Some(hash) = alert.process_record.executable_hash.as_ref() {
        out = out.with_context_data("exe_hash", capped(hash));
    }
    if is_complete {
        out
    } else {
        out.with_tag(TAG_PARTIAL)
    }
}

/// `pid:name` for each ancestor, nearest first, and whether the walk reached a root.
fn walk_ancestors<F>(child: &ProcessRecord, lookup: F) -> (Vec<String>, bool)
where
    F: Fn(u32, u64) -> Result<Option<ProcessRecord>, StorageError>,
{
    let mut entries = Vec::new();
    let mut current = child.clone();
    while let Some(ppid) = current.ppid {
        if entries.len() >= MAX_ANCESTORS {
            return (entries, false);
        }
        match lookup(ppid.raw(), effective_start_ms(&current)) {
            Ok(Some(parent)) => {
                entries.push(format!("{}:{}", parent.pid, parent.name));
                current = parent;
            }
            Ok(None) => return (entries, false),
            Err(error) => {
                tracing::warn!(%error, ppid = ppid.raw(), "ancestry lookup failed; alert marked partial");
                return (entries, false);
            }
        }
    }
    (entries, true)
}

/// `value` cut to at most [`MAX_VALUE_BYTES`] on a char boundary.
fn capped(value: &str) -> String {
    value
        .char_indices()
        .take_while(|&(index, ch)| index.saturating_add(ch.len_utf8()) <= MAX_VALUE_BYTES)
        .map(|(_, ch)| ch)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::models::{AlertSeverity, Completeness};

    fn ctx() -> EnrichContext {
        EnrichContext {
            hostname: "h".to_owned(),
            agent_version: "1".to_owned(),
        }
    }

    #[test]
    fn enrich_store_error_still_returns_the_alert_marked_partial() {
        let mut child = ProcessRecord::new(10, "c".to_owned());
        child.ppid = Some(crate::models::ProcessId::new(5));
        let alert = Alert::new(
            AlertSeverity::Low,
            "t",
            "d",
            "r",
            child,
            Completeness::complete(),
        );
        let out = enrich_with(&alert, &ctx(), |_, _| {
            Err(StorageError::Overflow {
                context: "test".to_owned(),
            })
        });
        assert!(out.context.tags.contains(&TAG_PARTIAL.to_owned()));
        assert!(out.context.tags.contains(&TAG_ENRICHED.to_owned()));
        assert_eq!(out.deduplication_key, alert.deduplication_key);
        assert_eq!(out.context.data.get("host").map(String::as_str), Some("h"));
    }

    #[test]
    fn enrich_caps_at_a_char_boundary() {
        let cut = capped(&"\u{e9}".repeat(300));
        assert_eq!(cut.len(), MAX_VALUE_BYTES);
        assert_eq!(capped("short"), "short");
    }
}
