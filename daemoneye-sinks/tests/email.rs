//! Email sink against a scripted loopback SMTP server.
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

use daemoneye_lib::alerting::config::{EmailSpec, EmailTls};
use daemoneye_lib::alerting::{
    AlertSink, AlertingError, Credentials, DeliveryError, DeliveryErrorClass, ResolvedSink,
    SinkBuilder, SinkSpec,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::process::ProcessRecord;
use daemoneye_lib::models::{Alert, AlertSeverity, Completeness};
use daemoneye_sinks::{EmailSink, NetworkSinkBuilder};
use secrecy::SecretString;
use support::mock_smtp::{MockSmtp, Mode, REPLY_SENTINEL, Script};

const USER: &str = "smtp-user";
const PASSWORD: &str = "PASSWORD-sentinel-4c7e";
const CMD_SENTINEL: &str = "CMDLINE-sentinel";

fn delivery() -> DeliveryConfig {
    DeliveryConfig {
        attempt_timeout_secs: 5,
        ..DeliveryConfig::default()
    }
}

fn spec(port: u16, tls: EmailTls) -> EmailSpec {
    EmailSpec {
        name: "mail".parse().unwrap(),
        host: "127.0.0.1".to_owned(),
        port,
        tls,
        username_env: Some("U".to_owned()),
        password_env: Some("P".to_owned()),
        from: "DaemonEye <daemoneye@example.com>".to_owned(),
        to: vec!["a@example.com".to_owned(), "b@example.com".to_owned()],
        subject_prefix: "[DE] ".to_owned(),
        include_command_line: false,
        include_user: false,
    }
}

fn resolved(spec: EmailSpec) -> ResolvedSink {
    ResolvedSink {
        spec: SinkSpec::Email(spec),
        credentials: Credentials::Basic {
            username: SecretString::from(USER.to_owned()),
            password: SecretString::from(PASSWORD.to_owned()),
        },
    }
}

fn sink(server: &MockSmtp, spec: EmailSpec) -> EmailSink {
    EmailSink::new_with_root(&resolved(spec), &delivery(), Some(server.ca_pem.as_bytes())).unwrap()
}

fn alert_titled(title: &str) -> Alert {
    let mut record = ProcessRecord::new(10, "bash".to_owned());
    record.command_line = Some(CMD_SENTINEL.to_owned());
    Alert::new(
        AlertSeverity::High,
        title,
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

fn assert_no_leaks(error: &AlertingError) {
    for text in [format!("{error}"), format!("{error:?}")] {
        assert!(!text.contains(REPLY_SENTINEL), "reply text leaked");
        assert!(!text.contains(PASSWORD), "password leaked");
        assert!(!text.contains(USER), "username leaked");
        assert!(!text.contains("example.com"), "recipient leaked");
    }
}

#[tokio::test]
async fn delivers_one_message_with_auth_envelope_and_headers() {
    let server = MockSmtp::start(Script::new(Mode::Starttls)).await;
    let alert = alert_titled("clean title");
    let sink = sink(&server, spec(server.port, EmailTls::Starttls));
    let result = sink.send(&alert).await.unwrap();
    assert_eq!(result.sink_name, "mail");

    let seen = server.recorded();
    assert_eq!(seen.auth_user.as_deref(), Some(USER));
    assert_eq!(seen.auth_secret.as_deref(), Some(PASSWORD));
    assert_eq!(seen.mail_from.as_deref(), Some("daemoneye@example.com"));
    assert_eq!(seen.rcpt_to, ["a@example.com", "b@example.com"]);
    assert_eq!(
        seen.header("message-id"),
        Some(format!("<{}@daemoneye>", alert.id))
    );
    assert_eq!(
        seen.header("x-daemoneye-dedup-key"),
        Some(alert.deduplication_key.clone())
    );
    assert_eq!(seen.header("subject"), Some("[DE] clean title".to_owned()));
    assert!(seen.body().contains(&alert.id.to_string()));
    assert!(format!("{sink:?}").contains("mail"));
    assert!(!format!("{sink:?}").contains(PASSWORD));
}

#[tokio::test]
async fn crlf_in_the_title_cannot_inject_a_header() {
    let server = MockSmtp::start(Script::new(Mode::Starttls)).await;
    let sink = sink(&server, spec(server.port, EmailTls::Starttls));
    sink.send(&alert_titled("hello\r\nBcc: evil@example.com"))
        .await
        .unwrap();

    let seen = server.recorded();
    assert!(seen.header("bcc").is_none(), "header injected");
    let subject = seen.header("subject").unwrap();
    assert!(subject.starts_with("[DE] hello"));
    assert!(!subject.contains(['\r', '\n']));
    assert_eq!(seen.rcpt_to.len(), 2, "envelope gained a recipient");
}

#[tokio::test]
async fn command_line_travels_only_when_the_sink_opts_in() {
    let server = MockSmtp::start(Script::new(Mode::Starttls)).await;
    let default_sink = sink(&server, spec(server.port, EmailTls::Starttls));
    default_sink.send(&alert_titled("t")).await.unwrap();
    assert!(!server.recorded().body().contains(CMD_SENTINEL));

    let mut opted_in = spec(server.port, EmailTls::Starttls);
    opted_in.include_command_line = true;
    sink(&server, opted_in)
        .send(&alert_titled("t"))
        .await
        .unwrap();
    assert!(server.recorded().body().contains(CMD_SENTINEL));
}

#[tokio::test]
async fn smtp_replies_map_to_typed_errors_without_reply_text() {
    let mut script = Script::new(Mode::Starttls);
    script.auth_code = 535;
    let server = MockSmtp::start(script).await;
    let result = sink(&server, spec(server.port, EmailTls::Starttls))
        .send(&alert_titled("t"))
        .await;
    assert_no_leaks(result.as_ref().unwrap_err());
    assert_eq!(
        delivery_error(result),
        DeliveryError::Misconfigured {
            class: DeliveryErrorClass::Auth
        }
    );

    for (code, transient) in [(450, true), (550, false)] {
        let mut script = Script::new(Mode::Starttls);
        script.rcpt_code = code;
        let server = MockSmtp::start(script).await;
        let result = sink(&server, spec(server.port, EmailTls::Starttls))
            .send(&alert_titled("t"))
            .await;
        assert_no_leaks(result.as_ref().unwrap_err());
        let class = DeliveryErrorClass::SmtpStatus(code);
        let expected = if transient {
            DeliveryError::Transient {
                class,
                retry_after: None,
            }
        } else {
            DeliveryError::Permanent { class }
        };
        assert_eq!(delivery_error(result), expected);
    }
}

#[tokio::test]
async fn starttls_is_never_skipped_when_the_server_does_not_offer_it() {
    let server = MockSmtp::start(Script::new(Mode::PlainOnly)).await;
    let result = sink(&server, spec(server.port, EmailTls::Starttls))
        .send(&alert_titled("t"))
        .await;
    assert_no_leaks(result.as_ref().unwrap_err());
    // lettre reports a missing STARTTLS as a client error; that is a config mismatch.
    assert_eq!(
        delivery_error(result),
        DeliveryError::Misconfigured {
            class: DeliveryErrorClass::Protocol
        }
    );
    let seen = server.recorded();
    assert!(!seen.saw("MAIL FROM"), "mail was sent in the clear");
    assert!(!seen.saw("AUTH"), "credentials were sent in the clear");
}

#[tokio::test]
async fn implicit_tls_delivers() {
    let server = MockSmtp::start(Script::new(Mode::ImplicitTls)).await;
    let alert = alert_titled("implicit");
    sink(&server, spec(server.port, EmailTls::Implicit))
        .send(&alert)
        .await
        .unwrap();
    let seen = server.recorded();
    assert_eq!(seen.auth_user.as_deref(), Some(USER));
    assert!(seen.body().contains(&alert.id.to_string()));
}

#[tokio::test]
async fn a_certificate_for_another_name_is_a_verification_failure() {
    let mut script = Script::new(Mode::Starttls);
    script.leaf_names = vec!["other.invalid".to_owned()];
    let server = MockSmtp::start(script).await;
    let result = sink(&server, spec(server.port, EmailTls::Starttls))
        .send(&alert_titled("t"))
        .await;
    assert_no_leaks(result.as_ref().unwrap_err());
    assert_eq!(delivery_error(result), DeliveryError::tls_verification());
    assert!(
        !server.recorded().saw("AUTH"),
        "credentials sent over an unverified channel"
    );
}

#[tokio::test]
async fn health_check_connects_and_noops() {
    let server = MockSmtp::start(Script::new(Mode::Starttls)).await;
    sink(&server, spec(server.port, EmailTls::Starttls))
        .health_check()
        .await
        .unwrap();
}

#[test]
fn builder_builds_an_email_sink_and_rejects_an_unparsable_sender() {
    let good = resolved(spec(2525, EmailTls::Starttls));
    let built = NetworkSinkBuilder
        .build(&good, &delivery())
        .expect("email sink builds");
    assert_eq!(built.name(), "mail");

    let mut bad = spec(2525, EmailTls::Starttls);
    bad.from = "not an address".to_owned();
    match NetworkSinkBuilder.build(&resolved(bad), &delivery()) {
        Err(AlertingError::ConfigurationError(message)) => {
            assert!(message.contains("mail"), "message names the sink");
            assert!(
                !message.contains("not an address"),
                "message echoes the address"
            );
        }
        other => panic!("expected a configuration error, got {:?}", other.err()),
    }
}
