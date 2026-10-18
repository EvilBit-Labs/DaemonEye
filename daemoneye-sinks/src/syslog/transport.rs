//! Syslog wire transports: UDP, TCP and TLS (octet counting), and Unix datagram.
//!
//! Stream transports connect lazily and drop the connection after any failure, so the
//! next attempt reconnects.

use crate::error::{HEALTH_TIMEOUT, classify_tls_io};
use daemoneye_lib::alerting::{DeliveryError, DeliveryErrorClass};
use rustls::pki_types::ServerName;
use std::future::Future;
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

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
    DeliveryError::transient(DeliveryErrorClass::Timeout)
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
    // ICMP port-unreachable, reported on this send for an earlier datagram: the receiver is
    // not listening, so this is a transient connect failure and the row is retried. The socket
    // itself is still good.
    let refused = matches!(outcome, Err(ref error) if error.kind() == ErrorKind::ConnectionRefused);
    if outcome.is_err() && !refused {
        *guard = None;
    }
    drop(guard);
    outcome.map_err(DeliveryError::from)
}

/// Whether a stream we only ever write to is still open: a read that stays pending means the
/// peer is quiet, anything else (EOF, an error, bytes) means the connection is not worth
/// reusing. Reading through the TLS layer lets rustls consume post-handshake records such as
/// TLS 1.3 session tickets, which a raw-socket probe would mistake for the peer speaking.
async fn is_open<S: AsyncRead + Unpin>(stream: &mut S) -> bool {
    let mut probe = [0_u8; 1];
    tokio::time::timeout(Duration::ZERO, stream.read(&mut probe))
        .await
        .is_err()
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
async fn stream_send<S: AsyncRead + AsyncWrite + Unpin>(
    slot: &Mutex<Option<S>>,
    connect: impl Future<Output = Result<S, DeliveryError>>,
    frame: &[u8],
    limit: Duration,
) -> Result<(), DeliveryError> {
    let mut guard = slot.lock().await;
    let mut cached = guard.take();
    let reusable = match cached {
        Some(ref mut stream) => is_open(stream).await,
        None => false,
    };
    let mut stream = match cached.filter(|_stream| reusable) {
        Some(open) => open,
        None => connect.await?,
    };
    // The stream is out of the slot while writing, so a cancelled or failed write drops it
    // instead of leaving a partial frame on a cached connection.
    write_frame(&mut stream, frame, limit).await?;
    *guard = Some(stream);
    drop(guard);
    Ok(())
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
        // RFC 6587 octet counting; only the stream transports frame.
        let octet_counted = |text: &str| format!("{} {text}", text.len());
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
                    dial(address, limit),
                    octet_counted(line).as_bytes(),
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
                    dial_tls(address, server_name, connector, limit),
                    octet_counted(line).as_bytes(),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{is_open, stream_send};
    use std::future::ready;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::Mutex;

    /// A quiet peer: reads stay pending.
    struct Stuck;

    impl AsyncRead for Stuck {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for Stuck {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_quiet_peer_is_open_and_an_eof_or_spoken_peer_is_not() {
        assert!(is_open(&mut Stuck).await);
        assert!(!is_open(&mut tokio::io::empty()).await, "EOF");
        assert!(!is_open(&mut b"unexpected".as_slice()).await, "bytes");
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_write_drops_the_cached_stream() {
        let slot = Mutex::new(Some(Stuck));
        let send = stream_send(&slot, ready(Ok(Stuck)), b"frame", Duration::from_mins(1));
        tokio::select! {
            result = send => panic!("write completed: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
        assert!(slot.lock().await.is_none());
    }
}
