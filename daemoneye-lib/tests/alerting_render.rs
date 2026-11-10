//! The external projection is an allowlist: sensitive fields never cross unless a flag asks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags, sanitise_control};
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness, ProcessId};
use serde_json::Value;

const SENTINEL: &str = "SENTINEL-s3cret";

fn alert() -> Alert {
    let mut record = ProcessRecord::new(42, "bash".to_owned());
    record.ppid = Some(ProcessId::new(7));
    record.command_line = Some(format!("curl -H token={SENTINEL}"));
    record.user_id = Some(1000);
    Alert::new(
        AlertSeverity::High,
        "title",
        "desc",
        "rule-1",
        record,
        Completeness::complete(),
    )
    .with_context_data("host", "host-a")
    .with_context_data("agent_version", "1.2.3")
    .with_context_data("ancestry", "init>sshd")
    .with_context_data("secret_sentinel", SENTINEL)
    .with_tag("enriched")
}

fn json(flags: IncludeFlags) -> Value {
    serde_json::to_value(ExternalAlert::project(&alert(), flags)).unwrap()
}

#[test]
fn default_projection_omits_sensitive_and_unlisted_fields() {
    let out = json(IncludeFlags::default());
    let object = out.as_object().unwrap();
    for absent in ["command_line", "user", "secret_sentinel"] {
        assert!(!object.contains_key(absent), "{absent} leaked");
    }
    assert!(!out.to_string().contains(SENTINEL), "sentinel leaked");
    assert_eq!(out["rule_id"], "rule-1");
    assert_eq!(out["pid"], 42);
    assert_eq!(out["ppid"], 7);
    assert_eq!(out["host"], "host-a");
    assert_eq!(out["agent_version"], "1.2.3");
    assert_eq!(out["ancestry"], "init>sshd");
    assert_eq!(out["tags"][0], "enriched");
    assert_eq!(out["completeness"]["status"], "Complete");
}

#[test]
fn flags_add_command_line_and_user() {
    let out = json(IncludeFlags {
        command_line: true,
        user: true,
    });
    assert!(out["command_line"].as_str().unwrap().contains(SENTINEL));
    assert_eq!(out["user"], "1000");
    assert!(out.get("secret_sentinel").is_none());
}

#[test]
fn sanitise_control_strips_control_characters_only() {
    assert_eq!(sanitise_control("a\r\nb\u{7}c\td"), "abcd");
    assert_eq!(sanitise_control("clean text-1"), "clean text-1");
}
