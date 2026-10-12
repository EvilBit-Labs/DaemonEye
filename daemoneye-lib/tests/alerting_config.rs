//! Typed sink configuration and credential resolution (U4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use daemoneye_lib::alerting::config::{CredentialResolver, SinkSpec};
use daemoneye_lib::config::{AlertSinkConfig, AlertingConfig, Config, ConfigLoader};
use figment::Jail;
use serde_json::{Value, json};

const SENTINEL: &str = "s3cr3t-sentinel";

fn sink(sink_type: &str, enabled: bool, config: Value) -> AlertSinkConfig {
    AlertSinkConfig {
        sink_type: sink_type.to_owned(),
        config,
        enabled,
    }
}

fn with_sinks(sinks: Vec<AlertSinkConfig>) -> Config {
    let mut config = Config::default();
    config.alerting.sinks = sinks;
    config
}

/// Base object overlaid with `extra`.
fn merged(base: &Value, extra: &Value) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    out.extend(extra.as_object().cloned().unwrap_or_default());
    Value::Object(out)
}

fn webhook(extra: &Value) -> Config {
    let base = json!({"name": "hook", "url": "https://h.example/x"});
    with_sinks(vec![sink("webhook", true, merged(&base, extra))])
}

fn syslog(extra: &Value) -> Config {
    let base = json!({"name": "sys", "transport": "udp", "address": "127.0.0.1:514"});
    with_sinks(vec![sink("syslog", true, merged(&base, extra))])
}

fn failure(config: &Config) -> String {
    config
        .validate()
        .expect_err("configuration must be rejected")
        .to_string()
}

#[test]
fn every_sink_type_loads_through_the_loader_and_validates() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "DAEMONEYE_AGENT_ALERTING__SINKS",
            concat!(
                "[",
                r#"{sink_type="webhook", enabled=true, config={name="hook", url="https://h.example/x", auth="bearer", token_env="HOOK_TOKEN"}},"#,
                r#"{sink_type="syslog", enabled=true, config={name="sys", transport="udp", address="127.0.0.1:514"}},"#,
                r#"{sink_type="email", enabled=true, config={name="mail", host="smtp.example", from="a@example", to=["b@example"], username_env="SMTP_USER", password_env="SMTP_PASS"}},"#,
                r#"{sink_type="stdout", enabled=true, config={name="out"}},"#,
                r#"{sink_type="file", enabled=true, config={name="log", path="alerts.jsonl"}}"#,
                "]"
            ),
        );
        let config = ConfigLoader::new("daemoneye-agent")
            .load()
            .expect("typed sinks must load and validate");
        let specs = config.alerting.enabled_specs().expect("specs parse");
        assert_eq!(specs.len(), 5, "all five sinks enabled");
        assert!(matches!(specs.first(), Some(SinkSpec::Webhook(_))));
        assert!(matches!(specs.get(2), Some(SinkSpec::Email(_))));
        assert_eq!(
            specs.iter().filter(|spec| spec.is_network()).count(),
            3,
            "webhook, syslog and email leave the host"
        );
        Ok(())
    });
}

#[test]
fn a_file_written_before_the_delivery_section_still_loads() {
    let json = r#"{"sinks":[],"dedup_window_seconds":1,"max_alerts_per_minute":null,"recent_threshold_seconds":1}"#;
    let parsed: AlertingConfig = serde_json::from_str(json).expect("delivery is optional");
    assert_eq!(parsed.delivery, AlertingConfig::default().delivery);
}

#[test]
fn unknown_field_is_rejected_naming_the_field() {
    let msg = failure(&webhook(&json!({"tokn_env": "X"})));
    assert!(msg.contains("tokn_env"), "{msg}");
    assert!(msg.contains("hook"), "{msg}");
}

#[test]
fn delivery_range_is_enforced_naming_field_and_range() {
    let mut config = Config::default();
    config.alerting.delivery.max_backoff_secs = 61;
    let msg = failure(&config);
    assert!(msg.contains("alerting.delivery.max_backoff_secs"), "{msg}");
    assert!(msg.contains('1') && msg.contains("60"), "{msg}");

    config.alerting.delivery.max_backoff_secs = 60;
    config.alerting.delivery.max_open_rows_per_sink = 0;
    assert!(failure(&config).contains("max_open_rows_per_sink"));
}

