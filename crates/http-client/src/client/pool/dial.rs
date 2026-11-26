//! How the pool opens a connection: the lookup and the connect reqwest's connector makes (its
//! resolver's addresses, `TCP_NODELAY`, the dialer's keep-alive, the connect timeout over the
//! lookup, the connect and the TLS handshake), TLS as reqwest configures it (the platform's
//! verifier with the `certificates_file` roots, ALPN `h2` and `http/1.1`), then HTTP/2's handshake
//! with hyper's settings for its h2 client.

use std::{
    error::Error,
    fmt, io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures_util::future::BoxFuture;
use h2::{
    PingPong,
    client::{Builder as H2Builder, SendRequest},
};
use rustls::{
    ClientConfig,
    client::danger::ServerCertVerifier,
    crypto::{CryptoProvider, aws_lc_rs},
    pki_types::{CertificateDer, ServerName},
};
use rustls_platform_verifier::Verifier;
use socket2::{SockRef, TcpKeepalive};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::{Instant, timeout_at},
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use url::{Host, Url};

use super::super::{http2::MAX_HEADER_LIST, resolve::lookup};
use crate::{
    error::{ErrorKind, RequestError},
    metrics::{Event, Trace},
    roots::Roots,
    settings::{HttpClientSettings, KEEP_ALIVE_DEFAULT, KEEP_ALIVE_PROBES},
};

/// hyper's h2 client defaults (hyper 1.x `proto::h2::client::Config`), which reqwest's HTTP/2
/// connections take: the streams assumed before the peer's SETTINGS (Go's
/// `initialMaxConcurrentStreams` too), the stream and connection windows, the frame size, the
/// send buffer and the resets a peer may provoke.
const INITIAL_MAX_SEND_STREAMS: usize = 100;
const STREAM_WINDOW: u32 = 2 * 1024 * 1024;
const CONNECTION_WINDOW: u32 = 5 * 1024 * 1024;
const MAX_FRAME_SIZE: u32 = 16 * 1024;
const MAX_SEND_BUFFER: usize = 1024 * 1024;
const MAX_LOCAL_ERROR_RESETS: usize = 1024;

/// The streams a connection is counted to take while it is being opened, before its peer's
/// SETTINGS can say: what HTTP/2's handshake assumes.
pub(super) const ASSUMED_STREAMS: usize = INITIAL_MAX_SEND_STREAMS;

/// The port of a URL that names none.
const fn default_port(tls: bool) -> u16 {
    if tls { 443 } else { 80 }
}

/// A connection the pool opened: its handle, the future that drives it, and its pings.
pub(super) struct Opened {
    pub(super) send: SendRequest<Bytes>,
    pub(super) connection: BoxFuture<'static, ()>,
    pub(super) ping: Option<PingPong>,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened").finish_non_exhaustive()
    }
}

/// What a dial gave.
#[derive(Debug)]
pub(super) enum Dialed {
    /// An HTTP/2 connection.
    H2(Box<Opened>),
    /// Over TLS, the server answered ALPN with `http/1.1` or with nothing: the connection is
    /// closed, and the request goes over HTTP/1.1.
    Http1,
}

/// How a dial failed, before HTTP/2 began.
#[derive(Debug)]
pub(in super::super) struct DialError {
    phase: Phase,
    cause: Box<dyn Error + Send + Sync>,
}

/// Where a dial failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The lookup.
    Dns,
    /// The connect.
    Connect,
    /// The TLS handshake.
    Tls,
    /// The connect timeout passed.
    Timeout,
    /// HTTP/2's handshake: the client's preface and SETTINGS could not be written.
    Handshake,
}

impl DialError {
    fn new<E: Into<Box<dyn Error + Send + Sync>>>(phase: Phase, cause: E) -> Self {
        Self {
            phase,
            cause: cause.into(),
        }
    }

    /// Whether HTTP/2's handshake failed, the connection open: an h2c origin read none of it.
    pub(in super::super) fn in_handshake(&self) -> bool {
        self.phase == Phase::Handshake
    }

