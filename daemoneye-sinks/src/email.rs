//! Email sink: one plain-text SMTP message per alert, always over TLS.

use crate::ensure_crypto_provider;
use crate::error::{HEALTH_TIMEOUT, classify_tls, tls_cause};
use async_trait::async_trait;
use daemoneye_lib::alerting::config::{EmailSpec, EmailTls};
use daemoneye_lib::alerting::{
    AlertSink, AlertingError, Credentials, DeliveryError, DeliveryErrorClass, DeliveryResult,
    ExternalAlert, IncludeFlags, ResolvedSink, SinkSpec, sanitise_control,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::Alert;
use lettre::message::header::{HeaderName, HeaderValue};
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use secrecy::ExposeSecret;
use std::error::Error as StdError;
use std::time::{Duration, Instant};

const PAYLOAD_ERROR: DeliveryError = DeliveryError::Permanent {
    class: DeliveryErrorClass::Payload,
};
const DEDUP_KEY_HEADER: HeaderName = HeaderName::new_from_ascii_str("X-DaemonEye-Dedup-Key");

/// Which TLS mode the transport was built with; there is no plaintext variant.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsKind {
    /// `STARTTLS` is mandatory; a server that does not offer it is refused.
    Required,
    /// TLS from the first byte.
    Wrapper,
}

/// An SMTP sink holding one transport and the parsed envelope addresses.
pub struct EmailSink {
    name: String,
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    to: Vec<Mailbox>,
    subject_prefix: String,
    flags: IncludeFlags,
    tls_kind: TlsKind,
}

impl std::fmt::Debug for EmailSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailSink")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn config_error(name: &str, what: &str) -> AlertingError {
    AlertingError::ConfigurationError(format!("email sink {name}: {what}"))
}

fn parse_mailbox(name: &str, address: &str, what: &str) -> Result<Mailbox, AlertingError> {
    address.parse().map_err(|_invalid| config_error(name, what))
}

fn tls_parameters(
    spec: &EmailSpec,
    extra_root_pem: Option<&[u8]>,
) -> Result<TlsParameters, AlertingError> {
    let mut builder = TlsParameters::builder(spec.host.clone());
    if let Some(pem) = extra_root_pem {
        let root = Certificate::from_pem(pem)
            .map_err(|_invalid| config_error(&spec.name, "root certificate is not valid PEM"))?;
        builder = builder.add_root_certificate(root);
    }
    builder
        .build()
        .map_err(|_failed| config_error(&spec.name, "TLS parameters cannot be built"))
}

impl EmailSink {
    /// Build the sink for a resolved email spec, trusting the platform roots.
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] naming the sink when the spec is not an
    /// email spec or an address does not parse. Messages never carry an address or a
    /// credential.
    pub fn new(resolved: &ResolvedSink, delivery: &DeliveryConfig) -> Result<Self, AlertingError> {
        Self::new_with_root(resolved, delivery, None)
    }

    /// As [`EmailSink::new`], additionally trusting one PEM root. Test seam: the spec has
    /// no `ca_file`, so loopback tests cannot reach the platform trust store.
    ///
    /// # Errors
    ///
    /// As [`EmailSink::new`], plus an unparsable root.
    #[doc(hidden)]
    pub fn new_with_root(
        resolved: &ResolvedSink,
        delivery: &DeliveryConfig,
        extra_root_pem: Option<&[u8]>,
    ) -> Result<Self, AlertingError> {
        let SinkSpec::Email(ref spec) = resolved.spec else {
            return Err(AlertingError::ConfigurationError(
                "not an email spec".to_owned(),
            ));
        };
        ensure_crypto_provider();
        let name = spec.name.as_str();
        let from = parse_mailbox(name, &spec.from, "from address does not parse")?;
        let to = spec
            .to
            .iter()
            .map(|address| parse_mailbox(name, address, "a recipient address does not parse"))
            .collect::<Result<Vec<_>, _>>()?;
        let params = tls_parameters(spec, extra_root_pem)?;
        let (tls, tls_kind) = match spec.tls {
            EmailTls::Starttls => (Tls::Required(params), TlsKind::Required),
            EmailTls::Implicit => (Tls::Wrapper(params), TlsKind::Wrapper),
            _ => return Err(config_error(name, "unsupported tls mode")),
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&spec.host)
            .port(spec.port)
            .tls(tls)
            .timeout(Some(Duration::from_secs(delivery.attempt_timeout_secs)))
            .authentication(vec![Mechanism::Plain, Mechanism::Login]);
        match resolved.credentials {
            Credentials::Basic {
                ref username,
                ref password,
            } => {
                // ponytail: lettre keeps these two Strings for the transport's life and does not
                // zeroize them; the exposure is bounded to this process's heap.
                builder = builder.credentials(SmtpCredentials::new(
                    username.expose_secret().to_owned(),
                    password.expose_secret().to_owned(),
                ));
            }
            Credentials::None => {}
            Credentials::Token(_) => {
                return Err(config_error(name, "a token credential does not fit SMTP"));
            }
        }
        Ok(Self {
            name: spec.name.clone(),
            transport: builder.build(),
            from,
            to,
            subject_prefix: spec.subject_prefix.clone(),
            flags: IncludeFlags {
                command_line: spec.include_command_line,
                user: spec.include_user,
            },
            tls_kind,
        })
    }

    /// The TLS mode this sink was built with.
    #[doc(hidden)]
    #[must_use]
    pub const fn tls_kind(&self) -> TlsKind {
        self.tls_kind
    }

    fn message(&self, alert: &Alert) -> Result<Message, DeliveryError> {
        let projection = serde_json::to_string_pretty(&ExternalAlert::project(alert, self.flags))
            .map_err(|_failed| PAYLOAD_ERROR)?;
        let summary = sanitise_control(&format!(
            "[{}] {} - {}: {}",
            alert.severity, alert.detection_rule_id, alert.title, alert.description
        ));
        let subject = sanitise_control(&format!("{}{}", self.subject_prefix, alert.title));
        let dedup = sanitise_control(&alert.deduplication_key);
        let mut builder = Message::builder()
            .from(self.from.clone())
            .subject(subject)
            .message_id(Some(format!("<{}@daemoneye>", alert.id)))
            .raw_header(HeaderValue::new(DEDUP_KEY_HEADER, dedup));
        for recipient in &self.to {
            builder = builder.to(recipient.clone());
        }
        builder
            .body(format!("{summary}\n\n{projection}\n"))
            .map_err(|_failed| PAYLOAD_ERROR)
    }
}

