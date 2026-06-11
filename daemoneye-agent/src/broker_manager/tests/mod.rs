//! Unit tests for the broker manager.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::str_to_string,
    clippy::semicolon_outside_block,
    clippy::semicolon_inside_block,
    clippy::semicolon_if_nothing_returned,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    clippy::panic
)]

mod lifecycle;
mod state_machine;

use daemoneye_eventbus::rpc::RegistrationRequest;

fn sample_registration_request() -> RegistrationRequest {
    RegistrationRequest {
        collector_id: "test-collector".to_owned(),
        collector_type: "test-collector".to_owned(),
        hostname: "localhost".to_owned(),
        version: Some("1.0.0".to_owned()),
        pid: Some(1234),
        capabilities: vec!["process".to_owned()],
        attributes: std::collections::HashMap::new(),
        heartbeat_interval_ms: Some(5_000),
    }
}
