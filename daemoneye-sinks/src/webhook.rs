//! Webhook sink: one HTTPS POST of the external projection per alert.

use crate::ensure_crypto_provider;
use crate::error::{HEALTH_TIMEOUT, classify_reqwest, config_error};
use crate::key::wire_key;
use async_trait::async_trait;
use daemoneye_lib::alerting::config::{WebhookAuth, WebhookSpec};
use daemoneye_lib::alerting::{
    AlertSink, AlertingError, Credentials, DeliveryError, DeliveryResult, ExternalAlert,
    IncludeFlags, ResolvedSink, SinkSpec, parse_retry_after,
};
use daemoneye_lib::config::DeliveryConfig;
use daemoneye_lib::models::Alert;
use reqwest::header::{HeaderName, HeaderValue, RETRY_AFTER};
use secrecy::ExposeSecret;
use std::time::{Duration, Instant};

const MAX_RETRY_AFTER: Duration = Duration::from_hours(1);
const MAX_KEY_HEADER_CHARS: usize = 256;
const ALERT_ID_HEADER: &str = "x-daemoneye-alert-id";
const DEDUP_KEY_HEADER: &str = "x-daemoneye-dedup-key";
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A webhook sink holding one client and a prebuilt, sensitive auth header.
pub struct WebhookSink {
    name: String,
    url: reqwest::Url,
    client: reqwest::Client,
    auth: Option<(HeaderName, HeaderValue)>,
    flags: IncludeFlags,
}

impl std::fmt::Debug for WebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSink")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Standard padded base64; only for the basic-auth credential.
fn base64(input: &[u8]) -> String {
    let symbol = |index: u32| {
        char::from(
            BASE64
                .get(usize::try_from(index & 63).unwrap_or(0))
                .copied()
                .unwrap_or(b'A'),
        )
    };
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(symbol(triple >> 18));
        out.push(symbol(triple >> 12));
        out.push(if chunk.len() > 1 {
            symbol(triple >> 6)
        } else {
            '='
        });
        out.push(if chunk.len() > 2 { symbol(triple) } else { '=' });
    }
    out
}

fn sensitive(value: &str) -> Result<HeaderValue, AlertingError> {
    let mut header = HeaderValue::from_str(value)
        .map_err(|_invalid| config_error("webhook credential is not a valid header value"))?;
    header.set_sensitive(true);
    Ok(header)
}

/// The auth header for the spec's scheme; the resolver shaped the credentials to match it.
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "WebhookAuth is non_exhaustive"
)]
fn auth_header(
    spec: &WebhookSpec,
    resolved: &ResolvedSink,
) -> Result<Option<(HeaderName, HeaderValue)>, AlertingError> {
    let mismatch = || config_error("webhook credentials do not match its auth scheme");
    let token = || match resolved.credentials {
        Credentials::Token(ref token) => Ok(token.expose_secret()),
        Credentials::None | Credentials::Basic { .. } => Err(mismatch()),
    };
    match spec.auth {
        WebhookAuth::None => match resolved.credentials {
            Credentials::None => Ok(None),
            Credentials::Token(_) | Credentials::Basic { .. } => Err(mismatch()),
        },
        WebhookAuth::Bearer => Ok(Some((
            reqwest::header::AUTHORIZATION,
            sensitive(&format!("Bearer {}", token()?))?,
        ))),
        WebhookAuth::Basic => match resolved.credentials {
            Credentials::Basic {
                ref username,
                ref password,
            } => {
                let pair = format!("{}:{}", username.expose_secret(), password.expose_secret());
                Ok(Some((
                    reqwest::header::AUTHORIZATION,
                    sensitive(&format!("Basic {}", base64(pair.as_bytes())))?,
                )))
            }
            Credentials::None | Credentials::Token(_) => Err(mismatch()),
        },
        WebhookAuth::Header => {
            let name = spec
                .header_name
                .as_deref()
                .ok_or_else(|| config_error("webhook header_name is not set"))?;
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_invalid| {
                config_error("webhook header_name is not a valid header name")
            })?;
            Ok(Some((header_name, sensitive(token()?)?)))
        }
        _ => Err(config_error("unsupported webhook auth scheme")),
    }
}

