//! Go: `net.Listen("tcp" | "unix", addr)` as Prebid wraps it in `server/listener.go`:
//! `tcpKeepAliveListener` (TCP keep-alive with a 3 minute idle time on every accepted
//! connection), `unixListener`, and `monitorableListener` (connection metrics, and a connection
//! type that hides `CloseWrite`).

use core::{error::Error, fmt, time::Duration};
use std::{
    fs, io,
    net::{self as stdnet, IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs as _},
    path::PathBuf,
    sync::Arc,
};

use pbs_logger::goerr::errno_text;
use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use tokio::{
    net::{TcpListener, UnixListener},
    task,
};

use super::netconn::{ConnAddrs, Network, Stream};

/// Go: `metrics.MetricsEngine`'s connection methods, which Prebid's `monitorableListener`
/// calls.
pub trait ConnMetrics: Send + Sync {
    /// Go: `RecordConnectionAccept(success)`.
    fn record_connection_accept(&self, success: bool);

    /// Go: `RecordConnectionClose(success)`.
    fn record_connection_close(&self, success: bool);
}

/// A listen error with Go's text (`listen tcp :8000: bind: address already in use`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenError(pub String);

impl fmt::Display for ListenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ListenError {}

#[derive(Debug)]
enum Inner {
    Tcp(TcpListener),
    Unix {
        listener: UnixListener,
        path: PathBuf,
    },
}

/// An accepted connection.
#[derive(Debug)]
pub(super) struct Accepted {
    pub(super) stream: Stream,
    pub(super) addrs: Arc<ConnAddrs>,
}

/// An accept error: Go's text and whether `net.Error.Temporary()` is true.
#[derive(Debug)]
pub(super) struct AcceptError {
    pub(super) text: String,
    pub(super) temporary: bool,
}

/// A listening socket as Prebid's `newTCPListener` / `newUnixListener` return it.
pub struct Listener {
    inner: Inner,
    network: Network,
    addr: String,
    metrics: Option<Arc<dyn ConnMetrics>>,
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("addr", &self.addr)
            .field("metrics", &self.metrics.is_some())
            .finish_non_exhaustive()
    }
}

/// Go: `TCPAddr.String()` (IPv4-mapped addresses print as IPv4).
pub(super) fn tcp_addr_string(addr: SocketAddr) -> String {
    match addr.ip().to_canonical() {
        IpAddr::V4(ip) => format!("{ip}:{}", addr.port()),
        IpAddr::V6(ip) => format!("[{ip}]:{}", addr.port()),
    }
}

/// Go: `maxListenerBacklog`.
fn max_listener_backlog() -> i32 {
    const SOMAXCONN: i32 = 128;
    fs::read_to_string("/proc/sys/net/core/somaxconn")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|f| f.parse::<i64>().ok())
        })
        .filter(|&n| n > 0)
        .map_or(SOMAXCONN, |n| {
            i32::try_from(n.min(i64::from(u32::MAX))).unwrap_or(i32::MAX)
        })
}

/// Go: `wrapSyscallError(op, err)` text.
fn syscall_text(op: &str, err: &io::Error) -> String {
    format!("{op}: {}", errno_text(err))
}

