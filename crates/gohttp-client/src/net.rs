//! Go `net.Dialer.DialContext` for `"tcp"`: resolution, Happy Eyeballs, per-address deadlines,
//! `TCP_NODELAY`, keep-alive settings and Go's dial error values.

use std::{
    borrow::Cow,
    fmt, fs,
    future::{Future, poll_fn},
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::Poll,
    time::{Duration, Instant},
};

use pbs_util::iputil::net::{join_host_port, split_host_port, tcp_addr_string};
use socket2::{SockRef, TcpKeepalive};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, lookup_host},
    time::{sleep, sleep_until, timeout_at},
};

use crate::{
    context::Context,
    error::{ContextError, DnsError, HttpError, OpError, SyscallError},
    httptrace::{DnsDoneInfo, DnsStartInfo, Traces},
};

/// A byte stream the transport can use as a connection.
pub trait Conn: AsyncRead + AsyncWrite + Send + Unpin + fmt::Debug {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin + fmt::Debug> Conn for T {}

/// A dialed connection with the addresses Go's errors print.
#[derive(Debug)]
pub struct DialedConn {
    /// The connection.
    pub conn: Box<dyn Conn>,
    /// Go: `LocalAddr().String()`.
    pub local_addr: Option<String>,
    /// Go: `RemoteAddr().String()`.
    pub remote_addr: Option<String>,
}

/// Go: `net.Dialer` (the fields Prebid sets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dialer {
    /// Go: `Timeout` (zero: none).
    pub timeout: Duration,
    /// Go: `KeepAlive`: `None` disables keep-alives (Go: negative); `Some(ZERO)` is Go's 15s
    /// default.
    pub keep_alive: Option<Duration>,
}

/// Go: `errTimeout` (`i/o timeout`, which `Is(context.DeadlineExceeded)`).
pub const ERR_TIMEOUT: HttpError = HttpError::Timeout(Cow::Borrowed("i/o timeout"));
/// Go: `errCanceled`.
const ERR_CANCELED: HttpError = HttpError::Static("operation was canceled");

/// Go: `mapErr` of a context error.
const fn map_ctx_err(e: ContextError) -> HttpError {
    match e {
        ContextError::Canceled => ERR_CANCELED,
        ContextError::DeadlineExceeded => ERR_TIMEOUT,
    }
}

/// Go's `syscall.Errno` text for an OS error: the C library message with a lowercase first
/// letter, which is how Go's errno tables are written.
#[must_use]
pub fn errno_text(e: &io::Error) -> String {
    let full = e.to_string();
    let msg = full.split(" (os error").next().unwrap_or(&full);
    let mut chars = msg.chars();
    chars.next().map_or_else(String::new, |c| {
        let mut s: String = c.to_lowercase().collect();
        s.push_str(chars.as_str());
        s
    })
}

/// Go: `Errno.Timeout()`.
fn errno_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// Go: the `*net.OpError` a failed `Read` or `Write` on a TCP connection returns.
#[must_use]
pub fn conn_io_error(
    op: &'static str,
    local: Option<&String>,
    remote: Option<&String>,
    e: &io::Error,
) -> HttpError {
    HttpError::Op(Box::new(OpError {
        op,
        net: "tcp",
        source: local.cloned(),
        addr: remote.cloned(),
        err: HttpError::Syscall(Box::new(SyscallError {
            syscall: op,
            errno: errno_text(e),
            timeout: errno_timeout(e),
        })),
    }))
}

fn dial_op_error(addr: Option<String>, err: HttpError) -> HttpError {
    HttpError::Op(Box::new(OpError {
        op: "dial",
        net: "tcp",
        source: None,
        addr,
        err,
    }))
}

/// Go's `DNSError.Server` for a failed lookup: on Linux, Go's resolver reports the first
/// `/etc/resolv.conf` name server; elsewhere the system resolver reports none.
fn resolver_server() -> &'static str {
    static SERVER: OnceLock<String> = OnceLock::new();
    SERVER.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return String::new();
        }
        let conf = fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        conf.lines()
            .filter_map(|l| l.strip_prefix("nameserver"))
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map_or_else(|| "127.0.0.1:53".to_owned(), |ns| join_host_port(ns, "53"))
    })
}

