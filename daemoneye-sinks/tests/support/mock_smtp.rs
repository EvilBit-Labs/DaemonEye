//! A loopback SMTP server speaking the minimum dialogue, scripted per connection.
//!
//! Serves one connection at a time. Every command, the authenticated identity, the
//! envelope and the raw message are recorded. Failure replies carry [`REPLY_SENTINEL`]
//! so a test can prove the sink never keeps reply text.
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
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout
)]

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

/// Free text the server puts in every failure reply.
pub const REPLY_SENTINEL: &str = "reply-text-sentinel";

/// How the server secures the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Advertise `STARTTLS` and upgrade on request.
    Starttls,
    /// Advertise no `STARTTLS`; refuse to upgrade.
    PlainOnly,
    /// Wrap the stream in TLS before the greeting.
    ImplicitTls,
}

/// What the server answers.
#[derive(Debug, Clone)]
pub struct Script {
    pub mode: Mode,
    /// Reply code to `AUTH`: 235 or 535.
    pub auth_code: u16,
    /// Reply code to every `RCPT TO`: 250, 450 or 550.
    pub rcpt_code: u16,
    /// Names the leaf certificate is valid for.
    pub leaf_names: Vec<String>,
}

impl Script {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            auth_code: 235,
            rcpt_code: 250,
            leaf_names: vec!["localhost".to_owned(), "127.0.0.1".to_owned()],
        }
    }
}

/// Everything the server saw.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    pub commands: Vec<String>,
    pub auth_user: Option<String>,
    pub auth_secret: Option<String>,
    pub mail_from: Option<String>,
    pub rcpt_to: Vec<String>,
    pub message: Option<String>,
}

impl Recorded {
    /// Whether any command began with `verb`, case-insensitively.
    pub fn saw(&self, verb: &str) -> bool {
        self.commands
            .iter()
            .any(|c| c.to_ascii_uppercase().starts_with(verb))
    }

    /// The first header named `name` in the recorded message, unfolded.
    pub fn header(&self, name: &str) -> Option<String> {
        let message = self.message.as_deref()?;
        let head = message.split("\r\n\r\n").next()?;
        let unfolded = head.replace("\r\n ", " ").replace("\r\n\t", " ");
        let prefix = format!("{}:", name.to_ascii_lowercase());
        unfolded
            .split("\r\n")
            .find(|line| line.to_ascii_lowercase().starts_with(&prefix))
            .map(|line| line[prefix.len()..].trim().to_owned())
    }

    /// The message body after the header block.
    pub fn body(&self) -> String {
        self.message
            .as_deref()
            .and_then(|m| m.split_once("\r\n\r\n"))
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default()
    }
}

/// A throwaway CA and a leaf certificate for the given names.
pub struct SmtpPki {
    pub ca_pem: String,
    leaf_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl SmtpPki {
    pub fn generate(names: &[String]) -> Self {
        let ca_key = rcgen::KeyPair::generate().expect("ca key");
        let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "daemoneye smtp test ca");
        let ca = ca_params.self_signed(&ca_key).expect("ca cert");
        let leaf_key = rcgen::KeyPair::generate().expect("leaf key");
        let leaf = rcgen::CertificateParams::new(names.to_vec())
            .expect("leaf params")
            .signed_by(&leaf_key, &ca, &ca_key)
            .expect("leaf cert");
        Self {
            ca_pem: ca.pem(),
            leaf_der: leaf.der().to_vec(),
            key_der: leaf_key.serialize_der(),
        }
    }

    fn acceptor(&self) -> TlsAcceptor {
        let cert = rustls::pki_types::CertificateDer::from(self.leaf_der.clone());
        let key = rustls::pki_types::PrivateKeyDer::try_from(self.key_der.clone()).expect("key");
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("server config");
        TlsAcceptor::from(Arc::new(config))
    }
}

type Shared = Arc<Mutex<Recorded>>;

/// A running mock server; the accept loop stops when it is dropped.
pub struct MockSmtp {
    pub port: u16,
    pub ca_pem: String,
    recorded: Shared,
    task: JoinHandle<()>,
}

impl Drop for MockSmtp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockSmtp {
    pub async fn start(script: Script) -> Self {
        let pki = SmtpPki::generate(&script.leaf_names);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let recorded = Shared::default();
        let ca_pem = pki.ca_pem.clone();
        let shared = Arc::clone(&recorded);
        let task = tokio::spawn(async move {
            let acceptor = pki.acceptor();
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                serve(tcp, &script, &acceptor, &shared).await;
            }
        });
        Self {
            port,
            ca_pem,
            recorded,
            task,
        }
    }

    pub fn recorded(&self) -> Recorded {
        self.recorded.lock().unwrap().clone()
    }
}

enum Outcome<S> {
    Done,
    Upgrade(S),
}