#[test]
fn duplicate_names_and_separator_in_names_are_rejected() {
    let stdout = |name: &str| sink("stdout", true, json!({"name": name}));
    let msg = failure(&with_sinks(vec![stdout("ops"), stdout("ops")]));
    assert!(msg.contains("ops") && msg.contains("duplicate"), "{msg}");

    let separator = failure(&with_sinks(vec![stdout("a\u{1f}b")]));
    assert!(separator.contains("alerting.sinks"), "{separator}");
}

#[test]
fn malformed_webhook_urls_are_rejected() {
    for url in [
        "https://user:pw@h.example/x",
        "https://h.example/x?token=1",
        "https://h.example/x#f",
        "https:///x",
        "ftp://h.example/x",
    ] {
        let msg = failure(&webhook(&json!({"url": url})));
        assert!(msg.contains("hook"), "{url}: {msg}");
        assert!(!msg.contains("pw@"), "user-info must not be echoed: {msg}");
    }
}

#[test]
fn plain_http_needs_opt_in_and_an_unauthenticated_sink() {
    let http = json!({"url": "http://h.example/"});
    assert!(failure(&webhook(&http)).contains("allow_http"));

    let with_auth = merged(
        &http,
        &json!({"allow_http": true, "auth": "bearer", "token_env": "X"}),
    );
    assert!(failure(&webhook(&with_auth)).contains("allow_http"));

    let opted_in = merged(&http, &json!({"allow_http": true, "auth": "none"}));
    webhook(&opted_in)
        .validate()
        .expect("explicit opt-in passes");
}

#[test]
fn header_auth_requires_a_header_name() {
    let msg = failure(&webhook(&json!({"auth": "header", "token_env": "X"})));
    assert!(msg.contains("header_name"), "{msg}");
}

#[test]
fn webhook_timeout_range_is_enforced() {
    let msg = failure(&webhook(&json!({"timeout_secs": 0})));
    assert!(msg.contains("timeout_secs") && msg.contains("120"), "{msg}");
}

#[test]
fn off_host_plaintext_syslog_needs_opt_in() {
    let off_host = json!({"address": "10.0.0.5:514"});
    assert!(failure(&syslog(&off_host)).contains("allow_plaintext"));
    syslog(&merged(&off_host, &json!({"allow_plaintext": true})))
        .validate()
        .expect("opt-in passes");
    syslog(&json!({}))
        .validate()
        .expect("loopback needs no opt-in");
    let tcp_off = json!({"transport": "tcp", "address": "example.org:514"});
    assert!(failure(&syslog(&tcp_off)).contains("allow_plaintext"));
}

#[test]
fn syslog_message_size_range_is_enforced() {
    let msg = failure(&syslog(&json!({"max_message_bytes": 100})));
    assert!(msg.contains("max_message_bytes"), "{msg}");
}

#[test]
fn ca_file_must_exist_parse_and_avoid_traversal() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "empty.pem",
            "this file is long enough to hold a marker but has none",
        )?;
        jail.create_file(
            "ok.pem",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
        )?;
        let dir = jail.directory().to_string_lossy().into_owned();
        let at = |name: &str| json!({"ca_file": format!("{dir}/{name}")});

        assert!(failure(&webhook(&at("missing.pem"))).contains("ca_file"));
        assert!(failure(&webhook(&at("empty.pem"))).contains("certificate"));
        assert!(failure(&webhook(&json!({"ca_file": "../ok.pem"}))).contains(".."));
        webhook(&at("ok.pem")).validate().expect("valid PEM passes");

        let tls = merged(
            &at("empty.pem"),
            &json!({"transport": "tls", "address": "h:6514"}),
        );
        assert!(failure(&syslog(&tls)).contains("ca_file"));
        Ok(())
    });
}

#[test]
fn email_needs_a_recipient() {
    let email = json!({"name": "mail", "host": "smtp.example", "from": "a@example", "to": []});
    let msg = failure(&with_sinks(vec![sink("email", true, email)]));
    assert!(msg.contains("to"), "{msg}");
}

