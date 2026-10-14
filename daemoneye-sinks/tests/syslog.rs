//! Syslog sink against loopback receivers.
#![allow(
    clippy::partial_pub_fields,
    clippy::pattern_type_mismatch,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::clone_on_ref_ptr,
    clippy::integer_division,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::missing_const_for_fn
)]

mod support;

use daemoneye_lib::alerting::config::{SyslogSpec, SyslogTransport};
use daemoneye_lib::alerting::{
    AlertSink, AlertingError, DeliveryError, DeliveryErrorClass, ResolvedSink, SinkBuilder,
    SinkSpec,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness};
use daemoneye_sinks::{NetworkSinkBuilder, SyslogSink};
#[cfg(unix)]
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};
use support::mock_http::TestPki;
use support::mock_syslog::MockSyslog;

const CMD_SENTINEL: &str = "CMDLINE-sentinel";
const ALERT_ID: &str = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";

fn delivery() -> DeliveryConfig {
    DeliveryConfig {
        attempt_timeout_secs: 2,
        ..DeliveryConfig::default()
    }
}

fn spec(transport: SyslogTransport, address: &str) -> SyslogSpec {
    SyslogSpec {
        name: "syslog-a".to_owned(),
        transport,
        address: address.to_owned(),
        facility: "daemon".to_owned(),
        app_name: "daemoneye".to_owned(),
        ca_file: None,
        max_message_bytes: 2048,
        allow_plaintext: false,
        include_command_line: false,
        include_user: false,
    }
}

fn resolved(spec: SyslogSpec) -> ResolvedSink {
    ResolvedSink {
        spec: SinkSpec::Syslog(spec),
        token: None,
        username: None,
        password: None,
    }
}

fn sink(spec: SyslogSpec) -> SyslogSink {
    SyslogSink::new(&resolved(spec), &delivery()).unwrap()
}

fn alert_with(title: &str, description: &str) -> Alert {
    let mut record = ProcessRecord::new(10, "bash".to_owned());
    record.command_line = Some(CMD_SENTINEL.to_owned());
    let mut alert = Alert::new(
        AlertSeverity::High,
        title,
        description,
        "rule-1",
        record,
        Completeness::complete(),
    );
    alert.id = ALERT_ID.parse().unwrap();
    alert.timestamp = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    alert
        .context
        .data
        .insert("host".to_owned(), "web01".to_owned());
    alert
}

fn alert() -> Alert {
    alert_with("Apache spawned bash", "httpd -> bash")
}

#[derive(Debug)]
struct Parsed {
    pri: u8,
    version: u8,
    timestamp: String,
    host: String,
    app: String,
    procid: String,
    msgid: String,
    sd: Vec<(String, String)>,
    msg: String,
}

/// Parse one RFC 5424 record with exactly one structured-data element.
fn parse(line: &str) -> Parsed {
    assert!(!line.contains(['\r', '\n']), "record spans lines: {line:?}");
    let mut head = line.splitn(7, ' ');
    let pri_ver = head.next().unwrap();
    let (pri, version) = pri_ver
        .strip_prefix('<')
        .and_then(|rest| rest.split_once('>'))
        .unwrap();
    let timestamp = head.next().unwrap().to_owned();
    let host = head.next().unwrap().to_owned();
    let app = head.next().unwrap().to_owned();
    let procid = head.next().unwrap().to_owned();
    let msgid = head.next().unwrap().to_owned();
    let rest = head.next().unwrap();
    let body = rest.strip_prefix("[daemoneye@32473").expect("sd id");
    let mut sd = Vec::new();
    let mut chars = body.chars();
    let mut closed = false;
    while let Some(c) = chars.next() {
        match c {
            ' ' => {
                let mut key = String::new();
                for k in chars.by_ref() {
                    if k == '=' {
                        break;
                    }
                    key.push(k);
                }
                assert_eq!(chars.next(), Some('"'), "value must open with a quote");
                let mut value = String::new();
                loop {
                    match chars.next().expect("unterminated value") {
                        '\\' => value.push(chars.next().unwrap()),
                        '"' => break,
                        other => value.push(other),
                    }
                }
                sd.push((key, value));
            }
            ']' => {
                closed = true;
                break;
            }
            other => panic!("unexpected {other:?} in sd"),
        }
    }
    assert!(closed, "sd not closed in {line:?}");
    let msg = chars.as_str().strip_prefix(' ').unwrap_or("").to_owned();
    Parsed {
        pri: pri.parse().unwrap(),
        version: version.parse().unwrap(),
        timestamp,
        host,
        app,
        procid,
        msgid,
        sd,
        msg,
    }
}

fn sd<'a>(parsed: &'a Parsed, key: &str) -> Option<&'a str> {
    parsed
        .sd
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

async fn send_udp(spec_fn: impl FnOnce(&mut SyslogSpec), alert: &Alert) -> String {
    let server = MockSyslog::udp().await;
    let mut spec = spec(SyslogTransport::Udp, &server.address);
    spec_fn(&mut spec);
    sink(spec).send(alert).await.unwrap();
    server.wait_for(1).await.remove(0)
}