async fn serve(tcp: tokio::net::TcpStream, script: &Script, acceptor: &TlsAcceptor, rec: &Shared) {
    if script.mode == Mode::ImplicitTls {
        if let Ok(tls) = acceptor.accept(tcp).await {
            drop(dialogue(tls, script, rec, true, true).await);
        }
        return;
    }
    if let Outcome::Upgrade(tcp) = dialogue(tcp, script, rec, false, true).await
        && let Ok(tls) = acceptor.accept(tcp).await
    {
        drop(dialogue(tls, script, rec, true, false).await);
    }
}

async fn reply<S: AsyncRead + AsyncWrite + Unpin>(io: &mut BufReader<S>, text: &str) {
    let _ignored = io.get_mut().write_all(text.as_bytes()).await;
    let _ignored = io.get_mut().flush().await;
}

fn failure(code: u16) -> String {
    format!("{code} {REPLY_SENTINEL}\r\n")
}

async fn dialogue<S>(
    stream: S,
    script: &Script,
    rec: &Shared,
    secure: bool,
    greet: bool,
) -> Outcome<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut io = BufReader::new(stream);
    if greet {
        reply(&mut io, "220 mock ESMTP\r\n").await;
    }
    let mut line = String::new();
    loop {
        line.clear();
        if io.read_line(&mut line).await.unwrap_or(0) == 0 {
            return Outcome::Done;
        }
        let command = line.trim_end().to_owned();
        rec.lock().unwrap().commands.push(command.clone());
        let upper = command.to_ascii_uppercase();
        if upper.starts_with("EHLO") {
            let mut caps = String::from("250-mock\r\n250-8BITMIME\r\n");
            if !secure && script.mode == Mode::Starttls {
                caps.push_str("250-STARTTLS\r\n");
            }
            caps.push_str("250 AUTH PLAIN LOGIN\r\n");
            reply(&mut io, &caps).await;
        } else if upper == "STARTTLS" {
            if script.mode == Mode::Starttls && !secure {
                reply(&mut io, "220 go ahead\r\n").await;
                return Outcome::Upgrade(io.into_inner());
            }
            reply(&mut io, &failure(502)).await;
        } else if upper.starts_with("AUTH PLAIN") {
            auth(&mut io, &command, script, rec).await;
        } else if upper.starts_with("MAIL FROM") {
            rec.lock().unwrap().mail_from = Some(address(&command));
            reply(&mut io, "250 ok\r\n").await;
        } else if upper.starts_with("RCPT TO") {
            reply_rcpt(&mut io, &command, script, rec).await;
        } else if upper == "DATA" {
            reply(&mut io, "354 go\r\n").await;
            read_message(&mut io, rec).await;
            reply(&mut io, "250 queued\r\n").await;
        } else if upper == "QUIT" {
            reply(&mut io, "221 bye\r\n").await;
            return Outcome::Done;
        } else {
            reply(&mut io, "250 ok\r\n").await;
        }
    }
}

fn address(command: &str) -> String {
    let start = command.find('<').map_or(0, |i| i + 1);
    let end = command.find('>').unwrap_or(command.len());
    command[start..end].to_owned()
}

async fn reply_rcpt<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    command: &str,
    script: &Script,
    rec: &Shared,
) {
    if script.rcpt_code == 250 {
        rec.lock().unwrap().rcpt_to.push(address(command));
        reply(io, "250 ok\r\n").await;
    } else {
        reply(io, &failure(script.rcpt_code)).await;
    }
}

fn record_auth(rec: &Shared, user: Option<String>, secret: Option<String>) {
    let mut guard = rec.lock().unwrap();
    guard.auth_user = user;
    guard.auth_secret = secret;
}

async fn auth<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    command: &str,
    script: &Script,
    rec: &Shared,
) {
    let mut initial = command.split_whitespace().nth(2).map(str::to_owned);
    if initial.is_none() {
        reply(io, "334 \r\n").await;
        let mut line = String::new();
        let _read = io.read_line(&mut line).await;
        initial = Some(line.trim().to_owned());
    }
    let decoded = decode_base64(&initial.unwrap_or_default());
    let mut parts = decoded.split('\0').skip(1);
    let user = parts.next().map(str::to_owned);
    let secret = parts.next().map(str::to_owned);
    record_auth(rec, user, secret);
    if script.auth_code == 235 {
        reply(io, "235 ok\r\n").await;
    } else {
        reply(io, &failure(script.auth_code)).await;
    }
}

async fn read_message<S: AsyncRead + AsyncWrite + Unpin>(io: &mut BufReader<S>, rec: &Shared) {
    let mut message = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if io.read_line(&mut line).await.unwrap_or(0) == 0 || line == ".\r\n" {
            break;
        }
        message.push_str(&line);
    }
    rec.lock().unwrap().message = Some(message);
}

fn decode_base64(input: &str) -> String {
    let mut bits = 0_u32;
    let mut count = 0_u32;
    let mut out = Vec::new();
    for c in input.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from((bits >> count) & 0xff).unwrap());
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