#[cfg(windows)]
#[test]
fn unix_transport_is_rejected_on_windows() {
    let unix = json!({"transport": "unix", "address": "C:/s.sock"});
    assert!(failure(&syslog(&unix)).contains("Windows"));
}

#[cfg(unix)]
#[test]
fn overlong_unix_socket_path_is_rejected() {
    let long = format!("/tmp/{}", "a".repeat(115));
    let unix = json!({"transport": "unix", "address": long});
    assert!(failure(&syslog(&unix)).contains("107"));
    syslog(&json!({"transport": "unix", "address": "/tmp/s.sock"}))
        .validate()
        .expect("short path passes");
}

#[test]
fn disabled_sinks_only_need_a_known_type() {
    let junk = sink("webhook", false, json!({"nonsense": 1}));
    with_sinks(vec![junk])
        .validate()
        .expect("disabled sink is not parsed");
    let unknown = sink("carrier-pigeon", false, json!({}));
    assert!(failure(&with_sinks(vec![unknown])).contains("carrier-pigeon"));
}

fn only_spec(config: &Config) -> SinkSpec {
    let mut specs = config.alerting.enabled_specs().expect("specs parse");
    assert_eq!(specs.len(), 1, "exactly one enabled sink");
    specs.remove(0)
}

fn bearer_config(enabled: bool) -> Config {
    let cfg =
        json!({"name": "hook", "url": "https://h.example/x", "auth": "bearer", "token_env": "X"});
    with_sinks(vec![sink("webhook", enabled, cfg)])
}

#[test]
fn enabled_sink_with_missing_or_empty_variable_fails_naming_sink_and_variable() {
    let config = bearer_config(true);
    let spec = only_spec(&config);

    for value in [None, Some(String::new())] {
        let resolver = CredentialResolver::new(move |_name: &str| value.clone());
        let msg = resolver
            .resolve(&spec)
            .expect_err("must fail closed")
            .to_string();
        assert!(msg.contains("hook") && msg.contains('X'), "{msg}");
    }
}

#[test]
fn bearer_without_a_variable_name_is_refused() {
    let cfg = json!({"name": "hook", "url": "https://h.example/x", "auth": "bearer"});
    let config = with_sinks(vec![sink("webhook", true, cfg)]);
    let spec = only_spec(&config);
    let resolver = CredentialResolver::new(|_name: &str| Some("v".to_owned()));
    let msg = resolver.resolve(&spec).expect_err("refused").to_string();
    assert!(msg.contains("token_env"), "{msg}");
}

#[test]
fn disabled_sinks_are_never_resolved() {
    let config = bearer_config(false);
    assert!(config.alerting.enabled_specs().unwrap().is_empty());
}

#[test]
fn resolved_secrets_never_render() {
    let config = bearer_config(true);
    let spec = only_spec(&config);
    let resolver = CredentialResolver::new(|name: &str| (name == "X").then(|| SENTINEL.to_owned()));
    let resolved = resolver.resolve(&spec).expect("resolves");

    let debug = format!("{resolved:?}");
    assert!(!debug.contains(SENTINEL), "Debug leaked the secret");
    assert!(debug.contains("[REDACTED]"));
    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains(SENTINEL), "config JSON leaked the secret");
    assert!(json.contains("h.example"), "positive control");
}

#[test]
fn basic_auth_resolves_both_halves() {
    let cfg = json!({"name": "hook", "url": "https://h.example/x", "auth": "basic",
        "username_env": "U", "password_env": "P"});
    let config = with_sinks(vec![sink("webhook", true, cfg)]);
    let spec = only_spec(&config);
    let only_user = CredentialResolver::new(|name: &str| (name == "U").then(|| "u".to_owned()));
    let msg = only_user
        .resolve(&spec)
        .expect_err("password missing")
        .to_string();
    assert!(msg.contains('P'), "{msg}");
    let both = CredentialResolver::new(|_name: &str| Some("v".to_owned()));
    assert!(format!("{:?}", both.resolve(&spec).unwrap()).contains("[REDACTED]"));
}