impl Listener {
    /// Go: Prebid's `newTCPListener(address, nil)`: `net.Listen("tcp", address)` (a dual-stack
    /// socket for a wildcard host, `SO_REUSEADDR`, the system backlog) whose accepted
    /// connections have `TCP_NODELAY`, and TCP keep-alive probes after 3 minutes idle, every 15
    /// seconds, 9 times. Must be called inside a tokio runtime.
    ///
    /// # Errors
    ///
    /// The listen error with Go's text.
    pub async fn bind_tcp(address: &str) -> Result<Self, ListenError> {
        let (host, port) = split_host_port(address).ok_or_else(|| {
            ListenError(format!(
                "listen tcp: address {address}: missing port in address"
            ))
        })?;
        let port: u16 = port
            .parse()
            .map_err(|_bad| ListenError(format!("listen tcp: address {address}: invalid port")))?;
        let ip = resolve_listen_host(host, port).await.map_err(ListenError)?;
        let wildcard = ip.is_none_or(|ip| ip.is_unspecified());
        let (domain, bind_addr) = match ip {
            Some(IpAddr::V4(v4)) if !wildcard => {
                (Domain::IPV4, SocketAddr::new(IpAddr::V4(v4), port))
            }
            Some(IpAddr::V6(v6)) if !wildcard => {
                (Domain::IPV6, SocketAddr::new(IpAddr::V6(v6), port))
            }
            Some(_) | None => (
                Domain::IPV6,
                SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
            ),
        };
        let shown = ip.map_or_else(
            || format!(":{port}"),
            |ip| tcp_addr_string(SocketAddr::new(ip, port)),
        );
        let listen_err = |op: &str, e: &io::Error| {
            ListenError(format!("listen tcp {shown}: {}", syscall_text(op, e)))
        };
        let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))
            .map_err(|e| listen_err("socket", &e))?;
        if domain == Domain::IPV6 {
            socket
                .set_only_v6(false)
                .map_err(|e| listen_err("setsockopt", &e))?;
        }
        socket
            .set_reuse_address(true)
            .map_err(|e| listen_err("setsockopt", &e))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| listen_err("setsockopt", &e))?;
        socket
            .bind(&bind_addr.into())
            .map_err(|e| listen_err("bind", &e))?;
        socket
            .listen(max_listener_backlog())
            .map_err(|e| listen_err("listen", &e))?;
        let std_listener: stdnet::TcpListener = socket.into();
        let listener = TcpListener::from_std(std_listener).map_err(|e| listen_err("listen", &e))?;
        let addr = listener
            .local_addr()
            .map_or_else(|_| shown.clone(), tcp_addr_string);
        Ok(Self {
            inner: Inner::Tcp(listener),
            network: Network::Tcp,
            addr,
            metrics: None,
        })
    }

    /// Go: Prebid's `newUnixListener(address, nil)`: `net.Listen("unix", path)`; the socket file
    /// is removed when the listener closes. Must be called inside a tokio runtime.
    ///
    /// # Errors
    ///
    /// The listen error with Go's text.
    pub fn bind_unix(path: &str) -> Result<Self, ListenError> {
        let listener = UnixListener::bind(path).map_err(|e| {
            ListenError(format!("listen unix {path}: {}", syscall_text("bind", &e)))
        })?;
        Ok(Self {
            inner: Inner::Unix {
                listener,
                path: PathBuf::from(path),
            },
            network: Network::Unix,
            addr: path.to_owned(),
            metrics: None,
        })
    }

    /// Go: Prebid's `monitorableListener`: records accepts and closes, logs accept errors and
    /// close failures, and (as its connection type does) hides `CloseWrite` from the server.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<dyn ConnMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Go: `ln.Addr().String()`.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub(super) fn metrics(&self) -> Option<Arc<dyn ConnMetrics>> {
        self.metrics.clone()
    }

    /// Go: `ln.Accept()` through Prebid's wrappers.
    pub(super) async fn accept(&self) -> Result<Accepted, AcceptError> {
        let result = self.accept_raw().await;
        if let Some(m) = &self.metrics {
            match &result {
                Ok(_) => m.record_connection_accept(true),
                Err(e) => {
                    pbs_logger::errorf!("Error accepting connection: %v", e.text.as_str());
                    m.record_connection_accept(false);
                }
            }
        }
        result
    }

    /// Go: the `Accept` that fails because `Shutdown` closed the listener under it (or before
    /// it). Its error, `accept tcp [::]:8000: use of closed network connection`, is logged and
    /// recorded as a failed accept by Prebid's `monitorableListener`; `Serve` then returns
    /// `http.ErrServerClosed`.
    pub(super) fn accept_closed(&self) {
        if let Some(m) = &self.metrics {
            let text = format!(
                "accept {} {}: use of closed network connection",
                self.network_name(),
                self.addr
            );
            pbs_logger::errorf!("Error accepting connection: %v", text.as_str());
            m.record_connection_accept(false);
        }
    }

    /// Go: `ln.Addr().Network()`.
    const fn network_name(&self) -> &'static str {
        match self.network {
            Network::Tcp => "tcp",
            Network::Unix => "unix",
        }
    }

    async fn accept_raw(&self) -> Result<Accepted, AcceptError> {
        let accept_err = |e: &io::Error| AcceptError {
            text: format!(
                "accept {} {}: {}",
                self.network_name(),
                self.addr,
                syscall_text("accept", e)
            ),
            temporary: is_temporary(e),
        };
        match &self.inner {
            Inner::Tcp(l) => {
                let (stream, remote) = l.accept().await.map_err(|e| accept_err(&e))?;
                let local = stream
                    .local_addr()
                    .map_or_else(|_e| String::new(), tcp_addr_string);
                let sock = SockRef::from(&stream);
                let _nodelay = sock.set_tcp_nodelay(true);
                let keepalive = TcpKeepalive::new()
                    .with_time(Duration::from_secs(180))
                    .with_interval(Duration::from_secs(15))
                    .with_retries(9);
                let _keepalive = sock.set_tcp_keepalive(&keepalive);
                Ok(Accepted {
                    stream: Stream::Tcp(stream),
                    addrs: Arc::new(ConnAddrs {
                        network: Network::Tcp,
                        local,
                        remote: tcp_addr_string(remote),
                    }),
                })
            }
            Inner::Unix { listener, path } => {
                let (stream, _remote) = listener.accept().await.map_err(|e| accept_err(&e))?;
                Ok(Accepted {
                    stream: Stream::Unix(stream),
                    addrs: Arc::new(ConnAddrs {
                        network: Network::Unix,
                        local: path.to_string_lossy().into_owned(),
                        remote: String::new(),
                    }),
                })
            }
        }
    }

    /// Go: `ln.Close()` (a Unix listener removes its socket file).
    pub(super) fn close(&self) {
        if let Inner::Unix { path, .. } = &self.inner {
            let _removed = fs::remove_file(path);
        }
    }
}

/// Go: `syscall.Errno.Temporary()` for accept errors.
fn is_temporary(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
    ) || matches!(e.raw_os_error(), Some(23 | 24))
}

/// Go: `net.SplitHostPort` for listen addresses.
fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':')?;
        return Some((host, port));
    }
    let (host, port) = hostport.rsplit_once(':')?;
    if host.contains(':') {
        return None;
    }
    Some((host, port))
}

/// Resolves the host of a listen address: `None` for the wildcard.
async fn resolve_listen_host(host: &str, port: u16) -> Result<Option<IpAddr>, String> {
    if host.is_empty() {
        return Ok(None);
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(Some(ip));
    }
    let query = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> =
        task::spawn_blocking(move || query.to_socket_addrs().map(Iterator::collect))
            .await
            .map_err(|e| format!("listen tcp: lookup {host}: {e}"))?
            .map_err(|e| format!("listen tcp: lookup {host}: {}", errno_text(&e)))?;
    // Go: `addrs.first(isIPv4)`.
    let first = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .map(SocketAddr::ip);
    first
        .map(Some)
        .ok_or_else(|| format!("listen tcp: lookup {host}: no such host"))
}
