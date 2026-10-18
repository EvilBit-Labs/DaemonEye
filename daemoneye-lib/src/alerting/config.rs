//! Typed alert-sink configuration and credential resolution.
//!
//! `alerting.sinks[].config` is parsed into a [`SinkSpec`] and validated inside
//! [`Config::validate`](crate::config::Config::validate). A spec holds environment-variable
//! *names*; [`CredentialResolver`] turns them into [`SecretString`]s at build time and fails closed.
//! No credential reaches `Debug`, serialised config, logs or errors.

use crate::alerting::OutputFormat;
use crate::config::{AlertSinkConfig, AlertingConfig, ConfigError, ConfigLoader, check_range};
use crate::storage::DELIVERY_KEY_SEP;
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};

const KNOWN_SINK_TYPES: [&str; 5] = ["stdout", "file", "webhook", "syslog", "email"];
const PEM_CERTIFICATE_MARKER: &[u8] = b"-----BEGIN CERTIFICATE-----";
const REDACTED: &str = "[REDACTED]";

const TIMEOUT_SECS_MIN: u64 = 1;
const TIMEOUT_SECS_MAX: u64 = 120;
const SYSLOG_MESSAGE_BYTES_MIN: usize = 480;
const SYSLOG_MESSAGE_BYTES_MAX: usize = 65_507;

const fn default_format() -> OutputFormat {
    OutputFormat::Json
}
const fn default_timeout_secs() -> u64 {
    10
}
const fn default_max_message_bytes() -> usize {
    2_048
}
const fn default_email_port() -> u16 {
    587
}
fn default_facility() -> String {
    "daemon".to_owned()
}
fn default_app_name() -> String {
    "daemoneye".to_owned()
}
fn default_subject_prefix() -> String {
    "[DaemonEye] ".to_owned()
}

/// Webhook authentication scheme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum WebhookAuth {
    /// No credential.
    #[default]
    None,
    /// `Authorization: Bearer <token_env>`.
    Bearer,
    /// HTTP basic auth from `username_env` and `password_env`.
    Basic,
    /// `<header_name>: <token_env>`.
    Header,
}

/// Syslog transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SyslogTransport {
    /// Plaintext datagrams.
    Udp,
    /// Plaintext stream.
    Tcp,
    /// TLS stream.
    Tls,
    /// Unix domain socket.
    Unix,
}

/// SMTP connection security.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum EmailTls {
    /// Upgrade a plain connection with STARTTLS.
    #[default]
    Starttls,
    /// TLS from the first byte.
    Implicit,
}

/// Stdout sink.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdoutSpec {
    /// Unique sink name.
    pub name: String,
    /// Output format.
    #[serde(default = "default_format")]
    pub format: OutputFormat,
}

/// File sink.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    /// Unique sink name.
    pub name: String,
    /// Destination file.
    pub path: PathBuf,
    /// Output format.
    #[serde(default = "default_format")]
    pub format: OutputFormat,
}

/// Webhook sink.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSpec {
    /// Unique sink name.
    pub name: String,
    /// Endpoint URL, without user-info, query or fragment.
    pub url: String,
    /// Authentication scheme.
    #[serde(default)]
    pub auth: WebhookAuth,
    /// Variable holding the bearer or header token.
    #[serde(default)]
    pub token_env: Option<String>,
    /// Variable holding the basic-auth username.
    #[serde(default)]
    pub username_env: Option<String>,
    /// Variable holding the basic-auth password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Header carrying the token when `auth = "header"`.
    #[serde(default)]
    pub header_name: Option<String>,
    /// Per-request timeout, 1..=120 seconds; the smaller of this and
    /// `alerting.delivery.attempt_timeout_secs` applies to each send.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Permit plain HTTP; only for an unauthenticated sink.
    #[serde(default)]
    pub allow_http: bool,
    /// Trust-anchor PEM that replaces the platform roots for this sink.
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    /// Include the command line in the payload.
    #[serde(default)]
    pub include_command_line: bool,
    /// Include the user in the payload.
    #[serde(default)]
    pub include_user: bool,
}

/// Syslog sink.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyslogSpec {
    /// Unique sink name.
    pub name: String,
    /// Transport.
    pub transport: SyslogTransport,
    /// `host:port`, or a socket path for `unix`.
    pub address: String,
    /// Syslog facility name (`kern`, `user`, `mail`, `daemon`, `auth`, `syslog`, `lpr`, `news`,
    /// `uucp`, `cron`, `authpriv`, `ftp`, `local0`..=`local7`); an unknown name refuses startup.
    #[serde(default = "default_facility")]
    pub facility: String,
    /// APP-NAME field.
    #[serde(default = "default_app_name")]
    pub app_name: String,
    /// Trust-anchor PEM that replaces the platform roots for this sink.
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    /// Message size cap, 480..=65 507 bytes.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: usize,
    /// Permit UDP/TCP to a non-loopback address.
    #[serde(default)]
    pub allow_plaintext: bool,
    /// Include the command line in the message.
    #[serde(default)]
    pub include_command_line: bool,
    /// Include the user in the message.
    #[serde(default)]
    pub include_user: bool,
}