fn delivery_error(result: Result<impl std::fmt::Debug, AlertingError>) -> DeliveryError {
    match result {
        Err(AlertingError::Delivery(error)) => error,
        other => panic!("expected a delivery error, got {other:?}"),
    }
}

#[tokio::test]
async fn message_matches_snapshot_and_parses_back() {
    let alert = alert();
    let line = send_udp(|_| {}, &alert).await;
    insta::assert_snapshot!(line);

    let parsed = parse(&line);
    assert_eq!(parsed.pri, 27, "daemon(3)*8 + high(3)");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.timestamp, "2023-11-14T22:13:20.123Z");
    assert_eq!(parsed.host, "web01");
    assert_eq!(parsed.app, "daemoneye");
    assert_eq!(parsed.procid, "-");
    assert_eq!(parsed.msgid, ALERT_ID.replace('-', ""));
    assert_eq!(sd(&parsed, "dedup"), Some(alert.deduplication_key.as_str()));
    assert_eq!(sd(&parsed, "rule"), Some("rule-1"));
    assert_eq!(sd(&parsed, "pid"), Some("10"));
    assert!(parsed.msg.starts_with("[high]"), "{}", parsed.msg);
    assert_eq!(sd(&parsed, "truncated"), None);
}

#[tokio::test]
async fn severity_and_facility_map_to_priority() {
    let mut critical = alert();
    critical.severity = AlertSeverity::Critical;
    let line = send_udp(|s| "local7".clone_into(&mut s.facility), &critical).await;
    assert_eq!(parse(&line).pri, 23 * 8 + 2);

    let line = send_udp(|s| "no-such-facility".clone_into(&mut s.facility), &alert()).await;
    assert_eq!(
        parse(&line).pri,
        27,
        "unknown facility falls back to daemon"
    );
}

#[tokio::test]
async fn hostile_fields_cannot_forge_a_record_or_break_the_sd_block() {
    let hostile = alert_with("t\r\n<13>1 forged\"] [evil x=\"1\"]\x07", "d\"] [evil\r\n");
    let line = send_udp(|_| {}, &hostile).await;
    let parsed = parse(&line);
    assert_eq!(parsed.msgid, ALERT_ID.replace('-', ""));
    assert_eq!(parsed.sd.len(), 4, "{:?}", parsed.sd);
    assert_eq!(sd(&parsed, "rule"), Some("rule-1"));
    assert_eq!(
        sd(&parsed, "dedup"),
        Some("high:rule-1:t<13>1 forged\"] [evil x=\"1\"]")
    );
    assert!(!line.contains('\x07'));

    let clean = send_udp(|_| {}, &alert()).await;
    assert_eq!(
        parse(&clean).msg,
        "[high] rule-1 - Apache spawned bash: httpd -> bash"
    );
}

#[tokio::test]
async fn command_line_travels_only_when_the_sink_opts_in() {
    let default = send_udp(|_| {}, &alert()).await;
    assert!(!default.contains(CMD_SENTINEL), "command line leaked");
    let opted_in = send_udp(|s| s.include_command_line = true, &alert()).await;
    assert!(opted_in.contains(CMD_SENTINEL), "{opted_in}");
}

#[tokio::test]
async fn oversize_message_is_truncated_on_a_char_boundary_and_flagged() {
    let long = alert_with("t", &"\u{e9}".repeat(2500));
    for cap in 1024..1028 {
        let line = send_udp(|s| s.max_message_bytes = cap, &long).await;
        assert!(line.len() <= cap, "{} bytes over cap {cap}", line.len());
        assert!(
            line.len() >= cap - 1,
            "cap should be used, got {}",
            line.len()
        );
        let parsed = parse(&line);
        assert_eq!(sd(&parsed, "truncated"), Some("1"));
        assert!(parsed.msg.starts_with("[high]"));
    }

    let short = send_udp(|s| s.max_message_bytes = 1024, &alert()).await;
    assert_eq!(sd(&parse(&short), "truncated"), None);
}

#[tokio::test]
async fn worst_case_header_and_sd_still_fit_the_minimum_cap() {
    const CAP: usize = 480;
    let hostile = "\"\\]".repeat(85);
    let mut alert = alert_with("t", &"d".repeat(2000));
    alert.detection_rule_id = hostile.clone();
    alert.deduplication_key = hostile.clone();
    alert.context.data.insert("host".to_owned(), hostile);
    let line = send_udp(
        |s| {
            s.max_message_bytes = CAP;
            s.app_name = "a".repeat(100);
        },
        &alert,
    )
    .await;
    assert!(line.len() <= CAP, "{} bytes over cap {CAP}", line.len());
    let parsed = parse(&line);
    assert_eq!(parsed.app.len(), 48);
    assert_eq!(parsed.msgid, ALERT_ID.replace('-', ""));
    assert_eq!(sd(&parsed, "truncated"), Some("1"));
    assert_eq!(sd(&parsed, "pid"), Some("10"));
}

