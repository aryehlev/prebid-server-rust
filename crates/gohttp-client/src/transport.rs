//! Go `net/http.Transport` for HTTP/1.1: connection pooling, dialing (direct, HTTP proxy and
//! `CONNECT` tunnels), TLS, the request writer, response reading, retries and transparent gzip.
//!
//! Go runs a read loop and a write loop goroutine per connection. Here one task drives a
//! request: the round-trip future writes the request and reads the response head concurrently,
//! and the response body keeps driving any unfinished write. Idle connections are checked for a
//! server close when they are taken from the pool, which is when Go's read loop would already
//! have removed them.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, VecDeque},
    fmt,
    future::{Future, poll_fn},
    io::{self, Cursor},
    iter, mem,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak},
    task::{Context as TaskContext, Poll, Waker, ready},
    time::{Duration, Instant},
};

use bytes::Bytes;
use pbs_gocompat::{base64, http::Header, strconv::quote, url::Url};
use pbs_util::iputil::net::{join_host_port, split_host_port};
use tokio::{
    io::{AsyncRead as _, ReadBuf, empty},
    runtime::Handle,
    sync::{Notify, oneshot},
    time::{Sleep, sleep, sleep_until, timeout},
};

use crate::{
    bufio::{BufReader, DEFAULT_BUF_SIZE, GoRead},
    chunked::{ChunkedReader, chunk_header},
    context::{CancelCauseFunc, Context},
    error::{HttpError, OpError},
    gzip::{BodySource, GzipReader},
    httpguts::{
        has_port, header_values_contains_token, idna_to_ascii, valid_header_field_name,
        valid_header_field_value, valid_method,
    },
    httpproxy::proxy_from_environment,
    httptrace::{ConnectionState, GotConnInfo, Traces, WroteRequestInfo, context_client_trace},
    net::{Conn, DialedConn, Dialer, conn_io_error},
    request::{Body, BodyReader, Request},
    response::{Response, ResponseBody, ResponseHead, ResponseHeadReader},
    textproto::MimeHeaderReader,
    tls::{TlsClient, TlsConfig, handshake},
    transfer::{
        Framing, ProbeResult, ResponseTransfer, TransferWriter, read_transfer,
        request_method_usually_lacks_body,
    },
};

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Go: `http.RoundTripper`.
pub trait RoundTripper: Send + Sync + fmt::Debug {
    /// Go: `RoundTrip(req)`: one HTTP transaction. The response carries the request back in
    /// [`Response::request`].
    fn round_trip(&self, req: Request) -> BoxFuture<'_, Result<Response, HttpError>>;
}

/// Go: `Transport.Proxy`.
pub type ProxyFunc = Arc<dyn Fn(&Request) -> Result<Option<Url>, HttpError> + Send + Sync>;

/// Go: `Transport.DialContext`: dials `addr` (`host:port`) over TCP.
pub type DialContextFunc =
    Arc<dyn Fn(Context, String) -> BoxFuture<'static, Result<DialedConn, HttpError>> + Send + Sync>;

/// Go: the `http.Transport` fields Prebid configures. [`Default`] is Go's zero `Transport{}`.
#[derive(Clone, Default)]
pub struct TransportOptions {
    /// Go: `Proxy` (`None`: no proxy; see [`crate::httpproxy::proxy_from_environment`]).
    pub proxy: Option<ProxyFunc>,
    /// Go: `DialContext` (`None`: [`TransportOptions::dialer`]).
    pub dial_context: Option<DialContextFunc>,
    /// The `net.Dialer` used when `dial_context` is `None` (Go: `zeroDialer`).
    pub dialer: Dialer,
    /// Go: `TLSClientConfig`.
    pub tls_client_config: Option<TlsConfig>,
    /// Go: `TLSHandshakeTimeout` (zero: none).
    pub tls_handshake_timeout: Duration,
    /// Go: `DisableKeepAlives`.
    pub disable_keep_alives: bool,
    /// Go: `DisableCompression`.
    pub disable_compression: bool,
    /// Go: `MaxIdleConns` (zero: no limit).
    pub max_idle_conns: i64,
    /// Go: `MaxIdleConnsPerHost` (zero: 2; negative: keep no idle connections).
    pub max_idle_conns_per_host: i64,
    /// Go: `MaxConnsPerHost` (zero or negative: no limit).
    pub max_conns_per_host: i64,
    /// Go: `IdleConnTimeout` (zero: none).
    pub idle_conn_timeout: Duration,
    /// Go: `ResponseHeaderTimeout` (zero: none).
    pub response_header_timeout: Duration,
    /// Go: `ExpectContinueTimeout` (zero: send the body at once).
    pub expect_continue_timeout: Duration,
    /// Go: `MaxResponseHeaderBytes` (zero: 10 MiB).
    pub max_response_header_bytes: i64,
    /// Go: `ProxyConnectHeader`.
    pub proxy_connect_header: Header,
}

impl fmt::Debug for TransportOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportOptions")
            .field("proxy", &self.proxy.is_some())
            .field("dial_context", &self.dial_context.is_some())
            .field("dialer", &self.dialer)
            .field("tls_client_config", &self.tls_client_config)
            .field("tls_handshake_timeout", &self.tls_handshake_timeout)
            .field("disable_keep_alives", &self.disable_keep_alives)
            .field("disable_compression", &self.disable_compression)
            .field("max_idle_conns", &self.max_idle_conns)
            .field("max_idle_conns_per_host", &self.max_idle_conns_per_host)
            .field("max_conns_per_host", &self.max_conns_per_host)
            .field("idle_conn_timeout", &self.idle_conn_timeout)
            .finish_non_exhaustive()
    }
}

impl TransportOptions {
    /// Go: `http.DefaultTransport`'s settings (HTTP/1.1 only: see the crate docs).
    #[must_use]
    pub fn default_transport() -> Self {
        Self {
            proxy: Some(Arc::new(proxy_from_environment)),
            dialer: Dialer {
                timeout: Duration::from_secs(30),
                keep_alive: Some(Duration::from_secs(30)),
            },
            max_idle_conns: 100,
            idle_conn_timeout: Duration::from_secs(90),
            tls_handshake_timeout: Duration::from_secs(10),
            expect_continue_timeout: Duration::from_secs(1),
            ..Self::default()
        }
    }

    /// Go: `maxIdleConnsPerHost()` (a negative value never keeps a connection).
    fn max_idle_per_host(&self) -> usize {
        match self.max_idle_conns_per_host {
            0 => 2,
            n => usize::try_from(n).unwrap_or(0),
        }
    }

    /// Go: `maxHeaderResponseSize()`.
    const fn max_header_bytes(&self) -> i64 {
        if self.max_response_header_bytes != 0 {
            self.max_response_header_bytes
        } else {
            10 << 20
        }
    }
}

const ERR_KEEP_ALIVES_DISABLED: HttpError =
    HttpError::Static("http: putIdleConn: keep alives disabled");
const ERR_CONN_BROKEN: HttpError =
    HttpError::Static("http: putIdleConn: connection is in bad state");
const ERR_CLOSE_IDLE: HttpError =
    HttpError::Static("http: putIdleConn: CloseIdleConnections was called");
const ERR_TOO_MANY_IDLE_HOST: HttpError =
    HttpError::Static("http: putIdleConn: too many idle connections for host");
/// Go: `errRequestDone`.
const ERR_REQUEST_DONE: HttpError = HttpError::Static("net/http: request completed");
/// Go: `errReadOnClosedResBody`.
pub const ERR_READ_ON_CLOSED_RES_BODY: HttpError =
    HttpError::Static("http: read on closed response body");
/// Go: `errTimeout` of `net/http` (the response header timeout).
const ERR_RESPONSE_HEADER_TIMEOUT: HttpError =
    HttpError::Timeout(Cow::Borrowed("net/http: timeout awaiting response headers"));
/// Go: `errServerClosedIdle`.
const ERR_SERVER_CLOSED_IDLE: HttpError = HttpError::Static("http: server closed idle connection");
/// Go: `errCannotRewind`.
const ERR_CANNOT_REWIND: HttpError =
    HttpError::Static("net/http: cannot rewind body after connection loss");
/// Go: `errTrailerEOF`.
const ERR_TRAILER_EOF: HttpError = HttpError::Static("http: unexpected EOF reading trailer");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Go: `connectMethodKey`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnKey {
    proxy: String,
    scheme: String,
    addr: String,
}

/// Go: `connectMethod`.
#[derive(Debug, Clone)]
struct ConnectMethod {
    proxy_url: Option<Url>,
    target_scheme: String,
    target_addr: String,
}

/// Go: `schemePort` / `portMap`.
fn scheme_port(scheme: &str) -> &'static str {
    match scheme {
        "http" => "80",
        "https" => "443",
        "socks5" | "socks5h" => "1080",
        _ => "",
    }
}

/// Go: `canonicalAddr`: `host:port` with the IDNA host and the scheme's default port.
fn canonical_addr(url: &Url) -> String {
    let port = match url.port() {
        "" => scheme_port(&url.scheme),
        p => p,
    };
    let host = url.hostname();
    let host = idna_to_ascii(host).unwrap_or_else(|_err| host.to_owned());
    join_host_port(&host, port)
}

/// Go: `basicAuth`.
fn basic_auth(username: &str, password: &str) -> String {
    base64::STD_ENCODING.encode_to_string(format!("{username}:{password}").as_bytes())
}

impl ConnectMethod {
    fn key(&self) -> ConnKey {
        let mut addr = self.target_addr.clone();
        let mut proxy = String::new();
        if let Some(p) = &self.proxy_url {
            proxy = p.to_string();
            if (p.scheme == "http" || p.scheme == "https") && self.target_scheme == "http" {
                addr.clear();
            }
        }
        ConnKey {
            proxy,
            scheme: self.target_scheme.clone(),
            addr,
        }
    }

    fn scheme(&self) -> &str {
        self.proxy_url
            .as_ref()
            .map_or(self.target_scheme.as_str(), |p| p.scheme.as_str())
    }