/// Classify a failed SMTP exchange. Reply text is never read.
fn classify_smtp(error: &lettre::transport::smtp::Error) -> DeliveryError {
    let transient = DeliveryError::transient;
    if let Some(code) = error.status() {
        return DeliveryError::from_smtp_status(u16::from(code));
    }
    if error.is_timeout() {
        return transient(DeliveryErrorClass::Timeout);
    }
    // lettre reports a rejected certificate as a connection error wrapping the rustls error.
    if let Some(cause) = tls_cause(error) {
        return classify_tls(cause);
    }
    if error.is_tls() {
        return transient(DeliveryErrorClass::Tls);
    }
    if error.is_client() {
        // A server without STARTTLS or AUTH: a mismatch with the sink's configuration.
        return DeliveryError::Misconfigured {
            class: DeliveryErrorClass::Protocol,
        };
    }
    if error.is_response() {
        return transient(DeliveryErrorClass::Protocol);
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return DeliveryError::from(std::io::Error::from(io.kind()));
        }
        source = cause.source();
    }
    transient(DeliveryErrorClass::Connect)
}

#[async_trait]
impl AlertSink for EmailSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let started = Instant::now();
        let message = self.message(alert)?;
        self.transport
            .send(message)
            .await
            .map_err(|error| classify_smtp(&error))?;
        Ok(DeliveryResult::since(&self.name, started))
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        let probe = tokio::time::timeout(HEALTH_TIMEOUT, self.transport.test_connection())
            .await
            .map_err(|_elapsed| DeliveryError::transient(DeliveryErrorClass::Timeout))?;
        match probe {
            Ok(true) => Ok(()),
            Ok(false) => Err(DeliveryError::transient(DeliveryErrorClass::Connect).into()),
            Err(error) => Err(classify_smtp(&error).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use secrecy::SecretString;

    fn resolved(tls: EmailTls) -> ResolvedSink {
        ResolvedSink {
            spec: SinkSpec::Email(EmailSpec {
                name: "mail".to_owned(),
                host: "smtp.example.com".to_owned(),
                port: 587,
                tls,
                username_env: None,
                password_env: None,
                from: "a@example.com".to_owned(),
                to: vec!["b@example.com".to_owned()],
                subject_prefix: String::new(),
                include_command_line: false,
                include_user: false,
            }),
            credentials: Credentials::Basic {
                username: SecretString::from("u".to_owned()),
                password: SecretString::from("p".to_owned()),
            },
        }
    }

    #[test]
    fn email_tls_modes_map_to_required_and_wrapper_only() {
        let delivery = DeliveryConfig::default();
        let starttls = EmailSink::new(&resolved(EmailTls::Starttls), &delivery).unwrap();
        let implicit = EmailSink::new(&resolved(EmailTls::Implicit), &delivery).unwrap();
        assert_eq!(starttls.tls_kind(), TlsKind::Required);
        assert_eq!(implicit.tls_kind(), TlsKind::Wrapper);
    }
}
