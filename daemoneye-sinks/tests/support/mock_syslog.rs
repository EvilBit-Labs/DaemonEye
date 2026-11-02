//! Loopback syslog receivers: UDP, octet-counted TCP, TLS and Unix datagram.
//!
//! Each records every message it receives as a string. The stream receivers serve one
//! connection at a time and can be told to drop it, then accept the next.
#![allow(
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::shadow_unrelated,
    clippy::shadow_reuse
)]

use super::mock_http::TestPki;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

type Messages = Arc<Mutex<Vec<String>>>;

/// A running receiver; its task stops when it is dropped.
pub struct MockSyslog {
    /// `host:port` (or the socket path for Unix).
    pub address: String,
    received: Messages,
    kick: Arc<Notify>,
    accepted: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Drop for MockSyslog {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockSyslog {
    fn new(address: String, received: Messages, kick: Arc<Notify>, task: JoinHandle<()>) -> Self {
        Self {
            address,
            received,
            kick,
            accepted: Arc::default(),
            task,
        }
    }

    /// Everything received so far.
    pub fn messages(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }

    /// Stream connections accepted so far (always zero for the datagram receivers).
    pub fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Wait until `count` messages arrived and return them.
    pub async fn wait_for(&self, count: usize) -> Vec<String> {
        for _ in 0..200 {
            let got = self.messages();
            if got.len() >= count {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {count} messages, got {:?}", self.messages());
    }

    /// Drop the current stream connection; the next one is accepted afresh.
    pub async fn drop_connection(&self) {
        self.kick.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    pub async fn udp() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap().to_string();
        let received = Messages::default();
        let sink = received.clone();
        let task = tokio::spawn(async move {
            let mut buf = vec![0_u8; 70_000];
            while let Ok(n) = socket.recv(&mut buf).await {
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
        Self::new(address, received, Arc::default(), task)
    }

    pub async fn tcp() -> Self {
        Self::stream(None).await
    }

    pub async fn tls(pki: &TestPki) -> Self {
        Self::stream(Some(pki.acceptor())).await
    }

    async fn stream(tls: Option<tokio_rustls::TlsAcceptor>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let received = Messages::default();
        let kick = Arc::new(Notify::new());
        let accepted = Arc::new(AtomicUsize::new(0));
        let (sink, stop, count) = (received.clone(), kick.clone(), accepted.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                match tls {
                    Some(ref acceptor) => {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            read_frames(stream, &sink, &stop).await;
                        }
                    }
                    None => read_frames(stream, &sink, &stop).await,
                }
            }
        });
        let mut server = Self::new(address, received, kick, task);
        server.accepted = accepted;
        server
    }

    #[cfg(unix)]
    pub fn unix(path: &std::path::Path) -> Self {
        let socket = tokio::net::UnixDatagram::bind(path).unwrap();
        let received = Messages::default();
        let sink = received.clone();
        let task = tokio::spawn(async move {
            let mut buf = vec![0_u8; 70_000];
            while let Ok(n) = socket.recv(&mut buf).await {
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
        Self::new(path.display().to_string(), received, Arc::default(), task)
    }
}

/// Read `"<len> <msg>"` frames until EOF, an error or a kick.
async fn read_frames<S: AsyncRead + Unpin>(mut stream: S, sink: &Messages, kick: &Notify) {
    loop {
        let frame = tokio::select! {
            () = kick.notified() => return,
            frame = read_frame(&mut stream) => frame,
        };
        match frame {
            Some(message) => sink.lock().unwrap().push(message),
            None => return,
        }
    }
}

async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut len = 0_usize;
    loop {
        let byte = stream.read_u8().await.ok()?;
        match byte {
            b' ' => break,
            b'0'..=b'9' => len = len * 10 + usize::from(byte - b'0'),
            _ => return None,
        }
    }
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body).await.ok()?;
    Some(String::from_utf8_lossy(&body).into_owned())
}