    fn addr(&self) -> String {
        self.proxy_url
            .as_ref()
            .map_or_else(|| self.target_addr.clone(), canonical_addr)
    }

    fn tls_host(&self) -> &str {
        let h = self.target_addr.as_str();
        if has_port(h) {
            h.rfind(':').and_then(|i| h.get(..i)).unwrap_or(h)
        } else {
            h
        }
    }

    fn proxy_auth(&self) -> Option<String> {
        let user = self.proxy_url.as_ref()?.user.as_ref()?;
        Some(format!(
            "Basic {}",
            basic_auth(user.username(), user.password().unwrap_or(""))
        ))
    }
}

/// A slot that a dial or a returned idle connection delivers into (Go: `wantConn`).
struct WantConn {
    key: ConnKey,
    cm: ConnectMethod,
    /// Go: the dial context (the request's values without its cancellation).
    ctx: Context,
    tx: Mutex<Option<oneshot::Sender<Delivered>>>,
}

type Delivered = Result<Box<PersistConn>, HttpError>;

impl WantConn {
    /// Go: `waiting()`.
    fn waiting(&self) -> bool {
        lock(&self.tx).as_ref().is_some_and(|tx| !tx.is_closed())
    }

    /// Go: `tryDeliver`: gives the result back when nobody waits any more.
    fn try_deliver(&self, d: Delivered) -> Result<(), Delivered> {
        let tx = lock(&self.tx).take();
        match tx {
            Some(tx) => tx.send(d),
            None => Err(d),
        }
    }
}

/// The idle pool and dial accounting (Go: `idleConn`, `idleLRU`, `idleConnWait`,
/// `connsPerHost`, `connsPerHostWait`).
#[derive(Default)]
struct Pool {
    idle: HashMap<ConnKey, Vec<PersistConn>>,
    /// Idle connections by insertion order: the first is the oldest.
    lru: BTreeMap<u64, ConnKey>,
    idle_wait: HashMap<ConnKey, VecDeque<Arc<WantConn>>>,
    conns_per_host: HashMap<ConnKey, usize>,
    conns_per_host_wait: HashMap<ConnKey, VecDeque<Arc<WantConn>>>,
    close_idle: bool,
    seq: u64,
    reaper_started: bool,
}

struct Inner {
    opts: TransportOptions,
    tls: OnceLock<Result<TlsClient, HttpError>>,
    pool: Mutex<Pool>,
    reaper: Arc<Notify>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.reaper.notify_one();
    }
}

/// Go: `*http.Transport`. Cloning shares the connection pool.
#[derive(Clone)]
pub struct Transport {
    inner: Arc<Inner>,
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("opts", &self.inner.opts)
            .finish_non_exhaustive()
    }
}

/// Decrements the per-host connection count when a connection closes (Go: `decConnsPerHost`
/// from `closeLocked`).
struct ConnsGuard {
    inner: Weak<Inner>,
    key: ConnKey,
}

impl Drop for ConnsGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            dec_conns_per_host(&inner, &self.key);
        }
    }
}

/// Go: `persistConn`.
pub(crate) struct PersistConn {
    conn: Box<dyn Conn>,
    local_addr: Option<String>,
    remote_addr: Option<String>,
    br: BufReader,
    /// Go: `readLimit`.
    read_limit: i64,
    /// Go: `sawEOF`.
    saw_eof: bool,
    /// Go: `nwrite`.
    nwrite: u64,
    /// Go: `reused`.
    reused: bool,
    /// Go: `closed != nil` (broken).
    broken: bool,
    /// Go: `canceledErr`.
    canceled: Option<HttpError>,
    /// Go: `isProxy` and its `mutateHeaderFunc`.
    forward_proxy: Option<ForwardProxy>,
    key: ConnKey,
    idle_at: Option<Instant>,
    idle_seq: u64,
    tls_state: Option<ConnectionState>,
    max_header_bytes: i64,
    _guard: Option<ConnsGuard>,
}

impl fmt::Debug for PersistConn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistConn")
            .field("remote_addr", &self.remote_addr)
            .field("reused", &self.reused)
            .finish_non_exhaustive()
    }
}

/// A connection to an HTTP proxy that takes requests in absolute form (Go: `isProxy`).
struct ForwardProxy {
    /// Go: the `mutateHeaderFunc` that sets `Proxy-Authorization`.
    authorization: Option<String>,
}

/// The reading side of a connection as Go's `persistConn.Read` sees it.
struct ConnRead<'a> {
    conn: &'a mut Box<dyn Conn>,
    local: Option<&'a String>,
    remote: Option<&'a String>,
    read_limit: &'a mut i64,
    saw_eof: &'a mut bool,
    max_header_bytes: i64,
    closed: bool,
    /// Go's `CONNECT` reader: an `io.LimitedReader`, which ends with `EOF`.
    limit_eof: bool,
}

impl GoRead for ConnRead<'_> {
    fn poll_go_read(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, HttpError>> {
        if self.closed {
            return Poll::Ready(Err(closed_conn_error("read", self.local, self.remote)));
        }
        if *self.read_limit <= 0 {
            if self.limit_eof {
                return Poll::Ready(Err(HttpError::Eof));
            }
            return Poll::Ready(Err(HttpError::Message(format!(
                "read limit of {} bytes exhausted",
                self.max_header_bytes
            ))));
        }
        let cap = usize::try_from(*self.read_limit)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let Some(dst) = buf.get_mut(..cap) else {
            return Poll::Ready(Ok(0));
        };
        let mut rb = ReadBuf::new(dst);
        match ready!(Pin::new(&mut **self.conn).poll_read(cx, &mut rb)) {
            Ok(()) => {
                let n = rb.filled().len();
                if n == 0 {
                    *self.saw_eof = true;
                    return Poll::Ready(Err(HttpError::Eof));
                }
                *self.read_limit -= i64::try_from(n).unwrap_or(i64::MAX);
                Poll::Ready(Ok(n))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // A TLS peer that closed without close_notify at a record boundary.
                *self.saw_eof = true;
                Poll::Ready(Err(HttpError::Eof))
            }
            Err(e) => Poll::Ready(Err(conn_io_error("read", self.local, self.remote, &e))),
        }
    }
}

/// Go: the error a read or write on a connection closed by this side returns.
fn closed_conn_error(
    op: &'static str,
    local: Option<&String>,
    remote: Option<&String>,
) -> HttpError {
    HttpError::Op(Box::new(OpError {
        op,
        net: "tcp",
        source: local.cloned(),
        addr: remote.cloned(),
        err: HttpError::Static("use of closed network connection"),
    }))
}

impl PersistConn {
    fn reader(&mut self) -> (&mut BufReader, ConnRead<'_>) {
        let closed = self.broken && self.canceled.is_some();
        (
            &mut self.br,
            ConnRead {
                conn: &mut self.conn,
                local: self.local_addr.as_ref(),
                remote: self.remote_addr.as_ref(),
                read_limit: &mut self.read_limit,
                saw_eof: &mut self.saw_eof,
                max_header_bytes: self.max_header_bytes,
                closed,
                limit_eof: false,
            },
        )
    }