/// Email sink.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailSpec {
    /// Unique sink name.
    pub name: String,
    /// SMTP host.
    pub host: String,
    /// SMTP port.
    #[serde(default = "default_email_port")]
    pub port: u16,
    /// Connection security.
    #[serde(default)]
    pub tls: EmailTls,
    /// Variable holding the SMTP username.
    #[serde(default)]
    pub username_env: Option<String>,
    /// Variable holding the SMTP password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Sender address.
    pub from: String,
    /// Recipient addresses; at least one.
    pub to: Vec<String>,
    /// Prefix for the subject line.
    #[serde(default = "default_subject_prefix")]
    pub subject_prefix: String,
    /// Include the command line in the body.
    #[serde(default)]
    pub include_command_line: bool,
    /// Include the user in the body.
    #[serde(default)]
    pub include_user: bool,
}

/// A parsed `alerting.sinks[]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SinkSpec {
    /// Stdout.
    Stdout(StdoutSpec),
    /// File.
    File(FileSpec),
    /// Webhook.
    Webhook(WebhookSpec),
    /// Syslog.
    Syslog(SyslogSpec),
    /// Email.
    Email(EmailSpec),
}

impl SinkSpec {
    /// The sink's unique name.
    #[must_use]
    pub fn name(&self) -> &str {
        match *self {
            Self::Stdout(ref s) => &s.name,
            Self::File(ref s) => &s.name,
            Self::Webhook(ref s) => &s.name,
            Self::Syslog(ref s) => &s.name,
            Self::Email(ref s) => &s.name,
        }
    }

    /// Whether the sink leaves the host.
    #[must_use]
    pub const fn is_network(&self) -> bool {
        matches!(self, Self::Webhook(_) | Self::Syslog(_) | Self::Email(_))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        match *self {
            Self::Webhook(ref w) => validate_webhook(w),
            Self::Syslog(ref s) => validate_syslog(s),
            Self::Email(ref e) => validate_email(e),
            Self::Stdout(_) | Self::File(_) => Ok(()),
        }
    }
}

fn name_of(sink: &AlertSinkConfig) -> Option<&str> {
    sink.config.get("name").and_then(serde_json::Value::as_str)
}

fn invalid(label: &str, message: impl Display) -> ConfigError {
    ConfigError::ValidationError {
        message: format!("alerting.sinks[{label}]: {message}"),
    }
}

fn parse(sink: &AlertSinkConfig) -> Result<SinkSpec, String> {
    let value = sink.config.clone();
    match sink.sink_type.as_str() {
        "stdout" => serde_json::from_value(value).map(SinkSpec::Stdout),
        "file" => serde_json::from_value(value).map(SinkSpec::File),
        "webhook" => serde_json::from_value(value).map(SinkSpec::Webhook),
        "syslog" => serde_json::from_value(value).map(SinkSpec::Syslog),
        "email" => serde_json::from_value(value).map(SinkSpec::Email),
        other => return Err(format!("unknown sink_type '{other}'")),
    }
    .map_err(|e| format!("invalid {} sink: {e}", sink.sink_type))
}

/// Parse with the sink's name as the label, falling back to its index.
fn parse_at(index: usize, sink: &AlertSinkConfig) -> Result<SinkSpec, ConfigError> {
    let label = name_of(sink).map_or_else(|| index.to_string(), str::to_owned);
    parse(sink).map_err(|message| invalid(&label, message))
}

impl AlertingConfig {
    /// Parse every enabled sink; disabled sinks are skipped, so their credentials are never
    /// resolved.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] naming the first sink that does not parse.
    pub fn enabled_specs(&self) -> Result<Vec<SinkSpec>, ConfigError> {
        let mut specs = Vec::new();
        for (index, sink) in self.sinks.iter().enumerate() {
            if sink.enabled {
                specs.push(parse_at(index, sink)?);
            }
        }
        Ok(specs)
    }

