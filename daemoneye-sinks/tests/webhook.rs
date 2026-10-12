//! Webhook sink against a scripted loopback server.
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
    clippy::too_many_lines
)]

mod support;

use daemoneye_lib::alerting::config::{WebhookAuth, WebhookSpec};
use daemoneye_lib::alerting::{
    AlertSink, AlertSinkFactory, AlertingError, DeliveryError, DeliveryErrorClass, ResolvedSink,
    SinkSpec,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness};
use daemoneye_sinks::{NetworkSinkBuilder, WebhookSink};
use secrecy::SecretString;
use std::time::{Duration, Instant};
use support::mock_http::{MockHttp, Response, TestPki};

const TOKEN: &str = "TOKEN-sentinel-9f31";
const CMD_SENTINEL: &str = "CMDLINE-sentinel";

fn delivery() -> DeliveryConfig {
    DeliveryConfig {
        attempt_timeout_secs: 1,
        ..DeliveryConfig::default()
    }
}

fn spec(url: String) -> WebhookSpec {
    WebhookSpec {
        name: "hook".to_owned(),
        url,
        auth: WebhookAuth::Bearer,
        token_env: Some("T".to_owned()),
        username_env: None,
        password_env: None,
        header_name: None,
        timeout_secs: 5,
        allow_http: true,
        ca_file: None,
        include_command_line: false,
        include_user: false,
    }
}

fn resolved(spec: WebhookSpec) -> ResolvedSink {
    ResolvedSink {
        spec: SinkSpec::Webhook(spec),
        token: Some(SecretString::from(TOKEN.to_owned())),
        username: Some(SecretString::from("u".to_owned())),
        password: Some(SecretString::from("p".to_owned())),
    }
}

fn sink(spec: WebhookSpec) -> WebhookSink {
    WebhookSink::new(&resolved(spec), &delivery()).unwrap()
}

fn alert() -> Alert {
    let mut record = ProcessRecord::new(10, "bash".to_owned());
    record.command_line = Some(CMD_SENTINEL.to_owned());
    Alert::new(
        AlertSeverity::High,
        "t",
        "d",
        "rule-1",
        record,
        Completeness::complete(),
    )
}

fn delivery_error(result: Result<impl std::fmt::Debug, AlertingError>) -> DeliveryError {
    match result {
        Err(AlertingError::Delivery(error)) => error,
        other => panic!("expected a delivery error, got {other:?}"),
    }
}