    /// Writes `buf` to the connection, counting bytes in `nwrite`.
    fn poll_write(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, HttpError>> {
        if self.broken {
            return Poll::Ready(Err(closed_conn_error(
                "write",
                self.local_addr.as_ref(),
                self.remote_addr.as_ref(),
            )));
        }
        match ready!(Pin::new(&mut *self.conn).poll_write(cx, buf)) {
            Ok(n) => {
                self.nwrite += n as u64;
                Poll::Ready(Ok(n))
            }
            Err(e) => Poll::Ready(Err(conn_io_error(
                "write",
                self.local_addr.as_ref(),
                self.remote_addr.as_ref(),
                &e,
            ))),
        }
    }

    /// Go: `cancelRequest(err)`: records the cause and closes the connection (buffered bytes
    /// stay readable).
    fn cancel_request(&mut self, cause: HttpError) {
        if self.canceled.is_none() {
            self.canceled = Some(cause);
        }
        self.broken = true;
    }

    /// Whether the server closed or wrote to the idle connection (Go's read loop would have
    /// closed it): one non-blocking read.
    fn idle_conn_alive(&mut self) -> bool {
        if self.broken || self.br.buffered() > 0 {
            return false;
        }
        let mut cx = TaskContext::from_waker(Waker::noop());
        let mut probe = [0_u8; 1];
        let mut rb = ReadBuf::new(&mut probe);
        matches!(
            Pin::new(&mut *self.conn).poll_read(&mut cx, &mut rb),
            Poll::Pending
        )
    }
}

/// Why a round trip on one connection failed, for Go's retry rules.
enum RtError {
    /// Go: `nothingWrittenError`.
    NothingWritten(HttpError),
    /// Go: `transportReadFromServerError`.
    ReadFromServer(HttpError),
    /// Go: any other error, already mapped by `mapRoundTripError`.
    Other(HttpError),
}

impl RtError {
    fn into_inner(self) -> HttpError {
        match self {
            Self::NothingWritten(e) | Self::ReadFromServer(e) | Self::Other(e) => e,
        }
    }
}

/// Go: `shouldRetryRequest`.
fn should_retry_request(reused: bool, req: &Request, err: &RtError) -> bool {
    if !reused {
        return false;
    }
    match err {
        RtError::NothingWritten(_) => req.outgoing_length() == 0 || req.get_body.is_some(),
        RtError::ReadFromServer(_) => req.is_replayable(),
        RtError::Other(e) => req.is_replayable() && *e == ERR_SERVER_CLOSED_IDLE,
    }
}

/// Go: `rewindBody`.
fn rewind_body(mut req: Request) -> Result<Request, HttpError> {
    if matches!(req.body, Body::Nil | Body::NoBody) {
        return Ok(req);
    }
    let Some(get_body) = req.get_body.clone() else {
        return Err(ERR_CANNOT_REWIND);
    };
    req.body = get_body.call()?;
    Ok(req)
}

fn dec_conns_per_host(inner: &Arc<Inner>, key: &ConnKey) {
    if inner.opts.max_conns_per_host <= 0 {
        return;
    }
    let mut pool = lock(&inner.pool);
    // Go: hand the count to the first dial still waiting, dropping those that gave up.
    let (next, emptied) = pool
        .conns_per_host_wait
        .get_mut(key)
        .map_or((None, false), |q| {
            let next = iter::from_fn(|| q.pop_front()).find(|w| w.waiting());
            (next, q.is_empty())
        });
    if emptied {
        let _removed = pool.conns_per_host_wait.remove(key);
    }
    if let Some(w) = next {
        // The slot passes to the waiting dial.
        drop(pool);
        start_dial(inner, w);
        return;
    }
    let remaining = pool
        .conns_per_host
        .get(key)
        .copied()
        .unwrap_or(0)
        .saturating_sub(1);
    if remaining == 0 {
        let _removed = pool.conns_per_host.remove(key);
    } else {
        let _previous = pool.conns_per_host.insert(key.clone(), remaining);
    }
}

fn start_dial(inner: &Arc<Inner>, w: Arc<WantConn>) {
    let Ok(handle) = Handle::try_current() else {
        return;
    };
    let inner = Arc::clone(inner);
    let _task = handle.spawn(async move { dial_conn_for(&inner, &w).await });
}

/// Go: `dialConnFor`.
async fn dial_conn_for(inner: &Arc<Inner>, w: &Arc<WantConn>) {
    if !w.waiting() {
        dec_conns_per_host(inner, &w.key);
        return;
    }
    match dial_conn(inner, &w.ctx, &w.cm).await {
        Ok(pc) => {
            if let Err(Ok(pc)) = w.try_deliver(Ok(pc)) {
                put_or_close_idle_conn(inner, pc);
            }
        }
        Err(e) => {
            let _undelivered = w.try_deliver(Err(e));
            dec_conns_per_host(inner, &w.key);
        }
    }
}

/// Go: `putOrCloseIdleConn`.
fn put_or_close_idle_conn(inner: &Arc<Inner>, pc: Box<PersistConn>) {
    let _closed = try_put_idle_conn(inner, pc);
}

/// What [`put_locked`] leaves to drop once the pool lock is released (closing a connection
/// takes the lock again to update the per-host count).
type Evicted = Option<PersistConn>;

/// Go: `tryPutIdleConn` under `idleMu`.
fn put_locked(
    pool: &mut Pool,
    opts: &TransportOptions,
    mut pc: Box<PersistConn>,
) -> Result<Evicted, (HttpError, Box<PersistConn>)> {
    let key = pc.key.clone();
    if let Some(q) = pool.idle_wait.get_mut(&key) {
        // Go: hand the connection to a waiting request (with no idle time).
        pc.idle_at = None;
        let mut left = Some(pc);
        while let Some(c) = left.take() {
            let Some(w) = q.pop_front() else {
                left = Some(c);
                break;
            };
            match w.try_deliver(Ok(c)) {
                Ok(()) | Err(Err(_)) => {}
                Err(Ok(back)) => left = Some(back),
            }
        }
        if q.is_empty() {
            let _removed = pool.idle_wait.remove(&key);
        }
        match left {
            None => return Ok(None),
            Some(back) => pc = back,
        }
    }
    if pool.close_idle {
        return Err((ERR_CLOSE_IDLE, pc));
    }
    if pool.idle.get(&key).map_or(0, Vec::len) >= opts.max_idle_per_host() {
        return Err((ERR_TOO_MANY_IDLE_HOST, pc));
    }
    pool.seq += 1;
    let seq = pool.seq;
    pc.idle_at = Some(Instant::now());
    pc.idle_seq = seq;
    pool.idle.entry(key.clone()).or_default().push(*pc);
    let _previous = pool.lru.insert(seq, key);
    let idle = i64::try_from(pool.lru.len()).unwrap_or(i64::MAX);
    if opts.max_idle_conns == 0 || idle <= opts.max_idle_conns {
        return Ok(None);
    }
    // Go: close the least recently used idle connection (errTooManyIdle).
    Ok(pool
        .lru
        .pop_first()
        .and_then(|(oldest, okey)| remove_idle_seq(pool, &okey, oldest)))
}

/// Go: `tryPutIdleConn`. On failure the connection is closed (dropped) and the error returned.
fn try_put_idle_conn(inner: &Arc<Inner>, mut pc: Box<PersistConn>) -> Result<(), HttpError> {
    let opts = &inner.opts;
    if opts.disable_keep_alives || opts.max_idle_conns_per_host < 0 {
        return Err(ERR_KEEP_ALIVES_DISABLED);
    }
    if pc.broken {
        return Err(ERR_CONN_BROKEN);
    }
    pc.reused = true;
    let mut pool = lock(&inner.pool);
    let result = put_locked(&mut pool, opts, pc);
    let start_reaper_now = result.is_ok()
        && !opts.idle_conn_timeout.is_zero()
        && !mem::replace(&mut pool.reaper_started, true);
    drop(pool);
    match result {
        Ok(evicted) => {
            drop(evicted);
            if start_reaper_now {
                start_reaper(inner);
            }
            inner.reaper.notify_one();
            Ok(())
        }
        Err((e, closed)) => {
            drop(closed);
            Err(e)
        }
    }
}

/// Go: `queueForIdleConn` under `idleMu`: takes the most recently used live idle connection,
/// or queues `w` for the next one. Dead connections are moved to `dead`.
fn queue_idle_locked(
    pool: &mut Pool,
    opts: &TransportOptions,
    w: &Arc<WantConn>,
    dead: &mut Vec<PersistConn>,
) -> bool {
    // Go: stop closing connections that become idle (undo CloseIdleConnections).
    pool.close_idle = false;
    let old_time = if opts.idle_conn_timeout.is_zero() {
        None
    } else {
        Instant::now().checked_sub(opts.idle_conn_timeout)
    };
    let delivered = pool
        .idle
        .get_mut(&w.key)
        .and_then(|list| offer_idle(list, &mut pool.lru, old_time, w, dead));
    if pool.idle.get(&w.key).is_some_and(Vec::is_empty) {
        let _removed = pool.idle.remove(&w.key);
    }
    if let Some(d) = delivered {
        return d;
    }
    let q = pool.idle_wait.entry(w.key.clone()).or_default();
    while q.front().is_some_and(|f| !f.waiting()) {
        let _dropped = q.pop_front();
    }
    q.push_back(Arc::clone(w));
    false
}

/// Go: `queueForIdleConn`'s scan of one host's idle list, most recent first: `Some` once a
/// live connection was offered to `w` (`true` when taken). Expired or closed connections go
/// to `dead`.
fn offer_idle(
    list: &mut Vec<PersistConn>,
    lru: &mut BTreeMap<u64, ConnKey>,
    old_time: Option<Instant>,
    w: &WantConn,
    dead: &mut Vec<PersistConn>,
) -> Option<bool> {
    while let Some(mut pc) = list.pop() {
        let too_old = old_time.is_some_and(|old| pc.idle_at.is_some_and(|at| at < old));
        if too_old || !pc.idle_conn_alive() {
            let _removed = lru.remove(&pc.idle_seq);
            dead.push(pc);
            continue;
        }
        let seq = pc.idle_seq;
        return Some(match w.try_deliver(Ok(Box::new(pc))) {
            Ok(()) => {
                let _removed = lru.remove(&seq);
                true
            }
            Err(Ok(back)) => {
                list.push(*back);
                false
            }
            Err(Err(_impossible)) => false,
        });
    }
    None
}

/// Removes the idle connection with `seq` from its host list and returns it.
fn remove_idle_seq(pool: &mut Pool, key: &ConnKey, seq: u64) -> Evicted {
    let list = pool.idle.get_mut(key)?;
    let i = list.iter().position(|pc| pc.idle_seq == seq)?;
    let pc = list.remove(i);
    if list.is_empty() {
        let _removed = pool.idle.remove(key);
    }
    Some(pc)
}

/// Closes idle connections once `IdleConnTimeout` passes (Go: `closeConnIfStillIdle` timers).
fn start_reaper(inner: &Arc<Inner>) {
    let Ok(handle) = Handle::try_current() else {
        return;
    };
    let weak = Arc::downgrade(inner);
    let notify = Arc::clone(&inner.reaper);
    let _task = handle.spawn(async move {
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            let mut expired = Vec::new();
            let next = {
                let Some(live) = weak.upgrade() else {
                    return;
                };
                let mut pool = lock(&live.pool);
                let next = reap_expired(&mut pool, live.opts.idle_conn_timeout, &mut expired);
                drop(pool);
                next
            };
            drop(expired);
            match next {
                Some(deadline) => {
                    tokio::select! {
                        () = sleep_until(deadline.into()) => {}
                        () = &mut notified => {}
                    }
                }
                None => notified.await,
            }
        }
    });
}

/// Moves the idle connections whose `IdleConnTimeout` passed to `expired`, oldest first;
/// returns when the next one expires.
fn reap_expired(
    pool: &mut Pool,
    timeout: Duration,
    expired: &mut Vec<PersistConn>,
) -> Option<Instant> {
    let now = Instant::now();
    loop {
        let (&seq, key) = pool.lru.first_key_value()?;
        let key = key.clone();
        let idle_at = pool
            .idle
            .get(&key)
            .and_then(|l| l.iter().find(|pc| pc.idle_seq == seq))
            .and_then(|pc| pc.idle_at);
        if let Some(at) = idle_at
            && at + timeout > now
        {
            return Some(at + timeout);
        }
        let _removed = pool.lru.remove(&seq);
        expired.extend(remove_idle_seq(pool, &key, seq));
    }
}