fn build_client(spec: &WebhookSpec) -> Result<reqwest::Client, AlertingError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(spec.timeout_secs));
    if let Some(ref path) = spec.ca_file {
        let pem = std::fs::read(path)
            .map_err(|_unreadable| config_error("webhook ca_file cannot be read"))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|_invalid| config_error("webhook ca_file holds no valid PEM certificate"))?;
        if certs.is_empty() {
            return Err(config_error("webhook ca_file holds no certificate"));
        }
        // `tls_certs_only` replaces the OS roots with the file's certificates.
        builder = builder.tls_certs_only(certs);
    }
    builder
        .build()
        .map_err(|_failed| config_error("webhook HTTP client cannot be built"))
}

/// Header-safe form of a key: printable ASCII only, bounded.
fn header_safe(value: &str) -> HeaderValue {
    let text: String = value
        .chars()
        .take(MAX_KEY_HEADER_CHARS)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    HeaderValue::from_str(&text).unwrap_or_else(|_invalid| HeaderValue::from_static(""))
}

impl WebhookSink {
    /// Build the sink for a resolved webhook spec.
    ///
    /// The client timeout is the sink's `timeout_secs`; the worker's global attempt timeout
    /// still caps each send from outside, so the smaller of the two applies. Nothing else in
    /// `delivery` reaches the client, which is why that argument goes unread here.
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] when the spec is not a webhook, the credentials
    /// do not fit the auth scheme or are not a valid header value, the URL does not parse, or
    /// the `ca_file` cannot be read. Messages never carry the URL or a credential.
    pub fn new(resolved: &ResolvedSink, _delivery: &DeliveryConfig) -> Result<Self, AlertingError> {
        let SinkSpec::Webhook(ref spec) = resolved.spec else {
            return Err(config_error("not a webhook spec"));
        };
        ensure_crypto_provider();
        let url = reqwest::Url::parse(&spec.url)
            .map_err(|_invalid| config_error("webhook url does not parse"))?;
        Ok(Self {
            name: spec.name.clone(),
            url,
            client: build_client(spec)?,
            auth: auth_header(spec, resolved)?,
            flags: IncludeFlags {
                command_line: spec.include_command_line,
                user: spec.include_user,
            },
        })
    }

    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let builder = self.client.request(method, self.url.clone());
        match self.auth {
            Some((ref name, ref value)) => builder.header(name.clone(), value.clone()),
            None => builder,
        }
    }
}

/// Status and `Retry-After` of a response, reading nothing else.
fn judge(response: reqwest::Response) -> Result<(), DeliveryError> {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| parse_retry_after(text, MAX_RETRY_AFTER));
    drop(response);
    if status.is_success() {
        Ok(())
    } else {
        Err(DeliveryError::from_http_status(
            status.as_u16(),
            retry_after,
        ))
    }
}

#[async_trait]
impl AlertSink for WebhookSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let started = Instant::now();
        let response = self
            .request(reqwest::Method::POST)
            .header(ALERT_ID_HEADER, header_safe(&alert.id.to_string()))
            .header(
                DEDUP_KEY_HEADER,
                header_safe(&wire_key(&alert.deduplication_key, MAX_KEY_HEADER_CHARS)),
            )
            .json(&ExternalAlert::project(alert, self.flags))
            .send()
            .await
            .map_err(|error| classify_reqwest(&error.without_url()))?;
        judge(response)?;
        Ok(DeliveryResult::since(&self.name, started))
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        let response = self
            .request(reqwest::Method::HEAD)
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|error| classify_reqwest(&error.without_url()))?;
        judge(response)?;
        Ok(())
    }
}
