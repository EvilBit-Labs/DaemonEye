//! A plain-HTTP loopback server scripted per request. Serves one connection at a time; each
//! request pops the next scripted [`Response`] (an empty script answers 200) and is recorded.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::pattern_type_mismatch,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::clone_on_ref_ptr,
    clippy::partial_pub_fields
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const HANG: Duration = Duration::from_secs(30);

/// What the server does with the next request.
#[derive(Debug, Clone, Copy)]
pub enum Response {
    /// Reply with a status and an empty body.
    Status(u16),
    /// Read the request and never answer.
    Hang,
}

/// One request the server saw.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Recordings = Arc<Mutex<Vec<Recorded>>>;
type Script = Arc<Mutex<VecDeque<Response>>>;

/// A running mock server; dropping it, or calling [`close`](Self::close), closes the listener.
pub struct MockHttp {
    pub port: u16,
    recorded: Recordings,
    script: Script,
    task: JoinHandle<()>,
}

impl Drop for MockHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockHttp {
    pub async fn start(script: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let recorded: Recordings = Arc::default();
        let script: Script = Arc::new(Mutex::new(VecDeque::from(script)));
        let (rec, scr) = (recorded.clone(), script.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serve(stream, &rec, &scr).await;
            }
        });
        Self {
            port,
            recorded,
            script,
            task,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/hook", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }

    pub fn push(&self, response: Response) {
        self.script.lock().unwrap().push_back(response);
    }

    /// Stop the accept loop and close the listener: later connections are refused.
    pub async fn close(&mut self) {
        self.task.abort();
        drop((&mut self.task).await);
    }
}

async fn serve(mut stream: TcpStream, recorded: &Recordings, script: &Script) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    recorded.lock().unwrap().push(request);
    let next = script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Response::Status(200));
    match next {
        Response::Status(code) => {
            let head =
                format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            drop(stream.write_all(head.as_bytes()).await);
        }
        Response::Hang => tokio::time::sleep(HANG).await,
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
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
    let headers: Vec<(String, String)> = head
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut have = buf.len() - header_end - 4;
    while have < length {
        let mut chunk = [0_u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        have += n;
    }
    Some(Recorded { headers })
}