/// Go: `Transport.dialConn`.
async fn dial_conn(
    inner: &Arc<Inner>,
    ctx: &Context,
    cm: &ConnectMethod,
) -> Result<Box<PersistConn>, HttpError> {
    let trace = context_client_trace(ctx);
    let wrap = |e: HttpError| {
        if cm.proxy_url.is_some() {
            HttpError::Op(Box::new(OpError {
                op: "proxyconnect",
                net: "tcp",
                source: None,
                addr: None,
                err: e,
            }))
        } else {
            e
        }
    };
    let addr = cm.addr();
    let dialed = match &inner.opts.dial_context {
        Some(f) => f(ctx.clone(), addr.clone()).await,
        None => {
            inner
                .opts
                .dialer
                .dial_context(ctx, &addr, trace.as_ref())
                .await
        }
    }
    .map_err(wrap)?;
    let DialedConn {
        mut conn,
        local_addr,
        remote_addr,
    } = dialed;
    let tls_state = if cm.scheme() == "https" {
        let (first_host, _) =
            split_host_port(&addr).map_err(|e| wrap(HttpError::Message(e.to_string())))?;
        let peers = (local_addr.as_ref(), remote_addr.as_ref());
        let (c, state) = add_tls(inner, conn, first_host, trace.as_ref(), peers)
            .await
            .map_err(wrap)?;
        conn = c;
        Some(state)
    } else {
        None
    };
    let mut pc = PersistConn {
        conn,
        local_addr,
        remote_addr,
        br: BufReader::new(),
        read_limit: inner.opts.max_header_bytes(),
        saw_eof: false,
        nwrite: 0,
        reused: false,
        broken: false,
        canceled: None,
        forward_proxy: None,
        key: cm.key(),
        idle_at: None,
        idle_seq: 0,
        tls_state,
        max_header_bytes: inner.opts.max_header_bytes(),
        _guard: (inner.opts.max_conns_per_host > 0).then(|| ConnsGuard {
            inner: Arc::downgrade(inner),
            key: cm.key(),
        }),
    };
    match &cm.proxy_url {
        None => {}
        Some(p) if p.scheme == "socks5" || p.scheme == "socks5h" => {
            return Err(HttpError::Message(format!(
                "socks connect tcp {}->{}: unsupported by this port",
                cm.addr(),
                cm.target_addr
            )));
        }
        Some(_) if cm.target_scheme == "http" => {
            pc.forward_proxy = Some(ForwardProxy {
                authorization: cm.proxy_auth(),
            });
        }
        Some(_) => {
            proxy_connect(inner, &mut pc, cm).await?;
            let (c, state) = add_tls(
                inner,
                mem::replace(&mut pc.conn, Box::new(empty())),
                cm.tls_host(),
                trace.as_ref(),
                (pc.local_addr.as_ref(), pc.remote_addr.as_ref()),
            )
            .await?;
            pc.conn = c;
            pc.tls_state = Some(state);
        }
    }
    Ok(Box::new(pc))
}

/// Go: the `CONNECT` exchange in `dialConn` (one minute limit, status 200 required).
async fn proxy_connect(
    inner: &Arc<Inner>,
    pc: &mut PersistConn,
    cm: &ConnectMethod,
) -> Result<(), HttpError> {
    let mut hdr = inner.opts.proxy_connect_header.clone();
    if let Some(pa) = cm.proxy_auth() {
        hdr.set("Proxy-Authorization", pa);
    }
    let connect_req = Request {
        method: "CONNECT".to_owned(),
        url: Url {
            opaque: cm.target_addr.clone(),
            ..Url::default()
        },
        header: hdr,
        body: Body::Nil,
        get_body: None,
        content_length: 0,
        transfer_encoding: Vec::new(),
        close: false,
        host: cm.target_addr.clone(),
        trailer: None,
        ctx: Context::background(),
    };
    let mut tw = TransferWriter::for_request(&connect_req)?;
    tw.sanitize();
    let mut wire = Vec::new();
    connect_req.write_head(false, &Header::new(), &tw, &mut wire)?;
    let exchange = async {
        let mut written = 0;
        while written < wire.len() {
            let rest = wire.get(written..).unwrap_or_default();
            written += poll_fn(|cx| pc.poll_write(cx, rest)).await?;
        }
        let mut br = BufReader::new();
        let mut limit = inner.opts.max_header_bytes();
        let mut saw_eof = false;
        let mut reader = ResponseHeadReader::default();
        let mut head = poll_fn(|cx| {
            let mut src = ConnRead {
                conn: &mut pc.conn,
                local: pc.local_addr.as_ref(),
                remote: pc.remote_addr.as_ref(),
                read_limit: &mut limit,
                saw_eof: &mut saw_eof,
                max_header_bytes: inner.opts.max_header_bytes(),
                closed: false,
                limit_eof: true,
            };
            reader.poll_read(cx, &mut br, &mut src)
        })
        .await?;
        // Go: ReadResponse validates the framing headers too.
        fix_pragma_cache_control(&mut head.header);
        let _framing = read_transfer(
            head.status_code,
            (head.proto_major, head.proto_minor),
            "CONNECT",
            &mut head.header,
        )?;
        Ok::<_, HttpError>(head)
    };
    let reply = timeout(Duration::from_secs(60), exchange)
        .await
        .map_err(|_elapsed| HttpError::DEADLINE_EXCEEDED)??;
    if reply.status_code != 200 {
        return Err(reply
            .status
            .split_once(' ')
            .map_or(HttpError::Static("unknown status code"), |(_, text)| {
                HttpError::Message(text.to_owned())
            }));
    }
    Ok(())
}

/// Go: `persistConn.addTLS`.
async fn add_tls(
    inner: &Arc<Inner>,
    conn: Box<dyn Conn>,
    name: &str,
    trace: Option<&Arc<Traces>>,
    (local, remote): (Option<&String>, Option<&String>),
) -> Result<(Box<dyn Conn>, ConnectionState), HttpError> {
    let tls_cfg = inner.opts.tls_client_config.as_ref();
    let client = match inner
        .tls
        .get_or_init(|| tls_cfg.map_or_else(|| TlsConfig::default().client(), TlsConfig::client))
    {
        Ok(c) => c,
        Err(e) => return Err(e.clone()),
    };
    let server_name = tls_cfg
        .map(|c| c.server_name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(name);
    if let Some(t) = trace {
        t.tls_handshake_start();
    }
    let hs = handshake(client, conn, server_name, local, remote);
    let result = if inner.opts.tls_handshake_timeout.is_zero() {
        hs.await
    } else {
        timeout(inner.opts.tls_handshake_timeout, hs)
            .await
            .unwrap_or(Err(HttpError::TlsHandshakeTimeout))
    };
    if let Some(t) = trace {
        match &result {
            Ok((_, state)) => t.tls_handshake_done(state, None),
            Err(e) => t.tls_handshake_done(&ConnectionState::default(), Some(e)),
        }
    }
    result
}

/// The in-flight request write (Go: `writeLoop` running `Request.write` into the connection's
/// 4096-byte `bufio.Writer`).
struct ReqWriter {
    /// The bytes to write, in order.
    out: Vec<u8>,
    /// How many of `out` may be written now (Go's flushed bytes).
    flush_to: usize,
    /// How many of `out` were written.
    pos: usize,
    /// The body to frame once the `100 Continue` wait ends.
    deferred: Option<Deferred>,
    /// The streaming body being copied.
    stream: Option<StreamBody>,
    /// The error to report once `flush_to` is written (`true`: Go's `requestBodyReadError`).
    fail: Option<(HttpError, bool)>,
    /// Waiting for `100 Continue` before the body (Go: `waitForContinue`).
    continue_timer: Option<Pin<Box<Sleep>>>,
    /// The `100 Continue` decision from the response reader.
    continue_decision: Option<bool>,
    /// Whether write errors now happen inside Go's body copy (returned unwrapped).
    raw: bool,
    trace: Option<Arc<Traces>>,
    done: bool,
}

/// The framing of the body (the `transferWriter` fields `writeBody` uses).
#[derive(Clone)]
struct BodyFraming {
    chunked: bool,
    content_length: i64,
    trailer: Option<Header>,
}

enum Deferred {
    Bytes(Bytes, BodyFraming),
    Stream(Box<StreamBody>),
}

/// A streaming body copy (Go: `transferWriter.writeBody` over a non-memory reader).
struct StreamBody {
    reader: BodyReader,
    framing: BodyFraming,
    ncopy: i64,
    buf: Box<[u8]>,
    /// A probed first byte to send before the rest.
    first: Option<u8>,
    /// The probe timed out: its 1-byte read is still the body's first read, and Go's
    /// `finishAsyncByteRead` sends that byte as a piece of its own.
    one_byte_first: bool,
}

/// What [`ReqWriter::new`] needs to know beyond the request.
struct WriteOptions<'a> {
    using_proxy: bool,
    extra: &'a Header,
    probe: Option<ProbeResult>,
    first_byte: Option<u8>,
    wait_continue: bool,
    continue_timeout: Duration,
    trace: Option<Arc<Traces>>,
}

impl ReqWriter {
    /// Go: `Request.write` before the body: the head bytes and the transfer writer, in Go's
    /// order (the request line, `Host` and `User-Agent` are checked before the framing).
    fn head(req: &Request, o: &WriteOptions<'_>) -> Result<(Vec<u8>, TransferWriter), HttpError> {
        let mut out = Vec::with_capacity(512);
        req.write_start(o.using_proxy, &mut out)?;
        let mut tw = TransferWriter::for_request(req)?;
        if let Some(p) = &o.probe {
            tw.after_probe(p);
        }
        tw.sanitize();
        req.write_fields(o.extra, &tw, &mut out)?;
        Ok((out, tw))
    }