/// Go: `parsePort` and `LookupPort`'s range check.
fn parse_port(port: &str) -> Result<u16, HttpError> {
    if port.is_empty() {
        return Ok(0);
    }
    let digits = port.strip_prefix('+').unwrap_or(port);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HttpError::Dns(Box::new(DnsError {
            err: "unknown port".to_owned(),
            name: format!("tcp/{port}"),
            server: String::new(),
            is_timeout: false,
            is_not_found: true,
        })));
    }
    digits
        .parse::<u16>()
        .map_err(|_overflow| HttpError::Message(format!("address {port}: invalid port")))
}

/// Go: `partialDeadline`.
fn partial_deadline(
    now: Instant,
    deadline: Option<Instant>,
    addrs_remaining: usize,
) -> Result<Option<Instant>, HttpError> {
    let Some(deadline) = deadline else {
        return Ok(None);
    };
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(ERR_TIMEOUT);
    }
    let sane_minimum = Duration::from_secs(2);
    let mut timeout = remaining / u32::try_from(addrs_remaining.max(1)).unwrap_or(u32::MAX);
    if timeout < sane_minimum {
        timeout = remaining.min(sane_minimum);
    }
    Ok(Some(now + timeout))
}

impl Dialer {
    /// Go: `Dialer.deadline`: the earliest of `now + Timeout` and the context deadline.
    fn deadline(&self, ctx: &Context, now: Instant) -> Option<Instant> {
        let own = (!self.timeout.is_zero()).then(|| now + self.timeout);
        match (own, ctx.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Go: `Dialer.DialContext(ctx, "tcp", address)`, calling the DNS and connect trace hooks.
    ///
    /// # Errors
    ///
    /// Go's `*net.OpError` values (`dial tcp 127.0.0.1:1: connect: connection refused`,
    /// `dial tcp 10.0.0.1:80: i/o timeout`, `dial tcp: lookup x.invalid: no such host`).
    pub async fn dial_context(
        &self,
        ctx: &Context,
        address: &str,
        trace: Option<&Arc<Traces>>,
    ) -> Result<DialedConn, HttpError> {
        let deadline = self.deadline(ctx, Instant::now());
        let (host, port) = split_host_port(address)
            .map_err(|e| dial_op_error(None, HttpError::Message(e.to_string())))?;
        let port = parse_port(port).map_err(|e| dial_op_error(None, e))?;
        let addrs = self.resolve(ctx, host, port, deadline, trace).await?;
        // Go: partition by the first address's family; the rest are fallbacks.
        let first_v4 = addrs.first().is_some_and(SocketAddr::is_ipv4);
        let (primaries, fallbacks): (Vec<SocketAddr>, Vec<SocketAddr>) =
            addrs.into_iter().partition(|a| a.is_ipv4() == first_v4);
        let conn = if fallbacks.is_empty() {
            self.dial_serial(ctx, &primaries, deadline, trace).await?
        } else {
            self.dial_parallel(ctx, &primaries, &fallbacks, deadline, trace)
                .await?
        };
        self.configure(&conn);
        let local_addr = conn.local_addr().ok().map(|a| tcp_addr_string(&a));
        let remote_addr = conn.peer_addr().ok().map(|a| tcp_addr_string(&a));
        Ok(DialedConn {
            conn: Box::new(conn),
            local_addr,
            remote_addr,
        })
    }

    /// Go: `newTCPConn`: `TCP_NODELAY` and the keep-alive configuration (idle from
    /// `KeepAlive` or 15s, interval 15s, count 9).
    fn configure(&self, conn: &TcpStream) {
        let _nodelay = conn.set_nodelay(true);
        if let Some(idle) = self.keep_alive {
            let idle = if idle.is_zero() {
                Duration::from_secs(15)
            } else {
                idle
            };
            let ka = TcpKeepalive::new()
                .with_time(idle)
                .with_interval(Duration::from_secs(15))
                .with_retries(9);
            let _keepalive = SockRef::from(conn).set_tcp_keepalive(&ka);
        }
    }

    /// Go: `resolveAddrList` for one host.
    async fn resolve(
        &self,
        ctx: &Context,
        host: &str,
        port: u16,
        deadline: Option<Instant>,
        trace: Option<&Arc<Traces>>,
    ) -> Result<Vec<SocketAddr>, HttpError> {
        if host.is_empty() {
            return Ok(vec![SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port)]);
        }
        let literal = host.split('%').next().unwrap_or(host);
        if let Ok(ip) = literal.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        if let Some(t) = trace {
            t.dns_start(&DnsStartInfo {
                host: host.to_owned(),
            });
        }
        let lookup = lookup_host((host.to_owned(), port));
        // Go: lookupIPAddr stops waiting when the dial context (ctx under the dialer's
        // deadline) is done.
        let given_up = async {
            if let Some(d) = deadline {
                tokio::select! {
                    () = sleep_until(d.into()) => ContextError::DeadlineExceeded,
                    () = ctx.done() => ctx.err().unwrap_or(ContextError::Canceled),
                }
            } else {
                ctx.done().await;
                ctx.err().unwrap_or(ContextError::Canceled)
            }
        };
        let result = tokio::select! {
            biased;
            gave_up = given_up => {
                // Go: newDNSError(mapErr(ctx.Err()), host, ""), which names no server even
                // where resolv.conf has one.
                let e = HttpError::Dns(Box::new(DnsError {
                    err: map_ctx_err(gave_up).to_string(),
                    name: host.to_owned(),
                    server: String::new(),
                    is_timeout: gave_up == ContextError::DeadlineExceeded,
                    is_not_found: false,
                }));
                if let Some(t) = trace {
                    t.dns_done(&DnsDoneInfo {
                        addrs: Vec::new(),
                        err: Some(e.clone()),
                        coalesced: false,
                    });
                }
                return Err(dial_op_error(None, e));
            }
            r = lookup => r.map_err(|_lookup| ()),
        };
        let _ = self;
        let addrs: Vec<SocketAddr> = result.map_or_else(|()| Vec::new(), Iterator::collect);
        let err = addrs.is_empty().then(|| {
            HttpError::Dns(Box::new(DnsError {
                err: "no such host".to_owned(),
                name: host.to_owned(),
                server: resolver_server().to_owned(),
                is_timeout: false,
                is_not_found: true,
            }))
        });
        if let Some(t) = trace {
            t.dns_done(&DnsDoneInfo {
                addrs: addrs.iter().map(|a| a.ip().to_string()).collect(),
                err: err.clone(),
                coalesced: false,
            });
        }
        err.map_or(Ok(addrs), |e| Err(dial_op_error(None, e)))
    }

    /// Go: `dialSerial`: tries each address in turn with its share of the deadline; the first
    /// error is the one returned.
    async fn dial_serial(
        &self,
        ctx: &Context,
        addrs: &[SocketAddr],
        deadline: Option<Instant>,
        trace: Option<&Arc<Traces>>,
    ) -> Result<TcpStream, HttpError> {
        let mut first_err = None;
        for (i, ra) in addrs.iter().enumerate() {
            let ra_str = tcp_addr_string(ra);
            if let Some(e) = ctx.err() {
                return Err(dial_op_error(Some(ra_str), map_ctx_err(e)));
            }
            let partial = match partial_deadline(Instant::now(), deadline, addrs.len() - i) {
                Ok(p) => p,
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(dial_op_error(Some(ra_str), e));
                    }
                    break;
                }
            };
            match self.dial_single(ctx, *ra, &ra_str, partial, trace).await {
                Ok(c) => return Ok(c),
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        Err(first_err.unwrap_or_else(|| dial_op_error(None, HttpError::Static("missing address"))))
    }

    /// Go: `dialParallel`: the fallback addresses start 300ms after the primaries, or as soon
    /// as the primaries fail; the first connection wins, and the primaries' error is reported
    /// when both fail.
    async fn dial_parallel(
        &self,
        ctx: &Context,
        primaries: &[SocketAddr],
        fallbacks: &[SocketAddr],
        deadline: Option<Instant>,
        trace: Option<&Arc<Traces>>,
    ) -> Result<TcpStream, HttpError> {
        type Racer<'a> = Pin<Box<dyn Future<Output = Result<TcpStream, HttpError>> + Send + 'a>>;
        let mut primary: Racer<'_> = Box::pin(self.dial_serial(ctx, primaries, deadline, trace));
        let mut fallback: Option<Racer<'_>> = None;
        let mut timer = Box::pin(sleep(Duration::from_millis(300)));
        let mut primary_err: Option<HttpError> = None;
        let mut fallback_err: Option<HttpError> = None;
        poll_fn(|cx| {
            if primary_err.is_none()
                && let Poll::Ready(r) = primary.as_mut().poll(cx)
            {
                match r {
                    Ok(c) => return Poll::Ready(Ok(c)),
                    Err(e) => primary_err = Some(e),
                }
            }
            if fallback.is_none() && (primary_err.is_some() || timer.as_mut().poll(cx).is_ready()) {
                fallback = Some(Box::pin(self.dial_serial(ctx, fallbacks, deadline, trace)));
            }
            if fallback_err.is_none()
                && let Some(f) = fallback.as_mut()
                && let Poll::Ready(r) = f.as_mut().poll(cx)
            {
                match r {
                    Ok(c) => return Poll::Ready(Ok(c)),
                    Err(e) => fallback_err = Some(e),
                }
            }
            if fallback_err.is_some()
                && let Some(e) = primary_err.take()
            {
                return Poll::Ready(Err(e));
            }
            Poll::Pending
        })
        .await
    }

    /// Go: `dialSingle` for TCP, with the connect trace hooks.
    async fn dial_single(
        &self,
        ctx: &Context,
        ra: SocketAddr,
        ra_str: &str,
        deadline: Option<Instant>,
        trace: Option<&Arc<Traces>>,
    ) -> Result<TcpStream, HttpError> {
        let _ = self;
        if let Some(t) = trace {
            t.connect_start("tcp", ra_str);
        }
        let connect = TcpStream::connect(ra);
        let canceled = ctx.done();
        let result = tokio::select! {
            r = async {
                match deadline {
                    Some(d) => timeout_at(d.into(), connect).await.map_err(|_elapsed| ERR_TIMEOUT),
                    None => Ok(connect.await),
                }
            } => match r {
                Ok(Ok(c)) => Ok(c),
                Ok(Err(e)) => Err(HttpError::Syscall(Box::new(SyscallError {
                    syscall: "connect",
                    errno: errno_text(&e),
                    timeout: errno_timeout(&e),
                }))),
                Err(e) => Err(e),
            },
            () = canceled => Err(map_ctx_err(ctx.err().unwrap_or(ContextError::Canceled))),
        };
        let result = result.map_err(|e| dial_op_error(Some(ra_str.to_owned()), e));
        if let Some(t) = trace {
            t.connect_done("tcp", ra_str, result.as_ref().err());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn partial_deadlines() -> Result<(), String> {
        let now = Instant::now();
        let share = partial_deadline(now, Some(now + Duration::from_secs(10)), 2)
            .map_err(|e| e.to_string())?;
        ensure_eq(&share, &Some(now + Duration::from_secs(5)), "equal share")?;
        let minimum = partial_deadline(now, Some(now + Duration::from_secs(3)), 3)
            .map_err(|e| e.to_string())?;
        ensure_eq(
            &minimum,
            &Some(now + Duration::from_secs(2)),
            "sane minimum",
        )?;
        ensure_eq(
            &partial_deadline(now, Some(now), 1).err(),
            &Some(ERR_TIMEOUT),
            "expired",
        )
    }

    #[tokio::test]
    async fn refused_dial_text() -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        drop(listener);
        let err = Dialer::default()
            .dial_context(&Context::background(), &addr.to_string(), None)
            .await
            .err()
            .map(|e| e.to_string());
        ensure_eq(
            &err,
            &Some(format!("dial tcp {addr}: connect: connection refused")),
            "refused",
        )
    }

    #[tokio::test]
    async fn lookup_gives_up_with_the_dial_context() -> Result<(), String> {
        // Go: lookupIPAddr returns newDNSError(mapErr(ctx.Err()), host, "") once the dial
        // context is done: the error names no server, even where resolv.conf names one.
        let dialer = Dialer::default();
        let expired = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or_else(|| "no instant a second ago".to_owned())?;
        let timed_out = dialer
            .resolve(&Context::background(), "localhost", 80, Some(expired), None)
            .await
            .err()
            .map(|e| (e.to_string(), e.timeout()));
        ensure_eq(
            &timed_out,
            &Some(("dial tcp: lookup localhost: i/o timeout".to_owned(), true)),
            "dial deadline",
        )?;
        let (ctx, cancel) = Context::background().with_cancel();
        cancel.cancel();
        let canceled = dialer
            .resolve(&ctx, "localhost", 80, None, None)
            .await
            .err()
            .map(|e| (e.to_string(), e.timeout()));
        ensure_eq(
            &canceled,
            &Some((
                "dial tcp: lookup localhost: operation was canceled".to_owned(),
                false,
            )),
            "canceled context",
        )
    }
}