    pub(crate) fn validate_sinks(&self) -> Result<(), ConfigError> {
        let mut seen = HashSet::new();
        for (index, sink) in self.sinks.iter().enumerate() {
            if !sink.enabled {
                if !KNOWN_SINK_TYPES.contains(&sink.sink_type.as_str()) {
                    return Err(invalid(
                        &index.to_string(),
                        format_args!("unknown sink_type '{}'", sink.sink_type),
                    ));
                }
                continue;
            }
            let spec = parse_at(index, sink)?;
            let name = spec.name();
            // The delivery key is `sink U+001F alert id`, so the separator cannot be in a name.
            if name.contains(DELIVERY_KEY_SEP) {
                return Err(invalid(name, "name must not contain U+001F"));
            }
            if !seen.insert(name.to_owned()) {
                return Err(invalid(name, "duplicate sink name"));
            }
            spec.validate()?;
        }
        Ok(())
    }
}

fn validate_webhook(w: &WebhookSpec) -> Result<(), ConfigError> {
    let name = &w.name;
    check_range(
        &format!("alerting.sinks[{name}].timeout_secs"),
        &w.timeout_secs,
        &TIMEOUT_SECS_MIN,
        &TIMEOUT_SECS_MAX,
    )?;
    let is_http =
        check_webhook_url(&w.url).map_err(|why| invalid(name, format_args!("url {why}")))?;
    if is_http && !w.allow_http {
        return Err(invalid(
            name,
            "url is plain http; set allow_http = true to permit it",
        ));
    }
    if w.allow_http && w.auth != WebhookAuth::None {
        return Err(invalid(
            name,
            "allow_http is only valid with auth = \"none\"",
        ));
    }
    if w.auth == WebhookAuth::Header && w.header_name.is_none() {
        return Err(invalid(
            name,
            "header_name is required when auth = \"header\"",
        ));
    }
    w.ca_file
        .as_deref()
        .map_or(Ok(()), |path| validate_ca_file(name, path))
}

/// Returns whether the URL is plain http. Parsed by hand: only the shape R22 constrains.
fn check_webhook_url(url: &str) -> Result<bool, &'static str> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("must start with https:// or http://")?;
    let is_http = match scheme.to_ascii_lowercase().as_str() {
        "https" => false,
        "http" => true,
        _ => return Err("scheme must be https or http"),
    };
    if rest.contains(['?', '#']) {
        return Err("must not contain a query or fragment");
    }
    let authority = rest
        .split_once('/')
        .map_or(rest, |(authority, _)| authority);
    if authority.contains('@') {
        return Err("must not contain user-info");
    }
    if host_of(authority).is_empty() {
        return Err("has an empty host");
    }
    Ok(is_http)
}

/// Host part of `host`, `host:port` or `[v6]:port`, without IPv6 brackets.
pub fn host_of(authority: &str) -> &str {
    if let Some(inner) = authority.strip_prefix('[') {
        return inner.split_once(']').map_or(inner, |(host, _)| host);
    }
    authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host)
}

/// Checks `host:port` / `[v6]:port` shape without resolving DNS. An unbracketed
/// IPv6 literal is rejected as ambiguous.
fn is_host_port(address: &str) -> bool {
    let (host, port) = if let Some(inner) = address.strip_prefix('[') {
        match inner.split_once("]:") {
            Some(parts) => parts,
            None => return false,
        }
    } else {
        match address.rsplit_once(':') {
            Some((host, _)) if host.contains(':') => return false,
            Some(parts) => parts,
            None => return false,
        }
    };
    !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p != 0)
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn validate_syslog(s: &SyslogSpec) -> Result<(), ConfigError> {
    let name = &s.name;
    check_range(
        &format!("alerting.sinks[{name}].max_message_bytes"),
        &s.max_message_bytes,
        &SYSLOG_MESSAGE_BYTES_MIN,
        &SYSLOG_MESSAGE_BYTES_MAX,
    )?;
    if s.transport != SyslogTransport::Unix && !is_host_port(&s.address) {
        return Err(invalid(
            name,
            "address must be host:port with a port in 1..=65535",
        ));
    }
    match s.transport {
        SyslogTransport::Udp | SyslogTransport::Tcp => {
            if !s.allow_plaintext && !is_loopback(host_of(&s.address)) {
                return Err(invalid(
                    name,
                    "plaintext syslog to a non-loopback address needs allow_plaintext = true",
                ));
            }
        }
        SyslogTransport::Tls => {}
        SyslogTransport::Unix => {
            if cfg!(windows) {
                return Err(invalid(
                    name,
                    "transport \"unix\" is not supported on Windows",
                ));
            }
            ConfigLoader::validate_socket_path_for(
                &format!("alerting.sinks[{name}].address"),
                &s.address,
            )?;
        }
    }
    s.ca_file
        .as_deref()
        .map_or(Ok(()), |path| validate_ca_file(name, path))
}