    /// Go: `Request.write` up to the body, and the body's framing.
    fn new(req: &mut Request, o: WriteOptions<'_>) -> Result<Self, HttpError> {
        let (out, tw) = match Self::head(req, &o) {
            Ok(head) => head,
            Err(e) => {
                // Go: Request.write reports WroteRequest for every error it returns.
                if let Some(t) = &o.trace {
                    t.wrote_request(&WroteRequestInfo {
                        err: Some(e.clone()),
                    });
                }
                return Err(e);
            }
        };
        if let Some(t) = &o.trace {
            t.wrote_headers();
        }
        let head_len = out.len();
        let mut w = Self {
            out,
            flush_to: 0,
            pos: 0,
            deferred: None,
            stream: None,
            fail: None,
            continue_timer: None,
            continue_decision: None,
            raw: false,
            trace: o.trace,
            done: false,
        };
        let body = if tw.has_body {
            mem::take(&mut req.body)
        } else {
            Body::Nil
        };
        // Leave a consumed body of the same kind behind (Go keeps the drained reader).
        match &body {
            Body::Reader(_) => req.body = Body::Reader(Box::new(empty())),
            Body::Bytes(b) => req.body = Body::Bytes(b.clone()),
            Body::NoBody => req.body = Body::NoBody,
            Body::Nil => {}
        }
        let framing = BodyFraming {
            chunked: tw.chunked(),
            content_length: tw.content_length,
            trailer: tw.trailer.clone(),
        };
        let waits = o.wait_continue && !o.continue_timeout.is_zero();
        if waits || tw.flush_headers {
            w.flush_to = head_len;
        }
        if waits {
            w.continue_timer = Some(Box::pin(sleep(o.continue_timeout)));
        }
        match body {
            Body::Nil | Body::NoBody if waits => {
                w.deferred = Some(Deferred::Bytes(Bytes::new(), framing));
            }
            Body::Nil | Body::NoBody => w.finish_in_memory(&framing, &[], false),
            Body::Bytes(b) if waits => w.deferred = Some(Deferred::Bytes(b, framing)),
            Body::Bytes(b) => w.finish_in_memory(&framing, &b, false),
            Body::Reader(reader) => {
                let stream = Box::new(StreamBody {
                    reader,
                    framing,
                    ncopy: 0,
                    buf: vec![0; 32 * 1024].into_boxed_slice(),
                    first: o.first_byte,
                    one_byte_first: o.probe == Some(ProbeResult::Slow),
                });
                if waits {
                    w.deferred = Some(Deferred::Stream(stream));
                } else {
                    w.stream = Some(*stream);
                }
            }
        }
        Ok(w)
    }

    /// Appends an in-memory body with Go's framing. On a length mismatch only the bytes Go's
    /// 4096-byte `bufio.Writer` would have flushed are sent before the error: all of them when
    /// the buffer filled (or the head was flushed for `100-continue`), none otherwise.
    fn finish_in_memory(&mut self, f: &BodyFraming, b: &[u8], head_flushed: bool) {
        if f.chunked {
            if !b.is_empty() {
                chunk_header(b.len(), &mut self.out);
                self.out.extend_from_slice(b);
                self.out.extend_from_slice(b"\r\n");
            }
            self.out.extend_from_slice(b"0\r\n");
            if let Some(t) = &f.trailer {
                self.out.extend_from_slice(t.write().as_bytes());
            }
            self.out.extend_from_slice(b"\r\n");
            self.flush_to = self.out.len();
            return;
        }
        if f.content_length == -1 {
            self.out.extend_from_slice(b);
            self.flush_to = self.out.len();
            return;
        }
        let cl = usize::try_from(f.content_length).unwrap_or(usize::MAX);
        let n = cl.min(b.len());
        self.out.extend_from_slice(b.get(..n).unwrap_or_default());
        // Go: bufio.Writer.ReadFrom flushes once its buffer fills and then writes directly.
        self.raw = self.out.len() > DEFAULT_BUF_SIZE;
        if f.content_length != i64::try_from(b.len()).unwrap_or(i64::MAX) {
            if head_flushed || self.out.len() >= DEFAULT_BUF_SIZE {
                self.flush_to = self.out.len();
            }
            self.fail = Some((
                HttpError::Message(format!(
                    "http: ContentLength={} with Body length {}",
                    f.content_length,
                    b.len()
                )),
                false,
            ));
            return;
        }
        self.flush_to = self.out.len();
    }

    /// Drives the write; `Ready(Ok)` when the request is fully written.
    fn poll(
        &mut self,
        cx: &mut TaskContext<'_>,
        pc: &mut PersistConn,
    ) -> Poll<Result<(), (HttpError, bool)>> {
        loop {
            while self.pos < self.flush_to {
                let chunk = self.out.get(self.pos..self.flush_to).unwrap_or_default();
                match ready!(pc.poll_write(cx, chunk)) {
                    Ok(0) => {
                        let raw = self.raw;
                        return self.finish(Err((HttpError::Static("short write"), raw)));
                    }
                    Ok(n) => self.pos += n,
                    Err(e) => {
                        let raw = self.raw;
                        return self.finish(Err((e, raw)));
                    }
                }
            }
            if self.pos > 0 && self.pos == self.out.len() {
                self.out.clear();
                self.pos = 0;
                self.flush_to = 0;
            }
            if let Some(timer) = &mut self.continue_timer {
                // Go: waitForContinue: the 100 Continue decision, or true once the timer fires.
                if self.continue_decision.is_none() && timer.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                let send = self.continue_decision.unwrap_or(true);
                self.continue_timer = None;
                match self.deferred.take() {
                    None => {}
                    Some(_) if !send => {
                        // Go: waitForContinue returned false: the body is not sent.
                        return self.finish(Ok(()));
                    }
                    Some(Deferred::Bytes(b, f)) => self.finish_in_memory(&f, &b, true),
                    Some(Deferred::Stream(s)) => self.stream = Some(*s),
                }
                continue;
            }
            if let Some(err) = self.fail.take() {
                return self.finish(Err(err));
            }
            let Some(body) = &mut self.stream else {
                return self.finish(Ok(()));
            };
            // Go: the body copy; its write errors are request body errors.
            self.raw = true;
            if let Some(b) = body.first.take() {
                frame_piece(body, &[b], &mut self.out);
                self.flush_to = self.out.len();
                continue;
            }
            let want = if body.one_byte_first {
                1
            } else {
                body.buf.len()
            };
            let mut rb = ReadBuf::new(body.buf.get_mut(..want).unwrap_or_default());
            let read = ready!(Pin::new(&mut body.reader).poll_read(cx, &mut rb));
            body.one_byte_first = false;
            match read {
                Err(e) => {
                    self.stream = None;
                    return self.finish(Err((HttpError::Message(e.to_string()), true)));
                }
                Ok(()) if rb.filled().is_empty() => {
                    let chunked = body.framing.chunked;
                    let cl = body.framing.content_length;
                    let ncopy = body.ncopy;
                    let trailer = body.framing.trailer.take();
                    if chunked {
                        self.out.extend_from_slice(b"0\r\n");
                        if let Some(t) = trailer {
                            self.out.extend_from_slice(t.write().as_bytes());
                        }
                        self.out.extend_from_slice(b"\r\n");
                    }
                    self.stream = None;
                    self.raw = false;
                    self.flush_to = self.out.len();
                    if cl != -1 && cl != ncopy {
                        self.fail = Some((
                            HttpError::Message(format!(
                                "http: ContentLength={cl} with Body length {ncopy}"
                            )),
                            false,
                        ));
                    }
                }
                Ok(()) => {
                    let piece = rb.filled().to_vec();
                    frame_piece(body, &piece, &mut self.out);
                    self.flush_to = self.out.len();
                }
            }
        }
    }

    /// Go: the write loop when the round trip fails before the write is done. A write that has
    /// flushed its head and waits for `100 Continue` sees the connection close
    /// (`waitForContinue` returns false) and reports `WroteRequest` without an error.
    fn abandon(&mut self) {
        let waiting = self.continue_timer.is_some() && self.deferred.is_some();
        if !self.done && waiting && self.pos >= self.flush_to {
            let _skipped = self.finish(Ok(()));
        }
    }

    fn finish(&mut self, r: Result<(), (HttpError, bool)>) -> Poll<Result<(), (HttpError, bool)>> {
        self.done = true;
        if let Some(t) = &self.trace {
            t.wrote_request(&WroteRequestInfo {
                err: r.as_ref().err().map(|e| e.0.clone()),
            });
        }
        Poll::Ready(r)
    }
}

/// Frames one read of a streaming body: a chunk, or raw bytes limited to the declared length
/// (the excess is counted, not sent, as Go's `io.Discard` copy does).
fn frame_piece(body: &mut StreamBody, piece: &[u8], out: &mut Vec<u8>) {
    let len = i64::try_from(piece.len()).unwrap_or(i64::MAX);
    if body.framing.chunked {
        chunk_header(piece.len(), out);
        out.extend_from_slice(piece);
        out.extend_from_slice(b"\r\n");
    } else if body.framing.content_length == -1 {
        out.extend_from_slice(piece);
    } else {
        let room = usize::try_from((body.framing.content_length - body.ncopy).max(0)).unwrap_or(0);
        out.extend_from_slice(piece.get(..room.min(piece.len())).unwrap_or_default());
    }
    body.ncopy += len;
}

/// Go: the 200ms body probe for GET-like requests with a body of unknown length.
async fn probe_body(reader: &mut BodyReader) -> (ProbeResult, Option<u8>, Option<HttpError>) {
    let mut one = [0_u8; 1];
    let read = async {
        let mut rb = ReadBuf::new(&mut one);
        poll_fn(|cx| Pin::new(&mut *reader).poll_read(cx, &mut rb))
            .await
            .map(|()| rb.filled().len())
    };
    match timeout(Duration::from_millis(200), read).await {
        Ok(Ok(0)) => (ProbeResult::Empty, None, None),
        Ok(Ok(_)) => (ProbeResult::HasData, one.first().copied(), None),
        Ok(Err(e)) => (
            ProbeResult::HasData,
            None,
            Some(HttpError::Message(e.to_string())),
        ),
        Err(_slow) => (ProbeResult::Slow, None, None),
    }
}

/// Go: `transportRequest` (the transport's cancelable context).
struct TransportReq {
    ctx: Context,
    cancel: CancelCauseFunc,
    trace: Option<Arc<Traces>>,
}