    /// The request's error, worded as reqwest words a failed connect.
    pub(in super::super) fn request_error(self: &Arc<Self>) -> RequestError {
        let kind = match self.phase {
            Phase::Timeout => ErrorKind::ConnectTimeout,
            Phase::Handshake => ErrorKind::Request,
            Phase::Dns | Phase::Connect | Phase::Tls => ErrorKind::Connect,
        };
        RequestError::chained(
            kind,
            "error sending request: client error (Connect)",
            Arc::clone(self),
        )
    }
}

impl fmt::Display for DialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.phase {
            Phase::Dns => "dns error",
            Phase::Connect => "tcp connect error",
            Phase::Tls => "tls handshake error",
            Phase::Timeout => "connect timeout",
            Phase::Handshake => "http2 handshake error",
        })
    }
}

impl Error for DialError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.cause)
    }
}

/// The connect timeout's error, as hyper-util words it.
#[derive(Debug, Clone, Copy)]
struct TimedOut;

impl fmt::Display for TimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation timed out")
    }
}

impl Error for TimedOut {}

/// What the pool dials with: the bidder client's resolver, dialer and TLS.
pub(in super::super) struct Dialer {
    /// The loopback guard of the client's resolver.
    loopback: Option<u16>,
    connect_timeout: Option<Duration>,
    keep_alive: Option<Duration>,
    tls: TlsConnector,
}

impl fmt::Debug for Dialer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dialer")
            .field("loopback", &self.loopback)
            .field("connect_timeout", &self.connect_timeout)
            .field("keep_alive", &self.keep_alive)
            .finish_non_exhaustive()
    }
}

