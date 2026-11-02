//! A loopback HTTP/1.1 server scripted per request, optionally behind TLS.
//!
//! Serves one connection at a time. Each request pops the next scripted [`Response`];
//! an empty script answers 200. Every request is recorded.
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
    clippy::arithmetic_side_effects,
    clippy::print_stdout
)]

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

const LARGE_BODY_CHUNK: usize = 64 * 1024;
const LARGE_BODY_PAUSE: Duration = Duration::from_millis(5);
const HANG: Duration = Duration::from_secs(30);

/// What the server does with the next request.
#[derive(Debug, Clone)]
pub enum Response {
    /// Reply with a status, extra headers and an empty body.
    Status(u16, Vec<(String, String)>),
    /// Read the request and never answer.
    Hang,
    /// Close the connection without answering.
    Reset,
    /// Announce a body of this many bytes, then stream it in 64 KiB chunks, 5 ms apart.
    LargeBody(u16, usize),
}

/// One request the server saw.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    /// The first header named `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("json body")
    }
}

/// A throwaway CA and a leaf certificate for `localhost` and `127.0.0.1`.
pub struct TestPki {
    pub ca_pem: String,
    leaf_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl TestPki {
    pub fn generate() -> Self {
        Self::generate_for(&["localhost", "127.0.0.1"])
    }

    /// Like [`Self::generate`], with a leaf valid only for `names`.
    pub fn generate_for(names: &[&str]) -> Self {
        let ca_key = rcgen::KeyPair::generate().expect("ca key");
        let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "daemoneye test ca");
        let ca = ca_params.self_signed(&ca_key).expect("ca cert");

        let leaf_key = rcgen::KeyPair::generate().expect("leaf key");
        let leaf_params = rcgen::CertificateParams::new(
            names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
        )
        .expect("leaf params");
        let leaf = leaf_params
            .signed_by(&leaf_key, &ca, &ca_key)
            .expect("leaf cert");
        Self {
            ca_pem: ca.pem(),
            leaf_der: leaf.der().to_vec(),
            key_der: leaf_key.serialize_der(),
        }
    }

    /// Write the CA PEM into `dir` and return its path.
    pub fn write_ca(&self, dir: &Path) -> PathBuf {
        let path = dir.join("ca.pem");
        std::fs::write(&path, &self.ca_pem).expect("write ca");
        path
    }

    pub fn acceptor(&self) -> TlsAcceptor {
        let cert = rustls::pki_types::CertificateDer::from(self.leaf_der.clone());
        let key = rustls::pki_types::PrivateKeyDer::try_from(self.key_der.clone()).expect("key");
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("server config");
        TlsAcceptor::from(Arc::new(config))
    }
}

type Recordings = Arc<Mutex<Vec<Recorded>>>;
type Script = Arc<Mutex<VecDeque<Response>>>;

/// A running mock server; the accept loop stops when it is dropped.
pub struct MockHttp {
    pub port: u16,
    recorded: Recordings,
    script: Script,
    bytes_written: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Drop for MockHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockHttp {
    /// Plain HTTP on `127.0.0.1:0`.
    pub async fn start(script: Vec<Response>) -> Self {
        Self::spawn(script, None).await
    }

    /// HTTPS on `127.0.0.1:0` with a leaf from `pki`.
    pub async fn start_tls(script: Vec<Response>, pki: &TestPki) -> Self {
        Self::spawn(script, Some(pki.acceptor())).await
    }

    async fn spawn(script: Vec<Response>, tls: Option<TlsAcceptor>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let recorded: Recordings = Arc::default();
        let script: Script = Arc::new(Mutex::new(VecDeque::from(script)));
        let bytes_written = Arc::new(AtomicUsize::new(0));
        let (rec, scr, written) = (recorded.clone(), script.clone(), bytes_written.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                match tls {
                    Some(ref acceptor) => {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            serve(stream, &rec, &scr, &written).await;
                        }
                    }
                    None => serve(stream, &rec, &scr, &written).await,
                }
            }
        });
        Self {
            port,
            recorded,
            script,
            bytes_written,
            task,
        }
    }

    /// `scheme://127.0.0.1:<port>/hook`.
    pub fn url(&self, scheme: &str) -> String {
        format!("{scheme}://127.0.0.1:{}/hook", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().expect("lock").clone()
    }

    /// Body bytes written for a `LargeBody` reply.
    pub fn bytes_written(&self) -> usize {
        self.bytes_written.load(Ordering::SeqCst)
    }

    /// Queue another scripted reply.
    pub fn push(&self, response: Response) {
        self.script.lock().expect("lock").push_back(response);
    }
}

async fn serve<S>(mut stream: S, recorded: &Recordings, script: &Script, written: &AtomicUsize)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    recorded.lock().expect("lock").push(request);
    let next = script
        .lock()
        .expect("lock")
        .pop_front()
        .unwrap_or_else(|| Response::Status(200, Vec::new()));
    match next {
        Response::Status(code, headers) => {
            let mut head =
                format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n");
            for (name, value) in headers {
                write!(head, "{name}: {value}\r\n").expect("write to string");
            }
            head.push_str("\r\n");
            drop(stream.write_all(head.as_bytes()).await);
        }
        Response::Hang => tokio::time::sleep(HANG).await,
        Response::Reset => {}
        Response::LargeBody(code, size) => {
            let head = format!("HTTP/1.1 {code} X\r\nContent-Length: {size}\r\n\r\n");
            if stream.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let chunk = vec![b'x'; LARGE_BODY_CHUNK];
            let mut remaining = size;
            while remaining > 0 {
                let take = remaining.min(LARGE_BODY_CHUNK);
                if stream.write_all(&chunk[..take]).await.is_err() {
                    return;
                }
                written.fetch_add(take, Ordering::SeqCst);
                remaining -= take;
                tokio::time::sleep(LARGE_BODY_PAUSE).await;
            }
        }
    }
}

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<Recorded> {
    let mut buf = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0_u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}