impl Transport {
    /// Go: `&http.Transport{...}`.
    #[must_use]
    pub fn new(opts: TransportOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                opts,
                tls: OnceLock::new(),
                pool: Mutex::new(Pool::default()),
                reaper: Arc::new(Notify::new()),
            }),
        }
    }

    /// The options the transport was built with.
    #[must_use]
    pub fn options(&self) -> &TransportOptions {
        &self.inner.opts
    }

    /// Go: `CloseIdleConnections()`.
    pub fn close_idle_connections(&self) {
        let mut pool = lock(&self.inner.pool);
        pool.close_idle = true;
        let idle = mem::take(&mut pool.idle);
        pool.lru.clear();
        drop(pool);
        drop(idle);
    }

    /// The number of idle connections (for tests and metrics).
    #[must_use]
    pub fn idle_conn_count(&self) -> usize {
        lock(&self.inner.pool).lru.len()
    }

    /// Go: `connectMethodForRequest`.
    fn connect_method_for_request(&self, req: &Request) -> Result<ConnectMethod, HttpError> {
        let proxy_url = match &self.inner.opts.proxy {
            Some(f) => f(req)?,
            None => None,
        };
        Ok(ConnectMethod {
            proxy_url,
            target_scheme: req.url.scheme.clone(),
            target_addr: canonical_addr(&req.url),
        })
    }

    /// Go: `queueForIdleConn`: `true` when an idle connection was delivered.
    fn queue_for_idle_conn(&self, w: &Arc<WantConn>) -> bool {
        let opts = &self.inner.opts;
        if opts.disable_keep_alives {
            return false;
        }
        // Dead connections are closed once the lock is released (closing one takes it).
        let mut dead = Vec::new();
        let delivered = queue_idle_locked(&mut lock(&self.inner.pool), opts, w, &mut dead);
        drop(dead);
        delivered
    }

    /// Go: `queueForDial`.
    fn queue_for_dial(&self, w: &Arc<WantConn>) {
        let max = self.inner.opts.max_conns_per_host;
        if max <= 0 {
            start_dial(&self.inner, Arc::clone(w));
            return;
        }
        let mut pool = lock(&self.inner.pool);
        let n = pool.conns_per_host.get(&w.key).copied().unwrap_or(0);
        if i64::try_from(n).is_ok_and(|count| count < max) {
            let _previous = pool.conns_per_host.insert(w.key.clone(), n + 1);
            drop(pool);
            start_dial(&self.inner, Arc::clone(w));
            return;
        }
        let q = pool.conns_per_host_wait.entry(w.key.clone()).or_default();
        while q.front().is_some_and(|f| !f.waiting()) {
            let _dropped = q.pop_front();
        }
        q.push_back(Arc::clone(w));
        drop(pool);
    }

    /// Go: `getConn`.
    async fn get_conn(
        &self,
        treq: &TransportReq,
        cm: ConnectMethod,
    ) -> Result<Box<PersistConn>, HttpError> {
        if let Some(t) = &treq.trace {
            t.get_conn(&cm.addr());
        }
        let (tx, mut rx) = oneshot::channel();
        let w = Arc::new(WantConn {
            key: cm.key(),
            cm,
            ctx: treq.ctx.without_cancel(),
            tx: Mutex::new(Some(tx)),
        });
        if !self.queue_for_idle_conn(&w) {
            self.queue_for_dial(&w);
        }
        let canceled = || {
            let cause = treq.ctx.cause().unwrap_or(HttpError::CANCELED);
            if cause == HttpError::Static("net/http: request canceled") {
                HttpError::Static("net/http: request canceled while waiting for connection")
            } else {
                cause
            }
        };
        tokio::select! {
            r = &mut rx => match r {
                Ok(Ok(mut pc)) => {
                    let idle_time = pc.idle_at.take().map(|at| at.elapsed());
                    if let Some(t) = &treq.trace {
                        t.got_conn(GotConnInfo {
                            reused: pc.reused,
                            was_idle: idle_time.is_some(),
                            idle_time: idle_time.unwrap_or_default(),
                        });
                    }
                    Ok(pc)
                }
                Ok(Err(e)) => {
                    if treq.ctx.err().is_some() {
                        return Err(canceled());
                    }
                    Err(e)
                }
                Err(_dropped) => Err(canceled()),
            },
            () = treq.ctx.done() => {
                rx.close();
                if let Ok(Ok(pc)) = rx.try_recv() {
                    put_or_close_idle_conn(&self.inner, pc);
                }
                Err(canceled())
            }
        }
    }

    /// Go: `Transport.roundTrip`.
    async fn round_trip_impl(&self, mut req: Request) -> Result<Response, HttpError> {
        let trace = context_client_trace(&req.ctx);
        let scheme = req.url.scheme.clone();
        let is_http = scheme == "http" || scheme == "https";
        if is_http {
            if let Some(e) = validate_headers(&req.header) {
                return Err(HttpError::Message(format!("net/http: invalid header {e}")));
            }
            if let Some(e) = req.trailer.as_ref().and_then(validate_headers) {
                return Err(HttpError::Message(format!("net/http: invalid trailer {e}")));
            }
        }
        if !is_http {
            return Err(HttpError::Message(format!(
                "unsupported protocol scheme {}",
                quote(&scheme)
            )));
        }
        if !req.method.is_empty() && !valid_method(&req.method) {
            return Err(HttpError::Message(format!(
                "net/http: invalid method {}",
                quote(&req.method)
            )));
        }
        if req.url.host.is_empty() {
            return Err(HttpError::Static("http: no Host in request URL"));
        }
        let (ctx, cancel) = req.ctx.with_cancel_cause();
        loop {
            if let Some(cause) = ctx.cause() {
                return Err(cause);
            }
            let treq = TransportReq {
                ctx: ctx.clone(),
                cancel: cancel.clone(),
                trace: trace.clone(),
            };
            let cm = self.connect_method_for_request(&req)?;
            let pc = match self.get_conn(&treq, cm).await {
                Ok(pc) => pc,
                Err(e) => {
                    cancel.cancel(Some(e.clone()));
                    return Err(e);
                }
            };
            let reused = pc.reused;
            match self.persist_round_trip(pc, req, treq).await {
                Ok(resp) => return Ok(resp),
                Err(failed) => {
                    let (err, back) = *failed;
                    if !should_retry_request(reused, &back, &err) {
                        let e = err.into_inner();
                        cancel.cancel(Some(e.clone()));
                        return Err(e);
                    }
                    req = rewind_body(back)?;
                }
            }
        }
    }

    /// Go: `persistConn.roundTrip` plus the response half of `readLoop`.
    async fn persist_round_trip(
        &self,
        mut pc: Box<PersistConn>,
        mut req: Request,
        treq: TransportReq,
    ) -> Result<Response, Box<(RtError, Request)>> {
        let opts = &self.inner.opts;
        let (extra, requested_gzip) = extra_headers(opts, &pc, &req);
        let wait_continue = !matches!(req.body, Body::Nil) && req.expects_continue();
        let start_nwrite = pc.nwrite;
        let probe = probe_request(&mut req).await;
        let built = ReqWriter::new(
            &mut req,
            WriteOptions {
                using_proxy: pc.forward_proxy.is_some(),
                extra: &extra,
                probe: probe.result,
                first_byte: probe.first_byte,
                wait_continue,
                continue_timeout: opts.expect_continue_timeout,
                trace: treq.trace.clone(),
            },
        );
        let mut writer = match built {
            Ok(w) => w,
            Err(e) => {
                pc.broken = true;
                return Err(Box::new((RtError::NothingWritten(e), req)));
            }
        };
        if let Some(e) = probe.err {
            writer.fail = Some((e, true));
        }
        pc.read_limit = opts.max_header_bytes();
        let method = if req.method.is_empty() {
            "GET"
        } else {
            req.method.as_str()
        };
        let read = read_head(&mut pc, &mut writer, &treq, opts.response_header_timeout)
            .await
            .and_then(|mut head| {
                fix_pragma_cache_control(&mut head.header);
                let proto = (head.proto_major, head.proto_minor);
                read_transfer(head.status_code, proto, method, &mut head.header)
                    .map(|transfer| (head, transfer))
                    .map_err(Failure::Read)
            });
        let (head, transfer) = match read {
            Ok(r) => r,
            Err(f) => {
                let err = map_failure(&mut pc, f, start_nwrite, &treq);
                writer.abandon();
                return Err(Box::new((err, req)));
            }
        };
        if writer.continue_timer.is_some() {
            // Go: a final response without 100 Continue: send the body unless closing.
            let send = !(transfer.close || req.close);
            writer.continue_decision = Some(send);
            if !send && !writer.done {
                // Go: waitForContinue returns false and the write goroutine finishes at once
                // without the body, reporting WroteRequest while the response is handed over;
                // finish the writer here so the event does not wait for a later body read.
                let _skipped = poll_fn(|cx| Poll::Ready(writer.poll(cx, &mut pc))).await;
            }
        }
        pc.read_limit = i64::MAX;
        let has_body = req.method != "HEAD" && transfer.content_length != 0;
        let alive = !(transfer.close || req.close || head.status_code <= 199);
        let mut resp = new_response(head, &transfer, pc.tls_state.clone());
        if has_body {
            let gzip = requested_gzip
                && resp
                    .header
                    .get("Content-Encoding")
                    .eq_ignore_ascii_case("gzip");
            if gzip {
                resp.header.del("Content-Encoding");
                resp.header.del("Content-Length");
                resp.content_length = -1;
                resp.uncompressed = true;
            }
            let body = ConnBody::new(pc, &self.inner, treq, writer, transfer, alive, gzip);
            resp.body = ResponseBody::from_conn(body);
        } else {
            // Go: the connection goes back to the pool before the response is returned.
            let reuse = alive && !pc.saw_eof && wrote_request(&mut writer, &mut pc).await;
            if reuse {
                let r = try_put_idle_conn(&self.inner, pc);
                if let Some(t) = &treq.trace
                    && r != Err(ERR_KEEP_ALIVES_DISABLED)
                {
                    t.put_idle_conn(r.as_ref().err());
                }
            }
            treq.cancel.cancel(Some(ERR_REQUEST_DONE));
        }
        resp.request = Some(Box::new(req));
        Ok(resp)
    }
}