impl Dialer {
    /// The dialer of a client with `settings`, trusting `roots` beside the platform's, under the
    /// loopback guard `loopback`.
    ///
    /// # Errors
    ///
    /// The verifier could not be built ([`ErrorKind::Build`]).
    pub(in super::super) fn new(
        settings: &HttpClientSettings,
        roots: &Roots,
        loopback: Option<u16>,
    ) -> Result<Self, RequestError> {
        // reqwest's: the process's default provider, else aws-lc-rs's; every TLS version; the
        // platform's verifier, with the extra roots when there are some.
        let provider = CryptoProvider::get_default()
            .map_or_else(|| Arc::new(aws_lc_rs::default_provider()), Arc::clone);
        let extra: Vec<CertificateDer<'static>> = roots.certificates().to_vec();
        let verifier = if extra.is_empty() {
            Verifier::new(Arc::clone(&provider))
        } else {
            Verifier::new_with_extra_roots(extra, Arc::clone(&provider))
        }
        .map_err(|e| RequestError::new(ErrorKind::Build, e))?;
        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(verifier);
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .map_err(|e| RequestError::new(ErrorKind::Build, e))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.enable_sni = true;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Self {
            loopback,
            connect_timeout: settings.dialer.connect_timeout(),
            keep_alive: settings.dialer.keep_alive(),
            tls: TlsConnector::from(Arc::new(config)),
        })
    }

    /// Opens a connection to `url`'s origin, over TLS for an `https` URL and with prior knowledge
    /// for an `http` one, charging its lookup to `trace` unless the connection turns out not to
    /// speak HTTP/2 (the request then goes over reqwest, whose resolver charges its own).
    pub(super) async fn dial(&self, url: &Url, trace: Option<&Trace>) -> Result<Dialed, DialError> {
        let tls = url.scheme() == "https";
        let mut dns = None;
        let deadline = self.connect_timeout.map(|timeout| {
            Instant::now()
                .checked_add(timeout)
                .unwrap_or_else(Instant::now)
        });
        let connecting = self.connect(url, tls, &mut dns);
        let connected = match deadline {
            Some(deadline) => timeout_at(deadline, connecting)
                .await
                .unwrap_or_else(|_| Err(DialError::new(Phase::Timeout, TimedOut))),
            None => connecting.await,
        };
        let charge = |looked_up: Option<Duration>| {
            if let (Some(trace), Some(elapsed)) = (trace, looked_up) {
                trace.emit(Event::Dns { elapsed });
            }
        };
        let opened = match connected {
            Ok(Connected::Http1) => return Ok(Dialed::Http1),
            Ok(Connected::Tls(stream)) => {
                charge(dns);
                handshake(stream).await
            }
            Ok(Connected::Clear(stream)) => {
                charge(dns);
                handshake(stream).await
            }
            Err(error) => {
                charge(dns);
                return Err(error);
            }
        };
        opened.map(|opened| Dialed::H2(Box::new(opened)))
    }

    /// The lookup, the connect and, for `tls`, the TLS handshake, with the lookup's time in `dns`.
    async fn connect(
        &self,
        url: &Url,
        tls: bool,
        dns: &mut Option<Duration>,
    ) -> Result<Connected, DialError> {
        let port = url.port().unwrap_or_else(|| default_port(tls));
        let host = url
            .host()
            .ok_or_else(|| DialError::new(Phase::Connect, "the URL has no host"))?;
        let addresses = match &host {
            Host::Ipv4(ip) => vec![SocketAddr::from((*ip, port))],
            Host::Ipv6(ip) => vec![SocketAddr::from((*ip, port))],
            Host::Domain(name) => {
                let start = Instant::now();
                let found = lookup(self.loopback, name).await;
                *dns = Some(start.elapsed());
                let mut found = found.map_err(|e| DialError::new(Phase::Dns, e))?;
                // hyper-util's connector: the URL's port unless it names none and the resolver
                // gave one.
                for address in &mut found {
                    if url.port().is_some() || address.port() == 0 {
                        address.set_port(port);
                    }
                }
                found
            }
        };
        let stream = self.tcp(addresses).await?;
        if !tls {
            return Ok(Connected::Clear(stream));
        }
        let name = match host {
            Host::Domain(name) => {
                ServerName::try_from(name.to_owned()).map_err(|e| DialError::new(Phase::Tls, e))?
            }
            Host::Ipv4(ip) => ServerName::IpAddress(IpAddr::V4(ip).into()),
            Host::Ipv6(ip) => ServerName::IpAddress(IpAddr::V6(ip).into()),
        };
        let stream = self
            .tls
            .connect(name, stream)
            .await
            .map_err(|e| DialError::new(Phase::Tls, e))?;
        if stream.get_ref().1.alpn_protocol() == Some(b"h2".as_slice()) {
            Ok(Connected::Tls(Box::new(stream)))
        } else {
            Ok(Connected::Http1)
        }
    }

    /// Connects to each address in turn until one takes the connection, as hyper-util's
    /// connector does without its fallback to the other address family.
    async fn tcp(&self, addresses: Vec<SocketAddr>) -> Result<TcpStream, DialError> {
        let mut last = None;
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => {
                    // reqwest's TCP_NODELAY and the dialer's keep-alive; hyper-util logs a failure
                    // to set either and goes on.
                    drop(stream.set_nodelay(true));
                    if let Some(idle) = self.keep_alive {
                        let keep_alive = TcpKeepalive::new()
                            .with_time(idle)
                            .with_interval(KEEP_ALIVE_DEFAULT)
                            .with_retries(KEEP_ALIVE_PROBES);
                        drop(SockRef::from(&stream).set_tcp_keepalive(&keep_alive));
                    }
                    return Ok(stream);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(DialError::new(
            Phase::Connect,
            last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address")),
        ))
    }
}

/// A connection up to the point where HTTP/2 begins.
enum Connected {
    Clear(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    Http1,
}

/// HTTP/2's handshake over `io`, with hyper's settings and Go's response header limit, and the
/// connection's future, which ends when the connection does.
async fn handshake<T>(io: T) -> Result<Opened, DialError>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (send, mut connection) = H2Builder::new()
        .initial_max_send_streams(INITIAL_MAX_SEND_STREAMS)
        .initial_window_size(STREAM_WINDOW)
        .initial_connection_window_size(CONNECTION_WINDOW)
        .max_frame_size(MAX_FRAME_SIZE)
        .max_send_buffer_size(MAX_SEND_BUFFER)
        .max_header_list_size(MAX_HEADER_LIST)
        .max_local_error_reset_streams(Some(MAX_LOCAL_ERROR_RESETS))
        .enable_push(false)
        .handshake::<T, Bytes>(io)
        .await
        .map_err(|e| DialError::new(Phase::Handshake, e))?;
    let ping = connection.ping_pong();
    Ok(Opened {
        send,
        connection: Box::pin(async move {
            drop(connection.await);
        }),
        ping,
    })
}
