//! Materialized Relation Cache (MRC) — in-memory parent map (T3 · U7).
//!
//! The MRC maps `pid → {ppid, parent_name, start_time}` so the most common
//! parent/child join becomes a single map lookup instead of a B-tree probe. It
//! is rebuilt on start from a bounded recent window of process records and is a
//! **cache, never a persistence dependency** — if absent it is rebuilt, never
//! recovered. Building the full posting-list page LRU is deferred to T6; only
//! this small, known-hot parent map is materialized in T3.

use crate::models::ProcessRecord;
use crate::models::process::ProcessId;
use std::collections::HashMap;
use std::time::SystemTime;

/// Cached parent lineage for a single pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentInfo {
    /// Parent process id, if known.
    pub ppid: Option<u32>,
    /// Parent process name, resolved from the recent window (`None` when the
    /// parent is outside the window — an orphan w.r.t. the cache).
    pub parent_name: Option<String>,
    /// The process's own start time.
    pub start_time: Option<SystemTime>,
}

/// In-memory materialized relation cache: `pid → ParentInfo`.
pub type MrcMap = HashMap<u32, ParentInfo>;

/// Build the MRC from a slice of recent process records. Two passes: first index
/// `pid → name` so a child's `parent_name` can be resolved from its `ppid`.
pub(super) fn build_mrc(records: &[ProcessRecord]) -> MrcMap {
    let names: HashMap<u32, &str> = records
        .iter()
        .map(|record| (record.pid.raw(), record.name.as_str()))
        .collect();

    records
        .iter()
        .map(|record| {
            let ppid = record.ppid.map(ProcessId::raw);
            let parent_name = ppid
                .and_then(|parent| names.get(&parent))
                .map(|name| (*name).to_owned());
            (
                record.pid.raw(),
                ParentInfo {
                    ppid,
                    parent_name,
                    start_time: record.start_time,
                },
            )
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::models::ProcessRecord;

    fn child_of(pid: u32, name: &str, ppid: u32) -> ProcessRecord {
        let mut record = ProcessRecord::new(pid, name.to_owned());
        record.ppid = Some(crate::models::process::ProcessId::new(ppid));
        record
    }

    #[test]
    fn build_mrc_resolves_parent_name_within_window() {
        let records = vec![
            ProcessRecord::new(1, "init".to_owned()),
            child_of(100, "bash", 1),
            child_of(200, "vim", 100),
        ];
        let mrc = build_mrc(&records);

        // bash's parent is init (pid 1).
        let bash = mrc.get(&100).expect("bash in mrc");
        assert_eq!(bash.ppid, Some(1));
        assert_eq!(bash.parent_name.as_deref(), Some("init"));

        // vim's parent is bash (pid 100).
        let vim = mrc.get(&200).expect("vim in mrc");
        assert_eq!(vim.parent_name.as_deref(), Some("bash"));

        // init has no parent recorded.
        assert_eq!(mrc.get(&1).expect("init in mrc").ppid, None);
    }

    #[test]
    fn build_mrc_leaves_parent_name_none_for_orphans() {
        // ppid 999 is not in the window → parent_name unresolved.
        let mrc = build_mrc(&[child_of(50, "orphan", 999)]);
        let orphan = mrc.get(&50).expect("orphan in mrc");
        assert_eq!(orphan.ppid, Some(999));
        assert_eq!(orphan.parent_name, None);
    }
}