#[tokio::test]
async fn delivers_with_auth_and_idempotency_headers() {
    let server = MockHttp::start(vec![Response::Status(200, vec![])]).await;
    let alert = alert();
    let result = sink(spec(server.url("http"))).send(&alert).await.unwrap();
    assert_eq!(result.sink_name, "hook");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/hook");
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(
        request.header("x-daemoneye-alert-id"),
        Some(alert.id.to_string().as_str())
    );
    assert_eq!(
        request.header("x-daemoneye-dedup-key"),
        Some(alert.deduplication_key.as_str())
    );
    assert!(
        request
            .header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    let body = request.body_json();
    assert_eq!(body["id"], alert.id.to_string());
    assert!(body.get("command_line").is_none(), "command line leaked");
}

#[tokio::test]
async fn command_line_travels_only_when_the_sink_opts_in() {
    let server = MockHttp::start(vec![]).await;
    let mut opted_in = spec(server.url("http"));
    opted_in.include_command_line = true;
    sink(opted_in).send(&alert()).await.unwrap();
    let body = server.requests()[0].body_json();
    assert_eq!(body["command_line"], CMD_SENTINEL);
}

#[tokio::test]
async fn http_statuses_classify() {
    let cases = [(500, 'T'), (404, 'P'), (401, 'M')];
    for (status, kind) in cases {
        let server = MockHttp::start(vec![Response::Status(status, vec![])]).await;
        let error = delivery_error(sink(spec(server.url("http"))).send(&alert()).await);
        match (kind, &error) {
            ('T', DeliveryError::Transient { class, .. })
            | ('P', DeliveryError::Permanent { class }) => {
                assert_eq!(*class, DeliveryErrorClass::HttpStatus(status));
            }
            ('M', DeliveryError::Misconfigured { class }) => {
                assert_eq!(*class, DeliveryErrorClass::Auth);
            }
            _ => panic!("status {status} classified as {error:?}"),
        }
    }
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let target = MockHttp::start(vec![]).await;
    let origin = MockHttp::start(vec![Response::Status(
        302,
        vec![("Location".to_owned(), target.url("http"))],
    )])
    .await;
    let error = delivery_error(sink(spec(origin.url("http"))).send(&alert()).await);
    assert!(matches!(
        error,
        DeliveryError::Permanent {
            class: DeliveryErrorClass::HttpStatus(302)
        }
    ));
    assert!(
        target.requests().is_empty(),
        "redirect target was contacted"
    );
}

#[tokio::test]
async fn hang_times_out_as_transient() {
    let server = MockHttp::start(vec![Response::Hang]).await;
    let started = Instant::now();
    let error = delivery_error(sink(spec(server.url("http"))).send(&alert()).await);
    assert!(matches!(
        error,
        DeliveryError::Transient {
            class: DeliveryErrorClass::Timeout,
            ..
        }
    ));
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn reset_connection_is_transient() {
    let server = MockHttp::start(vec![Response::Reset]).await;
    let error = delivery_error(sink(spec(server.url("http"))).send(&alert()).await);
    assert!(
        matches!(error, DeliveryError::Transient { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn response_body_is_never_read() {
    const TEN_MIB: usize = 10 * 1024 * 1024;
    let server = MockHttp::start(vec![Response::LargeBody(200, TEN_MIB)]).await;
    let started = Instant::now();
    sink(spec(server.url("http"))).send(&alert()).await.unwrap();
    // Streaming 10 MiB at 64 KiB per 5 ms takes about 0.8 s; Ok must not wait for it.
    assert!(started.elapsed() < Duration::from_millis(500));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(server.bytes_written() < TEN_MIB / 2);
}

#[tokio::test]
async fn retry_after_delta_seconds_is_kept_and_dates_are_not() {
    let server = MockHttp::start(vec![
        Response::Status(429, vec![("Retry-After".to_owned(), "7".to_owned())]),
        Response::Status(
            429,
            vec![(
                "Retry-After".to_owned(),
                "Wed, 21 Oct 2026 07:28:00 GMT".to_owned(),
            )],
        ),
    ])
    .await;
    let sink = sink(spec(server.url("http")));
    let first = delivery_error(sink.send(&alert()).await);
    assert_eq!(
        first,
        DeliveryError::Transient {
            class: DeliveryErrorClass::HttpStatus(429),
            retry_after: Some(Duration::from_secs(7)),
        }
    );
    let second = delivery_error(sink.send(&alert()).await);
    assert_eq!(
        second,
        DeliveryError::Transient {
            class: DeliveryErrorClass::HttpStatus(429),
            retry_after: None,
        }
    );
}

#[tokio::test]
async fn ca_file_replaces_the_roots_and_is_required_for_a_private_ca() {
    let pki = TestPki::generate();
    let dir = tempfile::tempdir().unwrap();
    let ca = pki.write_ca(dir.path());
    let server = MockHttp::start_tls(vec![], &pki).await;

    let mut trusted = spec(server.url("https"));
    trusted.allow_http = false;
    trusted.ca_file = Some(ca);
    sink(trusted).send(&alert()).await.unwrap();
    assert_eq!(server.requests().len(), 1);

    let mut untrusted = spec(server.url("https"));
    untrusted.allow_http = false;
    let error = delivery_error(sink(untrusted).send(&alert()).await);
    assert_eq!(error, DeliveryError::tls_verification());
    assert_eq!(server.requests().len(), 1, "request sent despite bad cert");
}

#[tokio::test]
async fn basic_and_custom_header_auth() {
    let server = MockHttp::start(vec![]).await;
    let mut basic = spec(server.url("http"));
    basic.auth = WebhookAuth::Basic;
    sink(basic).send(&alert()).await.unwrap();

    let mut custom = spec(server.url("http"));
    custom.auth = WebhookAuth::Header;
    custom.header_name = Some("X-Api-Key".to_owned());
    sink(custom).send(&alert()).await.unwrap();

    let requests = server.requests();
    // base64("u:p")
    assert_eq!(requests[0].header("authorization"), Some("Basic dTpw"));
    assert_eq!(requests[1].header("x-api-key"), Some(TOKEN));
    assert_eq!(requests[1].header("authorization"), None);
}

#[tokio::test]
async fn token_never_appears_in_debug_or_error_text() {
    let ok_server = MockHttp::start(vec![]).await;
    let ok_sink = sink(spec(ok_server.url("http")));
    assert!(!format!("{ok_sink:?}").contains(TOKEN));

    let failing = [
        Response::Status(500, vec![]),
        Response::Status(401, vec![]),
        Response::Hang,
        Response::Reset,
    ];
    for response in failing {
        let server = MockHttp::start(vec![response]).await;
        let sink = sink(spec(server.url("http")));
        let error = sink.send(&alert()).await.unwrap_err();
        let text = format!("{error} {error:?}");
        assert!(!text.contains(TOKEN), "token in error text");
        assert!(!text.contains("127.0.0.1"), "url in error text");
    }
}

#[tokio::test]
async fn health_check_uses_head() {
    let server = MockHttp::start(vec![Response::Status(200, vec![])]).await;
    sink(spec(server.url("http"))).health_check().await.unwrap();
    assert_eq!(server.requests()[0].method, "HEAD");
}

#[test]
fn builder_builds_webhook_and_syslog_sinks() {
    let built = AlertSinkFactory::build(
        &resolved(spec("http://127.0.0.1:1/hook".to_owned())),
        &delivery(),
        Some(&NetworkSinkBuilder),
    )
    .unwrap();
    assert_eq!(built.name(), "hook");
    assert!(!built.is_local());

    let syslog = daemoneye_lib::alerting::config::SyslogSpec {
        name: "sys".to_owned(),
        transport: daemoneye_lib::alerting::config::SyslogTransport::Udp,
        address: "127.0.0.1:514".to_owned(),
        facility: "local0".to_owned(),
        app_name: "daemoneye".to_owned(),
        ca_file: None,
        max_message_bytes: 1024,
        allow_plaintext: true,
        include_command_line: false,
        include_user: false,
    };
    let result = AlertSinkFactory::build(
        &ResolvedSink {
            spec: SinkSpec::Syslog(syslog),
            token: None,
            username: None,
            password: None,
        },
        &delivery(),
        Some(&NetworkSinkBuilder),
    );
    assert_eq!(result.unwrap().name(), "sys");
}