/// Go: the transport's extra request headers (`Proxy-Authorization` for an HTTP proxy,
/// `Accept-Encoding: gzip`, `Connection: close`) and whether it asked for gzip.
fn extra_headers(opts: &TransportOptions, pc: &PersistConn, req: &Request) -> (Header, bool) {
    let mut extra = Header::new();
    if let Some(auth) = pc
        .forward_proxy
        .as_ref()
        .and_then(|p| p.authorization.as_ref())
    {
        extra.set("Proxy-Authorization", auth.clone());
    }
    let requested_gzip = !opts.disable_compression
        && req.header.get("Accept-Encoding").is_empty()
        && req.header.get("Range").is_empty()
        && req.method != "HEAD";
    if requested_gzip {
        extra.set("Accept-Encoding", "gzip");
    }
    if opts.disable_keep_alives && !req.wants_close() && !is_protocol_switch_header(&req.header) {
        extra.set("Connection", "close");
    }
    (extra, requested_gzip)
}

/// What the request body probe found.
#[derive(Default)]
struct Probe {
    result: Option<ProbeResult>,
    first_byte: Option<u8>,
    err: Option<HttpError>,
}

/// Go: the body probe of `shouldSendChunkedRequestBody`, for a GET-like request whose body has
/// an unknown length.
async fn probe_request(req: &mut Request) -> Probe {
    let method = if req.method.is_empty() {
        "GET"
    } else {
        req.method.as_str()
    };
    let probes = req.outgoing_length() < 0
        && req.transfer_encoding.is_empty()
        && request_method_usually_lacks_body(method);
    if !probes {
        return Probe::default();
    }
    if let Body::Bytes(b) = &req.body {
        // Go reads the first byte of any reader, in-memory or not.
        req.body = Body::Reader(Box::new(Cursor::new(b.clone())));
    }
    let Body::Reader(reader) = &mut req.body else {
        return Probe::default();
    };
    let (result, first_byte, err) = probe_body(reader).await;
    Probe {
        result: Some(result),
        first_byte,
        err,
    }
}

/// Go: `persistConn.roundTrip` waiting for the response head while the request is written,
/// with `readResponse`'s loop over 1xx responses.
async fn read_head(
    pc: &mut PersistConn,
    writer: &mut ReqWriter,
    treq: &TransportReq,
    header_timeout: Duration,
) -> Result<ResponseHead, Failure> {
    let mut head_reader = ResponseHeadReader::default();
    let mut peeked = false;
    let mut header_timer: Option<Pin<Box<Sleep>>> = None;
    let mut ctx_done: BoxFuture<'static, ()> = {
        let ctx = treq.ctx.clone();
        Box::pin(async move { ctx.done().await })
    };
    poll_fn(|cx| {
        if !writer.done {
            match writer.poll(cx, pc) {
                Poll::Ready(Ok(())) => {
                    if !header_timeout.is_zero() {
                        header_timer = Some(Box::pin(sleep(header_timeout)));
                    }
                }
                Poll::Ready(Err((e, body_read))) => {
                    return Poll::Ready(Err(Failure::Write(e, body_read)));
                }
                Poll::Pending => {}
            }
        }
        if !peeked {
            let (br, mut src) = pc.reader();
            match br.poll_peek(cx, &mut src, 1) {
                Poll::Ready(Ok(())) => {
                    peeked = true;
                    if let Some(t) = &treq.trace {
                        t.got_first_response_byte();
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(Failure::Peek(e))),
                Poll::Pending => {}
            }
        }
        if peeked {
            loop {
                let (br, mut src) = pc.reader();
                match head_reader.poll_read(cx, br, &mut src) {
                    Poll::Ready(Ok(head)) => {
                        let code = head.status_code;
                        if !(100..=199).contains(&code) || code == 101 {
                            return Poll::Ready(Ok(head));
                        }
                        // Go: non-terminal 1xx responses are skipped; a 100 releases the body.
                        if code == 100 && writer.continue_timer.is_some() {
                            writer.continue_decision = Some(true);
                            cx.waker().wake_by_ref();
                        }
                        head_reader = ResponseHeadReader::default();
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(Failure::Read(e))),
                    Poll::Pending => break,
                }
            }
        }
        if ctx_done.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Failure::Canceled));
        }
        if let Some(t) = &mut header_timer
            && t.as_mut().poll(cx).is_ready()
        {
            return Poll::Ready(Err(Failure::HeaderTimeout));
        }
        Poll::Pending
    })
    .await
}

/// The response for a head and its framing (Go: `ReadResponse` after `readTransfer`).
fn new_response(
    head: ResponseHead,
    transfer: &ResponseTransfer,
    tls: Option<ConnectionState>,
) -> Response {
    Response {
        status: head.status,
        status_code: head.status_code,
        proto: head.proto,
        proto_major: head.proto_major,
        proto_minor: head.proto_minor,
        header: head.header,
        body: ResponseBody::empty(),
        content_length: transfer.content_length,
        transfer_encoding: transfer.transfer_encoding.clone(),
        close: transfer.close,
        uncompressed: false,
        trailer: transfer.trailer.clone(),
        request: None,
        tls,
    }
}

/// Go: `mapRoundTripError` (after the connection was closed by the failure).
fn map_failure(
    pc: &mut PersistConn,
    f: Failure,
    start_nwrite: u64,
    treq: &TransportReq,
) -> RtError {
    let nothing_written = pc.nwrite == start_nwrite;
    match f {
        Failure::Canceled => {
            let cause = treq.ctx.cause().unwrap_or(HttpError::CANCELED);
            pc.cancel_request(cause.clone());
            RtError::Other(cause)
        }
        Failure::HeaderTimeout => {
            pc.broken = true;
            RtError::Other(ERR_RESPONSE_HEADER_TIMEOUT)
        }
        Failure::Write(e, true) => {
            pc.broken = true;
            RtError::Other(e)
        }
        Failure::Write(e, false) | Failure::Read(e) => {
            pc.broken = true;
            let e = if pc.read_limit <= 0 {
                HttpError::Message(format!(
                    "net/http: server response headers exceeded {} bytes; aborted",
                    pc.max_header_bytes
                ))
            } else {
                e
            };
            if nothing_written {
                RtError::NothingWritten(e)
            } else {
                RtError::Other(HttpError::ConnectionBroken(Box::new(e)))
            }
        }
        Failure::Peek(e) => {
            pc.broken = true;
            if nothing_written {
                RtError::NothingWritten(e)
            } else {
                RtError::ReadFromServer(e)
            }
        }
    }
}

/// How the head phase of a round trip failed.
enum Failure {
    Write(HttpError, bool),
    Peek(HttpError),
    Read(HttpError),
    Canceled,
    HeaderTimeout,
}

/// Go: `validateHeaders`, in key order.
fn validate_headers(h: &Header) -> Option<String> {
    for (k, vv) in h.iter() {
        if !valid_header_field_name(k) {
            return Some(format!("field name {}", quote(k)));
        }
        if vv.iter().any(|v| !valid_header_field_value(v)) {
            return Some(format!("field value for {}", quote(k)));
        }
    }
    None
}

/// Go: `isProtocolSwitchHeader`.
fn is_protocol_switch_header(h: &Header) -> bool {
    !h.get("Upgrade").is_empty() && header_values_contains_token(h.values("Connection"), "Upgrade")
}

/// Go: `fixPragmaCacheControl`.
fn fix_pragma_cache_control(h: &mut Header) {
    let no_cache = h
        .get_raw("Pragma")
        .and_then(|v| v.first())
        .is_some_and(|v| v == "no-cache");
    if no_cache && h.get_raw("Cache-Control").is_none() {
        h.insert_raw("Cache-Control", vec!["no-cache".to_owned()]);
    }
}

/// Go: `persistConn.wroteRequest`: waits up to 50ms for an unfinished write.
async fn wrote_request(writer: &mut ReqWriter, pc: &mut PersistConn) -> bool {
    if writer.done {
        return writer.fail.is_none();
    }
    let finish = poll_fn(|cx| writer.poll(cx, pc));
    matches!(timeout(Duration::from_millis(50), finish).await, Ok(Ok(())))
}

impl RoundTripper for Transport {
    fn round_trip(&self, req: Request) -> BoxFuture<'_, Result<Response, HttpError>> {
        Box::pin(self.round_trip_impl(req))
    }
}

/// The state of reading a chunked body's trailer (Go: `body.readTrailer`).
enum TrailerState {
    Start,
    Scan(usize),
    Mime(MimeHeaderReader),
    /// Read, or failed.
    Done,
}

/// The request write as the body reader sees it (Go: what `pc.wroteRequest()` reports).
enum BodyWrite {
    /// The body is still being sent.
    Running(Box<ReqWriter>),
    /// The request was written.
    Written,
    /// Writing the request failed.
    Failed,
}

/// A response body read from a connection (Go: `body` inside `bodyEOFSignal`, optionally
/// inside `gzipReader`).
pub(crate) struct ConnBody {
    /// The connection, until Go's `bodyEOFSignal.fn` (or `earlyCloseFn`) runs.
    pc: Option<Box<PersistConn>>,
    inner: Arc<Inner>,
    framing: Framing,
    remaining: u64,
    chunked: ChunkedReader,
    trailer_state: Option<TrailerState>,
    /// Go: `body.sawEOF`.
    saw_eof: bool,
    /// Go: `bodyEOFSignal.rerr`.
    rerr: Option<HttpError>,
    /// Go: `bodyEOFSignal.closed`.
    closed: bool,
    /// Go: the read loop's `alive` before the body.
    alive: bool,
    gzip: Option<GzipReader>,
    ctx_done: BoxFuture<'static, ()>,
    ctx: Context,
    cancel: CancelCauseFunc,
    write: BodyWrite,
    trace: Option<Arc<Traces>>,
    trailer: Option<Header>,
}

