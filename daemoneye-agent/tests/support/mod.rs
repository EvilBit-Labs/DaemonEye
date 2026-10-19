//! Shared fixtures for the alert-delivery integration and chaos tests.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::field_reassign_with_default
)]

pub mod mock_http;

use daemoneye_agent::alert_sinks::build_sinks;
use daemoneye_lib::alerting::{AlertManager, CredentialResolver, EnrichContext, NoopPublisher};
use daemoneye_lib::config::{AlertSinkConfig, AlertingConfig};
use daemoneye_lib::models::{
    Alert, AlertSeverity, Completeness, DeliveryRecord, ProcessRecord, Uuid,
};
use daemoneye_lib::storage::EventStore;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const WEBHOOK: &str = "hook";
pub const FILE: &str = "file";
pub const SYSLOG: &str = "syslog";
pub const TOKEN_VAR: &str = "HOOK_TOKEN";

pub fn ctx() -> EnrichContext {
    EnrichContext {
        hostname: "test-host".to_owned(),
        agent_version: "9.9.9".to_owned(),
    }
}

/// A distinct alert: the dedup key embeds the title.
pub fn alert(title: &str) -> Alert {
    Alert::new(
        AlertSeverity::High,
        title,
        "d",
        "rule-1",
        ProcessRecord::new(1, "p".to_owned()),
        Completeness::complete(),
    )
}

pub fn webhook_sink(url: &str) -> AlertSinkConfig {
    AlertSinkConfig {
        sink_type: "webhook".to_owned(),
        config: json!({
            "name": WEBHOOK, "url": url, "allow_http": true,
            "auth": "bearer", "token_env": TOKEN_VAR, "timeout_secs": 5,
        }),
        enabled: true,
    }
}

/// A TCP syslog sink; loopback needs no `allow_plaintext`.
pub fn syslog_sink(address: &str) -> AlertSinkConfig {
    AlertSinkConfig {
        sink_type: "syslog".to_owned(),
        config: json!({ "name": SYSLOG, "transport": "tcp", "address": address }),
        enabled: true,
    }
}

/// A loopback `host:port` nothing listens on: bind, read the port, drop the listener.
pub fn closed_tcp_address() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

pub fn file_sink(path: &Path) -> AlertSinkConfig {
    AlertSinkConfig {
        sink_type: "file".to_owned(),
        config: json!({ "name": FILE, "path": path, "format": "json" }),
        enabled: true,
    }
}

/// Fast retries and a hair-trigger breaker so a test does not wait on production defaults.
pub fn alerting(sinks: Vec<AlertSinkConfig>) -> AlertingConfig {
    let mut config = AlertingConfig::default();
    config.sinks = sinks;
    config.delivery.initial_backoff_ms = 100;
    config.delivery.max_backoff_secs = 1;
    config.delivery.attempt_timeout_secs = 1;
    config.delivery.breaker_failure_threshold = 2;
    config.delivery.breaker_cooldown_secs = 30;
    config
}

pub fn resolver() -> CredentialResolver {
    CredentialResolver::new(|name| (name == TOKEN_VAR).then(|| "test-token".to_owned()))
}

pub fn temp_store() -> (tempfile::TempDir, Arc<EventStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("events.redb")).unwrap());
    (dir, store)
}

/// Sinks built the way the agent builds them, then a started manager over `store`.
pub async fn start_manager(store: &Arc<EventStore>, config: &AlertingConfig) -> AlertManager {
    let sinks = build_sinks(config, &resolver()).unwrap();
    AlertManager::start(Arc::clone(store), sinks, config, Arc::new(NoopPublisher))
        .await
        .unwrap()
}

pub fn row(store: &EventStore, sink: &str, id: Uuid) -> DeliveryRecord {
    store.get_delivery(sink, id).unwrap().unwrap()
}

/// Poll `condition` every 10 ms until it holds or `deadline` passes; returns whether it held.
pub async fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    condition()
}

/// Lines of `path` that mention `needle`.
pub fn lines_with(path: &Path, needle: &str) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| {
        text.lines().filter(|line| line.contains(needle)).count()
    })
}