#[tokio::test]
async fn udp_delivers_one_datagram_and_is_fire_and_forget() {
    let server = MockSyslog::udp().await;
    let sink = sink(spec(SyslogTransport::Udp, &server.address));
    assert!(sink.is_fire_and_forget());
    assert!(!sink.is_local());
    assert_eq!(sink.name(), "syslog-a");
    sink.send(&alert()).await.unwrap();
    sink.health_check().await.unwrap();
    assert_eq!(server.wait_for(1).await.len(), 1);
}

#[tokio::test]
async fn tcp_frames_with_octet_counting_and_reconnects() {
    let server = MockSyslog::tcp().await;
    let sink = sink(spec(SyslogTransport::Tcp, &server.address));
    assert!(!sink.is_fire_and_forget());
    sink.health_check().await.unwrap();
    sink.send(&alert()).await.unwrap();
    let first = server.wait_for(1).await.remove(0);
    assert_eq!(parse(&first).msgid, ALERT_ID.replace('-', ""));

    server.drop_connection().await;
    sink.send(&alert()).await.unwrap();
    assert_eq!(server.wait_for(2).await.len(), 2, "second send was lost");
}

#[tokio::test]
async fn tcp_frame_length_counts_bytes() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let sink = sink(spec(SyslogTransport::Tcp, &address));
    let accept = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some((len, rest)) = text.split_once(' ')
                && rest.len() >= len.parse::<usize>().unwrap()
            {
                return text;
            }
        }
    });
    sink.send(&alert_with("\u{e9}", "\u{e9}")).await.unwrap();
    let raw = accept.await.unwrap();
    let (len, msg) = raw.split_once(' ').unwrap();
    assert_eq!(len.parse::<usize>().unwrap(), msg.len());
    assert!(msg.len() > msg.chars().count(), "multi-byte title expected");
}

#[tokio::test]
async fn tls_verifies_the_server_against_the_configured_ca() {
    let dir = tempfile::tempdir().unwrap();
    let pki = TestPki::generate();
    let server = MockSyslog::tls(&pki).await;
    let mut good = spec(SyslogTransport::Tls, &server.address);
    good.ca_file = Some(pki.write_ca(dir.path()));
    let ok = sink(good);
    ok.health_check().await.unwrap();
    ok.send(&alert()).await.unwrap();
    assert_eq!(server.wait_for(1).await.len(), 1);

    // Leaf for a different name, issued by the trusted CA.
    let wrong_name = TestPki::generate_for(&["other.invalid"]);
    let server = MockSyslog::tls(&wrong_name).await;
    let mut spec_wrong = spec(SyslogTransport::Tls, &server.address);
    spec_wrong.ca_file = Some(wrong_name.write_ca(dir.path()));
    let error = delivery_error(sink(spec_wrong).send(&alert()).await);
    assert!(
        matches!(
            error,
            DeliveryError::Misconfigured {
                class: DeliveryErrorClass::TlsVerification
            }
        ),
        "{error:?}"
    );

    // Valid leaf, but its CA is not trusted: no ca_file means platform roots only.
    let unlisted = TestPki::generate();
    let server = MockSyslog::tls(&unlisted).await;
    let error = delivery_error(
        sink(spec(SyslogTransport::Tls, &server.address))
            .send(&alert())
            .await,
    );
    assert!(
        matches!(
            error,
            DeliveryError::Misconfigured {
                class: DeliveryErrorClass::TlsVerification
            }
        ),
        "{error:?}"
    );
    assert!(server.messages().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn unix_datagram_delivers() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("log.sock");
    let server = MockSyslog::unix(&path);
    let sink = sink(spec(SyslogTransport::Unix, &server.address));
    assert!(sink.is_fire_and_forget());
    sink.health_check().await.unwrap();
    sink.send(&alert()).await.unwrap();
    assert_eq!(parse(&server.wait_for(1).await[0]).host, "web01");
}

#[tokio::test]
async fn builder_builds_a_syslog_sink() {
    let server = MockSyslog::udp().await;
    let built = NetworkSinkBuilder
        .build(
            &resolved(spec(SyslogTransport::Udp, &server.address)),
            &delivery(),
        )
        .unwrap();
    assert_eq!(built.name(), "syslog-a");
    built.send(&alert()).await.unwrap();
    assert_eq!(server.wait_for(1).await.len(), 1);
}

#[tokio::test]
async fn debug_output_names_the_sink_only() {
    let server = MockSyslog::udp().await;
    let mut hostile = spec(SyslogTransport::Udp, &server.address);
    "secret-host.example".clone_into(&mut hostile.app_name);
    let text = format!("{:?}", sink(hostile));
    assert!(text.contains("syslog-a"));
    assert!(!text.contains(&server.address), "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(!text.contains("secret-host"), "{text}");
}
