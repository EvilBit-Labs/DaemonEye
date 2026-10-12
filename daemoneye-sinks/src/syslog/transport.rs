//! Syslog wire transports: UDP, TCP and TLS (octet counting), and Unix datagram.
//!
//! Stream transports connect lazily and drop the connection after any failure, so the
//! next attempt reconnects.

use crate::error::classify_tls_io;
use daemoneye_lib::alerting::{DeliveryError, DeliveryErrorClass};
use rustls::pki_types::ServerName;
use std::future::Future;
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

pub(super) const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

#[allow(
    clippy::large_enum_variant,
    reason = "one Transport per sink, built once at startup"
)]
pub(super) enum Transport {
    Udp {
        address: String,
        socket: Mutex<Option<UdpSocket>>,
    },
    Tcp {
        address: String,
        conn: Mutex<Option<TcpStream>>,
    },
    Tls {
        address: String,
        server_name: ServerName<'static>,
        connector: TlsConnector,
        conn: Mutex<Option<TlsStream<TcpStream>>>,
    },
    #[cfg(unix)]
    Unix { path: std::path::PathBuf },
}

const fn timeout_error() -> DeliveryError {
    DeliveryError::Transient {
        class: DeliveryErrorClass::Timeout,
        retry_after: None,
    }
}

async fn within<T>(
    limit: Duration,
    work: impl Future<Output = Result<T, DeliveryError>>,
) -> Result<T, DeliveryError> {
    tokio::time::timeout(limit, work)
        .await
        .map_err(|_elapsed| timeout_error())?
}

async fn dial(address: &str, limit: Duration) -> Result<TcpStream, DeliveryError> {
    within(limit, async {
        TcpStream::connect(address)
            .await
            .map_err(DeliveryError::from)
    })
    .await
}

async fn dial_tls(
    address: &str,
    name: &ServerName<'static>,
    connector: &TlsConnector,
    limit: Duration,
) -> Result<TlsStream<TcpStream>, DeliveryError> {
    let tcp = dial(address, limit).await?;
    within(limit, async {
        connector
            .connect(name.clone(), tcp)
            .await
            .map_err(|error| classify_tls_io(&error))
    })
    .await
}

async fn dial_udp(address: &str) -> Result<UdpSocket, DeliveryError> {
    let target = tokio::net::lookup_host(address)
        .await?
        .next()
        .ok_or_else(|| std::io::Error::from(ErrorKind::NotFound))?;
    let local = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(local).await?;
    socket.connect(target).await?;
    Ok(socket)
}

async fn udp_send(
    slot: &Mutex<Option<UdpSocket>>,
    address: &str,
    line: &str,
) -> Result<(), DeliveryError> {
    let mut guard = slot.lock().await;
    if guard.is_none() {
        *guard = Some(dial_udp(address).await?);
    }
    let outcome = match *guard {
        Some(ref udp) => udp.send(line.as_bytes()).await.map(drop),
        None => Ok(()),
    };
    // ICMP port-unreachable for an earlier datagram: nothing to confirm.
    let refused = matches!(outcome, Err(ref error) if error.kind() == ErrorKind::ConnectionRefused);
    if outcome.is_err() && !refused {
        *guard = None;
    }
    drop(guard);
    match outcome {
        Err(error) if !refused => Err(error.into()),
        Err(_) | Ok(()) => Ok(()),
    }
}

/// Whether the peer has closed (or spoken to) a stream we only ever write to.
fn peer_closed(stream: &TcpStream) -> bool {
    let mut probe = [0_u8; 1];
    !matches!(stream.try_read(&mut probe), Err(ref error) if error.kind() == ErrorKind::WouldBlock)
}

async fn write_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    frame: &[u8],
    limit: Duration,
) -> Result<(), DeliveryError> {
    within(limit, async {
        stream.write_all(frame).await?;
        stream.flush().await?;
        Ok(())
    })
    .await
}

/// Send `frame` on the cached connection, dialling first when there is none or the
/// peer closed it; any failure discards the connection.
async fn stream_send<S: AsyncWrite + Unpin>(
    slot: &Mutex<Option<S>>,
    is_closed: impl Fn(&S) -> bool,
    connect: impl Future<Output = Result<S, DeliveryError>>,
    frame: &[u8],
    limit: Duration,
) -> Result<(), DeliveryError> {
    let mut guard = slot.lock().await;
    if guard.as_ref().is_some_and(is_closed) {
        *guard = None;
    }
    let stream = match guard.as_mut() {
        Some(stream) => stream,
        None => guard.insert(connect.await?),
    };
    let outcome = write_frame(stream, frame, limit).await;
    if outcome.is_err() {
        *guard = None;
    }
    outcome
}

impl Transport {
    /// Whether a send cannot tell that anything received the message.
    pub(super) const fn is_fire_and_forget(&self) -> bool {
        match *self {
            Self::Udp { .. } => true,
            #[cfg(unix)]
            Self::Unix { .. } => true,
            Self::Tcp { .. } | Self::Tls { .. } => false,
        }
    }

    pub(super) async fn send(&self, line: &str, limit: Duration) -> Result<(), DeliveryError> {
        let frame = format!("{} {line}", line.len());
        match *self {
            Self::Udp {
                ref address,
                ref socket,
            } => udp_send(socket, address, line).await,
            Self::Tcp {
                ref address,
                ref conn,
            } => {
                stream_send(
                    conn,
                    peer_closed,
                    dial(address, limit),
                    frame.as_bytes(),
                    limit,
                )
                .await
            }
            Self::Tls {
                ref address,
                ref server_name,
                ref connector,
                ref conn,
            } => {
                stream_send(
                    conn,
                    |tls: &TlsStream<TcpStream>| peer_closed(tls.get_ref().0),
                    dial_tls(address, server_name, connector, limit),
                    frame.as_bytes(),
                    limit,
                )
                .await
            }
            #[cfg(unix)]
            Self::Unix { ref path } => {
                let socket = tokio::net::UnixDatagram::unbound()?;
                socket.send_to(line.as_bytes(), path).await?;
                Ok(())
            }
        }
    }

    /// Stream transports open a connection (completing any handshake) and drop it.
    pub(super) async fn check(&self) -> Result<(), DeliveryError> {
        match *self {
            Self::Tcp { ref address, .. } => dial(address, HEALTH_TIMEOUT).await.map(drop),
            Self::Tls {
                ref address,
                ref server_name,
                ref connector,
                ..
            } => dial_tls(address, server_name, connector, HEALTH_TIMEOUT)
                .await
                .map(drop),
            Self::Udp { .. } => Ok(()),
            #[cfg(unix)]
            Self::Unix { .. } => Ok(()),
        }
    }
}