impl fmt::Debug for ConnBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnBody")
            .field("framing", &self.framing)
            .field("saw_eof", &self.saw_eof)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl ConnBody {
    /// Go: the `body` the read loop wraps in `bodyEOFSignal` (and `gzipReader` when the
    /// transport asked for gzip).
    fn new(
        pc: Box<PersistConn>,
        inner: &Arc<Inner>,
        treq: TransportReq,
        writer: ReqWriter,
        transfer: ResponseTransfer,
        alive: bool,
        gzip: bool,
    ) -> Self {
        let ctx = treq.ctx.clone();
        Self {
            pc: Some(pc),
            inner: Arc::clone(inner),
            framing: transfer.framing,
            remaining: match transfer.framing {
                Framing::Length(n) => n,
                Framing::NoBody | Framing::Chunked | Framing::Close => 0,
            },
            chunked: ChunkedReader::default(),
            trailer_state: None,
            saw_eof: false,
            rerr: None,
            closed: false,
            alive,
            gzip: gzip.then(GzipReader::new),
            ctx_done: Box::pin(async move { ctx.done().await }),
            ctx: treq.ctx,
            cancel: treq.cancel,
            write: if writer.done {
                BodyWrite::Written
            } else {
                BodyWrite::Running(Box::new(writer))
            },
            trace: treq.trace,
            trailer: transfer.trailer,
        }
    }

    /// The trailer read after a chunked body (Go: `Response.Trailer` once EOF is seen).
    pub(crate) const fn trailer(&self) -> Option<&Header> {
        self.trailer.as_ref()
    }

    /// Whether the chunked body ended but its trailer is still being read.
    const fn trailer_pending(&self) -> bool {
        matches!(
            self.trailer_state,
            Some(TrailerState::Start | TrailerState::Scan(_) | TrailerState::Mime(_))
        )
    }

    /// Go: `body.readTrailer`.
    fn poll_trailer(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), HttpError>> {
        let result = ready!(self.poll_trailer_step(cx));
        self.trailer_state = Some(TrailerState::Done);
        Poll::Ready(result)
    }

    /// The steps of [`ConnBody::poll_trailer`].
    fn poll_trailer_step(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), HttpError>> {
        let Some(pc) = self.pc.as_mut() else {
            return Poll::Ready(Err(ERR_TRAILER_EOF));
        };
        loop {
            let state = self.trailer_state.get_or_insert(TrailerState::Start);
            let (br, mut src) = pc.reader();
            match state {
                TrailerState::Start | TrailerState::Done => {
                    match ready!(br.poll_peek(cx, &mut src, 2)) {
                        Ok(()) if br.buffer().starts_with(b"\r\n") => {
                            br.consume(2);
                            return Poll::Ready(Ok(()));
                        }
                        Ok(()) => *state = TrailerState::Scan(4),
                        Err(_short) => return Poll::Ready(Err(ERR_TRAILER_EOF)),
                    }
                }
                TrailerState::Scan(n) => {
                    // Go: seeUpcomingDoubleCRLF, within the 4096-byte buffer.
                    let size = *n;
                    let r = ready!(br.poll_peek(cx, &mut src, size));
                    let avail = br.buffer();
                    let window = avail.get(..size.min(avail.len())).unwrap_or_default();
                    if window.ends_with(b"\r\n\r\n") {
                        *state = TrailerState::Mime(MimeHeaderReader::default());
                    } else if r.is_err() || size >= DEFAULT_BUF_SIZE {
                        return Poll::Ready(Err(HttpError::Static(
                            "http: suspiciously long trailer after chunked body",
                        )));
                    } else {
                        *state = TrailerState::Scan(size + 1);
                    }
                }
                TrailerState::Mime(reader) => {
                    return match ready!(reader.poll_read(cx, br, &mut src)) {
                        Ok(hdr) => {
                            let trailer = self.trailer.get_or_insert_with(Header::new);
                            for (k, v) in hdr {
                                trailer.insert_raw(k, v);
                            }
                            Poll::Ready(Ok(()))
                        }
                        Err(HttpError::Eof) => Poll::Ready(Err(ERR_TRAILER_EOF)),
                        Err(e) => Poll::Ready(Err(e)),
                    };
                }
            }
        }
    }

    /// Go: `body.readLocked`.
    fn poll_body_locked(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        if self.trailer_pending() {
            return match ready!(self.poll_trailer(cx)) {
                Ok(()) => Poll::Ready((0, Some(HttpError::Eof))),
                Err(e) => {
                    self.saw_eof = false;
                    Poll::Ready((0, Some(e)))
                }
            };
        }
        if self.saw_eof {
            return Poll::Ready((0, Some(HttpError::Eof)));
        }
        let Some(pc) = self.pc.as_mut() else {
            return Poll::Ready((0, Some(HttpError::Eof)));
        };
        let (n, mut err) = match self.framing {
            Framing::NoBody => (0, Some(HttpError::Eof)),
            Framing::Length(_) if self.remaining == 0 => (0, Some(HttpError::Eof)),
            Framing::Length(_) => {
                let want = usize::try_from(self.remaining)
                    .unwrap_or(usize::MAX)
                    .min(buf.len());
                let (br, mut src) = pc.reader();
                let dst = buf.get_mut(..want).unwrap_or_default();
                match ready!(br.poll_read(cx, &mut src, dst)) {
                    Ok(n) => {
                        self.remaining -= n as u64;
                        (n, None)
                    }
                    Err(e) => (0, Some(e)),
                }
            }
            Framing::Chunked => {
                let (br, mut src) = pc.reader();
                ready!(self.chunked.poll_read(cx, br, &mut src, buf))
            }
            Framing::Close => {
                let (br, mut src) = pc.reader();
                match ready!(br.poll_read(cx, &mut src, buf)) {
                    Ok(n) => (n, None),
                    Err(e) => (0, Some(e)),
                }
            }
        };
        if err == Some(HttpError::Eof) {
            self.saw_eof = true;
            match self.framing {
                Framing::Chunked => match self.poll_trailer(cx) {
                    Poll::Ready(Ok(())) => {}
                    Poll::Ready(Err(e)) => {
                        // Go: no further reads may succeed after a bad trailer.
                        self.saw_eof = false;
                        err = Some(e);
                    }
                    Poll::Pending if n > 0 => return Poll::Ready((n, None)),
                    Poll::Pending => return Poll::Pending,
                },
                Framing::Length(_) if self.remaining > 0 => err = Some(HttpError::UnexpectedEof),
                Framing::Length(_) | Framing::NoBody | Framing::Close => {}
            }
        }
        if err.is_none()
            && n > 0
            && matches!(self.framing, Framing::Length(_))
            && self.remaining == 0
        {
            err = Some(HttpError::Eof);
            self.saw_eof = true;
        }
        Poll::Ready((n, err))
    }

    /// Go: `bodyEOFSignal.fn` (the read loop's reaction to the end of the body).
    fn eof_signal(&mut self, err: HttpError) -> HttpError {
        let Some(mut pc) = self.pc.take() else {
            return err;
        };
        if err == HttpError::Eof {
            let wrote = matches!(self.write, BodyWrite::Written);
            if self.alive && !pc.saw_eof && wrote && pc.canceled.is_none() {
                pc.br = mem::take(&mut pc.br);
                let r = try_put_idle_conn(&self.inner, pc);
                if let Some(t) = &self.trace
                    && r != Err(ERR_KEEP_ALIVES_DISABLED)
                {
                    t.put_idle_conn(r.as_ref().err());
                }
            }
            self.cancel.cancel(Some(ERR_REQUEST_DONE));
            return err;
        }
        let cause = pc.canceled.clone();
        drop(pc);
        self.cancel.cancel(Some(ERR_REQUEST_DONE));
        cause.unwrap_or(err)
    }

    /// Go: `bodyEOFSignal.Read`.
    fn poll_framed(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        if self.closed {
            return Poll::Ready((0, Some(ERR_READ_ON_CLOSED_RES_BODY)));
        }
        if let Some(e) = &self.rerr {
            return Poll::Ready((0, Some(e.clone())));
        }
        if let (BodyWrite::Running(w), Some(pc)) = (&mut self.write, self.pc.as_mut())
            && let Poll::Ready(r) = w.poll(cx, pc)
        {
            self.write = if r.is_ok() {
                BodyWrite::Written
            } else {
                pc.broken = true;
                BodyWrite::Failed
            };
        }
        if let Some(pc) = self.pc.as_mut()
            && pc.canceled.is_none()
            && self.ctx_done.as_mut().poll(cx).is_ready()
        {
            pc.cancel_request(self.ctx.cause().unwrap_or(HttpError::CANCELED));
        }
        let (n, err) = ready!(self.poll_body_locked(cx, buf));
        let Some(e) = err else {
            return Poll::Ready((n, None));
        };
        if self.rerr.is_none() {
            self.rerr = Some(e.clone());
        }
        Poll::Ready((n, Some(self.eof_signal(e))))
    }

    /// Go: `gzipReader.Read` or `bodyEOFSignal.Read`.
    pub(crate) fn poll_read(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        let Some(mut gz) = self.gzip.take() else {
            return self.poll_framed(cx, buf);
        };
        if self.closed {
            self.gzip = Some(gz);
            return Poll::Ready((0, Some(ERR_READ_ON_CLOSED_RES_BODY)));
        }
        let r = gz.poll_read(cx, self, buf);
        self.gzip = Some(gz);
        r
    }

    /// Go: `bodyEOFSignal.Close` (and `gzipReader.Close`): before EOF the connection is
    /// closed, not drained.
    pub(crate) fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if self.rerr.as_ref() != Some(&HttpError::Eof) {
            // Go: earlyCloseFn: the read loop stops and closes the connection.
            self.pc = None;
            self.cancel.cancel(Some(ERR_REQUEST_DONE));
        }
    }
}

impl BodySource for ConnBody {
    fn poll_body(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        self.poll_framed(cx, buf)
    }
}

/// Bytes for a response body built in memory.
#[must_use]
pub fn bytes_body(b: Bytes) -> ResponseBody {
    ResponseBody::from_bytes(b)
}