fn validate_email(e: &EmailSpec) -> Result<(), ConfigError> {
    if e.to.is_empty() {
        return Err(invalid(&e.name, "to must list at least one recipient"));
    }
    if e.username_env.is_some() != e.password_env.is_some() {
        return Err(invalid(
            &e.name,
            "username_env and password_env must be set together",
        ));
    }
    Ok(())
}

fn validate_ca_file(name: &str, path: &Path) -> Result<(), ConfigError> {
    ConfigLoader::validate_path_no_traversal(path, &format!("alerting.sinks[{name}].ca_file"))?;
    let pem = std::fs::read(path)
        .map_err(|e| invalid(name, format_args!("ca_file cannot be read: {}", e.kind())))?;
    if !pem
        .windows(PEM_CERTIFICATE_MARKER.len())
        .any(|window| window == PEM_CERTIFICATE_MARKER)
    {
        return Err(invalid(name, "ca_file contains no PEM certificate"));
    }
    Ok(())
}

type Lookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Resolves the environment variables a [`SinkSpec`] references.
pub struct CredentialResolver {
    lookup: Lookup,
}

impl std::fmt::Debug for CredentialResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialResolver").finish_non_exhaustive()
    }
}

impl CredentialResolver {
    /// Resolve through `lookup`; tests inject one instead of mutating the process environment.
    pub fn new(lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            lookup: Box::new(lookup),
        }
    }

    /// Resolve from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(|name| std::env::var(name).ok())
    }

    /// Resolve the credentials `spec` requires.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] naming the sink and the variable (never its
    /// value) when a required variable is unnamed, unset or empty.
    pub fn resolve(&self, spec: &SinkSpec) -> Result<ResolvedSink, ConfigError> {
        let name = spec.name();
        let credentials = match *spec {
            SinkSpec::Webhook(ref w) => match w.auth {
                WebhookAuth::None => Credentials::None,
                WebhookAuth::Bearer | WebhookAuth::Header => {
                    Credentials::Token(self.required(name, "token_env", w.token_env.as_deref())?)
                }
                WebhookAuth::Basic => Credentials::Basic {
                    username: self.required(name, "username_env", w.username_env.as_deref())?,
                    password: self.required(name, "password_env", w.password_env.as_deref())?,
                },
            },
            SinkSpec::Email(ref e) => {
                match (e.username_env.as_deref(), e.password_env.as_deref()) {
                    (Some(user_var), Some(pass_var)) => Credentials::Basic {
                        username: self.required(name, "username_env", Some(user_var))?,
                        password: self.required(name, "password_env", Some(pass_var))?,
                    },
                    (None, None) => Credentials::None,
                    (Some(_), None) => return Err(invalid(name, "password_env is required")),
                    (None, Some(_)) => return Err(invalid(name, "username_env is required")),
                }
            }
            SinkSpec::Stdout(_) | SinkSpec::File(_) | SinkSpec::Syslog(_) => Credentials::None,
        };
        Ok(ResolvedSink {
            spec: spec.clone(),
            credentials,
        })
    }

    fn required(
        &self,
        sink: &str,
        field: &str,
        var: Option<&str>,
    ) -> Result<SecretString, ConfigError> {
        let name = var.ok_or_else(|| invalid(sink, format_args!("{field} is required")))?;
        match (self.lookup)(name) {
            Some(value) if !value.is_empty() => Ok(SecretString::from(value)),
            _ => Err(invalid(
                sink,
                format_args!(
                    "{field} names environment variable '{name}', which is unset or empty"
                ),
            )),
        }
    }
}

/// The secrets a sink sends, shaped so a half-set pair or a token on a sink without one cannot
/// be built.
#[allow(
    clippy::exhaustive_enums,
    reason = "a new credential shape must make every sink decide what to do with it"
)]
pub enum Credentials {
    /// The sink authenticates with nothing.
    None,
    /// A bearer or custom-header token.
    Token(SecretString),
    /// A username and password pair (HTTP basic auth or SMTP).
    Basic {
        /// The username.
        username: SecretString,
        /// The password.
        password: SecretString,
    },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::None => f.write_str("None"),
            Self::Token(_) => f.debug_tuple("Token").field(&REDACTED).finish(),
            Self::Basic { .. } => f
                .debug_struct("Basic")
                .field("username", &REDACTED)
                .field("password", &REDACTED)
                .finish(),
        }
    }
}

/// A sink spec with its credentials resolved. Secrets never render.
#[derive(Debug)]
pub struct ResolvedSink {
    /// The parsed spec.
    pub spec: SinkSpec,
    /// The secrets the spec's `*_env` fields named.
    pub credentials: Credentials,
}
