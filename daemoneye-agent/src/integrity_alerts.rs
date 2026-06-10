//! Agent-side integrity-signal alert bridge.
//!
//! procmond cannot emit alerts (it has no `AlertManager` and no network by
//! design), so the per-process integrity flags it sets on the wire
//! (`ssdeep_degraded`, `on_disk_mismatch`) are turned into alerts here in the
//! orchestrator, independently of the SQL rule engine. The flags live on the
//! protobuf `ProcessRecord`; the native model does not carry them, so this
//! bridge reads the proto records before they are converted to the native model.

use daemoneye_lib::models::process::ProcessRecord as NativeProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity};
use daemoneye_lib::proto::ProcessRecord as ProtoProcessRecord;

/// Synthetic detection-rule id for the degraded-integrity-coverage alert
/// (ssdeep failed while the SHA-256 identity hash succeeded).
pub const RULE_DEGRADED_COVERAGE: &str = "integrity.coverage.degraded";

/// Synthetic detection-rule id for the on-disk-vs-running mismatch alert.
pub const RULE_DISK_MISMATCH: &str = "integrity.disk_mismatch";

/// Build integrity alerts from the per-process flags on proto process records.
///
/// A single record may produce both alerts (they carry distinct deduplication
/// keys); clean records produce none. The `AlertManager` deduplicates repeated
/// signals within its window, so re-observing the same condition across scans
/// does not flood downstream sinks.
#[must_use]
pub fn detect_integrity_alerts(records: &[ProtoProcessRecord]) -> Vec<Alert> {
    let mut alerts = Vec::new();
    for record in records {
        if record.ssdeep_degraded {
            let description = format!(
                "ssdeep fuzzy hash failed while the SHA-256 identity hash succeeded for \
                 process {} (pid {}); binary-change detection is degraded for this process",
                record.name, record.pid
            );
            alerts.push(build_alert(
                record,
                AlertSeverity::Medium,
                RULE_DEGRADED_COVERAGE,
                "Degraded integrity coverage",
                description,
            ));
        }
        if record.on_disk_mismatch {
            let description = format!(
                "running image of process {} (pid {}) differs from its on-disk executable \
                 (the backing file was deleted or replaced while the process runs)",
                record.name, record.pid
            );
            alerts.push(build_alert(
                record,
                AlertSeverity::High,
                RULE_DISK_MISMATCH,
                "On-disk executable mismatch",
                description,
            ));
        }
    }
    alerts
}

fn build_alert(
    record: &ProtoProcessRecord,
    severity: AlertSeverity,
    rule_id: &str,
    title: &str,
    description: String,
) -> Alert {
    let native: NativeProcessRecord = record.clone().into();
    Alert::new(severity, title, description, rule_id, native)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;

    fn record(name: &str, pid: u32, degraded: bool, mismatch: bool) -> ProtoProcessRecord {
        ProtoProcessRecord {
            pid,
            name: name.to_owned(),
            ssdeep_degraded: degraded,
            on_disk_mismatch: mismatch,
            ..Default::default()
        }
    }

    #[test]
    fn degraded_record_yields_one_medium_alert() {
        let alerts = detect_integrity_alerts(&[record("p", 7, true, false)]);
        assert_eq!(alerts.len(), 1);
        let alert = alerts.first().expect("one alert");
        assert_eq!(alert.severity, AlertSeverity::Medium);
        assert_eq!(alert.detection_rule_id, RULE_DEGRADED_COVERAGE);
    }

    #[test]
    fn mismatch_record_yields_one_high_alert() {
        let alerts = detect_integrity_alerts(&[record("p", 7, false, true)]);
        assert_eq!(alerts.len(), 1);
        let alert = alerts.first().expect("one alert");
        assert_eq!(alert.severity, AlertSeverity::High);
        assert_eq!(alert.detection_rule_id, RULE_DISK_MISMATCH);
    }

    #[test]
    fn clean_record_yields_no_alerts() {
        let alerts = detect_integrity_alerts(&[record("p", 7, false, false)]);
        assert!(alerts.is_empty());
    }

    #[test]
    fn degraded_and_mismatch_yields_two_distinct_alerts() {
        let alerts = detect_integrity_alerts(&[record("p", 7, true, true)]);
        assert_eq!(alerts.len(), 2);
        // Distinct deduplication keys so both reach the sinks.
        let keys: std::collections::HashSet<&str> = alerts
            .iter()
            .map(|alert| alert.deduplication_key.as_str())
            .collect();
        assert_eq!(keys.len(), 2);
    }
}
