//! Syslog sink: one RFC 5424 message per alert over UDP, TCP, TLS or a Unix socket.

mod format;
mod transport;

use crate::ensure_crypto_provider;
use crate::error::config_error;
use async_trait::async_trait;
use daemoneye_lib::alerting::config::{SyslogSpec, SyslogTransport};
use daemoneye_lib::alerting::{
    AlertSink, AlertingError, DeliveryResult, IncludeFlags, ResolvedSink, SinkSpec, host_of,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::Alert;
use format::{MessageParts, format_rfc5424, parse_facility};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls_platform_verifier::BuilderVerifierExt;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use transport::Transport;

/// A syslog sink holding its transport and the fixed parts of every message.
pub struct SyslogSink {
    name: String,
    facility: u8,
    app_name: String,
    max_bytes: usize,
    flags: IncludeFlags,
    attempt_timeout: Duration,
    transport: Transport,
}

impl std::fmt::Debug for SyslogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyslogSink")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Trust anchors from `ca_file` when set (replacing the platform roots), else the platform.
fn tls_config(ca_file: Option<&Path>) -> Result<rustls::ClientConfig, AlertingError> {
    let builder = rustls::ClientConfig::builder();
    let Some(path) = ca_file else {
        return Ok(builder
            .with_platform_verifier()
            .map_err(|_failed| config_error("syslog platform verifier is unavailable"))?
            .with_no_client_auth());
    };
    let pem =
        std::fs::read(path).map_err(|_unreadable| config_error("syslog ca_file cannot be read"))?;
    let mut roots = rustls::RootCertStore::empty();
    for parsed in CertificateDer::pem_slice_iter(&pem) {
        let cert = parsed
            .map_err(|_invalid| config_error("syslog ca_file holds an invalid PEM certificate"))?;
        roots
            .add(cert)
            .map_err(|_rejected| config_error("syslog ca_file certificate is not usable"))?;
    }
    if roots.is_empty() {
        return Err(config_error("syslog ca_file holds no certificate"));
    }
    Ok(builder.with_root_certificates(roots).with_no_client_auth())
}

fn build_transport(spec: &SyslogSpec) -> Result<Transport, AlertingError> {
    let address = spec.address.clone();
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "SyslogTransport is non_exhaustive"
    )]
    match spec.transport {
        SyslogTransport::Udp => Ok(Transport::Udp {
            address,
            socket: Mutex::new(None),
        }),
        SyslogTransport::Tcp => Ok(Transport::Tcp {
            address,
            conn: Mutex::new(None),
        }),
        SyslogTransport::Tls => {
            ensure_crypto_provider();
            let config = tls_config(spec.ca_file.as_deref())?;
            let server_name = ServerName::try_from(host_of(&address).to_owned())
                .map_err(|_invalid| config_error("syslog address host is not a valid TLS name"))?;
            Ok(Transport::Tls {
                address,
                server_name,
                connector: TlsConnector::from(std::sync::Arc::new(config)),
                conn: Mutex::new(None),
            })
        }
        #[cfg(unix)]
        SyslogTransport::Unix => Ok(Transport::Unix {
            path: address.into(),
        }),
        _ => Err(config_error("unsupported syslog transport")),
    }
}

impl SyslogSink {
    /// Build the sink for a resolved syslog spec.
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] when the spec is not a syslog spec, the facility
    /// is not one syslog defines, the transport is unsupported on this platform, the TLS host
    /// is not a valid name, or the `ca_file` cannot be read or holds no certificate. Messages
    /// never carry the address.
    pub fn new(resolved: &ResolvedSink, delivery: &DeliveryConfig) -> Result<Self, AlertingError> {
        let SinkSpec::Syslog(ref spec) = resolved.spec else {
            return Err(config_error("not a syslog spec"));
        };
        let facility = parse_facility(&spec.facility)
            .ok_or_else(|| config_error("syslog facility is not a name syslog defines"))?;
        Ok(Self {
            name: spec.name.as_str().to_owned(),
            facility,
            app_name: spec.app_name.clone(),
            max_bytes: spec.max_message_bytes,
            flags: IncludeFlags {
                command_line: spec.include_command_line,
                user: spec.include_user,
            },
            attempt_timeout: Duration::from_secs(delivery.attempt_timeout_secs),
            transport: build_transport(spec)?,
        })
    }
}

#[async_trait]
impl AlertSink for SyslogSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let started = Instant::now();
        let line = format_rfc5424(&MessageParts {
            facility: self.facility,
            app_name: &self.app_name,
            max_bytes: self.max_bytes,
            flags: self.flags,
            alert,
        });
        self.transport.send(&line, self.attempt_timeout).await?;
        Ok(DeliveryResult::since(&self.name, started))
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        Ok(self.transport.check().await?)
    }

    fn is_fire_and_forget(&self) -> bool {
        self.transport.is_fire_and_forget()
    }
}
