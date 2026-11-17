//! Go: `http.Server` (`Serve`, `Shutdown`, connection tracking) and `conn.serve`, the loop that
//! reads each request of a connection, runs the handler and finishes the response.

use core::{
    error::Error,
    fmt,
    future::pending,
    hash::BuildHasher as _,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    any::Any,
    collections::HashMap,
    hash::RandomState,
    io::{self, Write as _},
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use pbs_gocompat::time::{Duration as GoDuration, Time};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify},
    time::{Instant, sleep, sleep_until},
};

use super::{
    BoxFuture, CatchUnwind,
    body::{Body, ExpectContinue, HitEof, ServerBody},
    bufio::{BufReader, SharedReader},
    context::RequestContext,
    listener::{Accepted, ConnMetrics, Listener},
    netconn::{Closer, NetReader, NetWriter},
    request::{ReadRequest, Request, RequestError, check_request, first_raw, read_request},
    response::{Handler, ReqFacts, ResponseWriter, ServerResponse},
    status::status_text,
    writer::{SharedWriter, WriteSide},
};

/// Go: `http.DefaultMaxHeaderBytes`.
const DEFAULT_MAX_HEADER_BYTES: usize = 1 << 20;

/// Go: `rstAvoidanceDelay`.
const RST_AVOIDANCE_DELAY: Duration = Duration::from_millis(500);

/// Go: `shutdownPollIntervalMax`.
const SHUTDOWN_POLL_INTERVAL_MAX: Duration = Duration::from_millis(500);

/// Go: `Shutdown`'s `nextPollInterval`: `base` plus up to 10% jitter
/// (`rand.Intn(int(pollIntervalBase/10))`).
fn with_jitter(base: Duration) -> Duration {
    let tenth = u64::try_from((base / 10).as_nanos()).unwrap_or(u64::MAX);
    if tenth == 0 {
        return base;
    }
    base + Duration::from_nanos(RandomState::new().hash_one(()) % tenth)
}

/// Where the server's own log lines go (Go: `Server.ErrorLog`, a `*log.Logger`). The default
/// is Go's standard logger: `2009/11/10 23:00:00 message` on standard error.
pub type ErrorLog = Arc<dyn Fn(&str) + Send + Sync>;

/// Go: `log.Printf` with the standard logger's flags (`log.LstdFlags`).
fn std_log(msg: &str) {
    let mut line = Time::now().format("2006/01/02 15:04:05");
    line.push(' ');
    line.push_str(msg);
    if !msg.ends_with('\n') {
        line.push('\n');
    }
    let _ignored = io::stderr().write_all(line.as_bytes());
}

/// Go: the configuration fields of `http.Server` that Prebid sets or relies on.
#[derive(Clone)]
pub struct ServerConfig {
    /// Go: `Addr` (reported in logs; the [`Listener`] decides where the server listens).
    pub addr: String,
    /// Go: `Handler`.
    pub handler: Arc<dyn Handler>,
    /// Go: `ReadTimeout` (zero for none).
    pub read_timeout: Duration,
    /// Go: `ReadHeaderTimeout` (zero: `ReadTimeout`).
    pub read_header_timeout: Duration,
    /// Go: `WriteTimeout` (zero for none).
    pub write_timeout: Duration,
    /// Go: `IdleTimeout` (zero: `ReadTimeout`).
    pub idle_timeout: Duration,
    /// Go: `MaxHeaderBytes` (zero: `DefaultMaxHeaderBytes`, 1 MB).
    pub max_header_bytes: usize,
    /// Go: `DisableGeneralOptionsHandler`.
    pub disable_general_options_handler: bool,
    /// Go: `ErrorLog` (`None`: the standard logger).
    pub error_log: Option<ErrorLog>,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("addr", &self.addr)
            .field("read_timeout", &self.read_timeout)
            .field("read_header_timeout", &self.read_header_timeout)
            .field("write_timeout", &self.write_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_header_bytes", &self.max_header_bytes)
            .finish_non_exhaustive()
    }
}

impl ServerConfig {
    /// Go: `&http.Server{Addr: addr, Handler: handler}` (every other field zero).
    pub fn new<A: Into<String>>(addr: A, handler: Arc<dyn Handler>) -> Self {
        Self {
            addr: addr.into(),
            handler,
            read_timeout: Duration::ZERO,
            read_header_timeout: Duration::ZERO,
            write_timeout: Duration::ZERO,
            idle_timeout: Duration::ZERO,
            max_header_bytes: 0,
            disable_general_options_handler: false,
            error_log: None,
        }
    }
}

/// Go: the error `Server.Serve` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeError {
    /// Go: `http.ErrServerClosed` (`http: Server closed`).
    ServerClosed,
    /// An accept error that is not temporary, with Go's text.
    Accept(String),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerClosed => f.write_str("http: Server closed"),
            Self::Accept(t) => f.write_str(t),
        }
    }
}

impl Error for ServeError {}

/// Go: the error `Server.Shutdown` returns when its context ends first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownError {
    /// Go: `context.DeadlineExceeded` (`context deadline exceeded`).
    DeadlineExceeded,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context deadline exceeded")
    }
}

impl Error for ShutdownError {}

/// Go: `ConnState` values.
const STATE_NEW: u64 = 0;
const STATE_ACTIVE: u64 = 1;
const STATE_IDLE: u64 = 2;
const STATE_CLOSED: u64 = 4;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Go: `setState`'s packed value.
fn pack_state(state: u64) -> u64 {
    (unix_now() << 8) | state
}

/// Go: `c.rwc.Close()` on a connection that Prebid's `monitorableConnection` may wrap: the
/// first close succeeds, later ones fail with `use of closed network connection`.
fn close_rwc(closer: &Closer, metrics: Option<&Arc<dyn ConnMetrics>>) {
    let first = closer.record_close();
    closer.close();
    if let Some(m) = metrics {
        m.record_connection_close(first);
    }
}

struct ConnEntry {
    state: Arc<AtomicU64>,
    closer: Arc<Closer>,
    metrics: Option<Arc<dyn ConnMetrics>>,
}

struct Inner {
    cfg: ServerConfig,
    in_shutdown: AtomicBool,
    disable_keep_alives: AtomicBool,
    shutdown: Notify,
    listeners: AtomicUsize,
    listeners_done: Notify,
    conns: Mutex<HashMap<u64, ConnEntry>>,
    next_conn: AtomicU64,
}

impl Inner {
    fn logf(&self, msg: &str) {
        match &self.cfg.error_log {
            Some(log) => log(msg),
            None => std_log(msg),
        }
    }

    fn shutting_down(&self) -> bool {
        self.in_shutdown.load(Ordering::SeqCst)
    }

    /// Go: `doKeepAlives`.
    fn do_keep_alives(&self) -> bool {
        !self.disable_keep_alives.load(Ordering::SeqCst) && !self.shutting_down()
    }

    /// Go: `readHeaderTimeout`.
    const fn read_header_timeout(&self) -> Duration {
        if self.cfg.read_header_timeout.is_zero() {
            self.cfg.read_timeout
        } else {
            self.cfg.read_header_timeout
        }
    }

    /// Go: `idleTimeout`.
    const fn idle_timeout(&self) -> Duration {
        if self.cfg.idle_timeout.is_zero() {
            self.cfg.read_timeout
        } else {
            self.cfg.idle_timeout
        }
    }

    /// Go: `initialReadLimitSize`.
    fn initial_read_limit(&self) -> i64 {
        let max = if self.cfg.max_header_bytes == 0 {
            DEFAULT_MAX_HEADER_BYTES
        } else {
            self.cfg.max_header_bytes
        };
        i64::try_from(max).unwrap_or(i64::MAX).saturating_add(4096)
    }

    async fn wait_shutdown(&self) {
        loop {
            let notified = self.shutdown.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.shutting_down() {
                return;
            }
            notified.await;
        }
    }

    async fn wait_listeners(&self) {
        loop {
            let notified = self.listeners_done.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.listeners.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    fn untrack(&self, id: u64) {
        let _removed = self
            .conns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }

    /// Go: `closeIdleConns`.
    fn close_idle_conns(&self) -> bool {
        let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        let now = unix_now();
        let mut quiescent = true;
        conns.retain(|_, c| {
            let packed = c.state.load(Ordering::SeqCst);
            let mut st = packed & 0xFF;
            let sec = packed >> 8;
            if st == STATE_NEW && sec < now.saturating_sub(5) {
                st = STATE_IDLE;
            }
            if st != STATE_IDLE || sec == 0 {
                quiescent = false;
                return true;
            }
            close_rwc(&c.closer, c.metrics.as_ref());
            false
        });
        quiescent
    }
}

/// Go: `*http.Server`. Clones share the server.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("config", &self.inner.cfg)
            .finish_non_exhaustive()
    }
}

/// Go: `globalOptionsHandler`, which answers `OPTIONS *`.
struct GlobalOptionsHandler;

impl Handler for GlobalOptionsHandler {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            w.header().set("Content-Length", "0");
            if r.content_length != 0 {
                let notifier = w.body_limit_notifier();
                r.body.discard_limited(notifier.as_ref(), 4 << 10).await;
            }
        })
    }
}

/// Go: the `%v` of a recovered panic value.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned())
}

/// What the connection does when `serve` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Close,
    /// Go: `closeWriteAndWait` first.
    CloseWriteAndWait,
}

/// The connection state a response needs.
pub(super) struct ConnShared {
    pub(super) writer: SharedWriter,
    srv: Arc<Inner>,
    closer: Arc<Closer>,
    metrics: Option<Arc<dyn ConnMetrics>>,
}

impl ConnShared {
    pub(super) fn keep_alives_enabled(&self) -> bool {
        self.srv.do_keep_alives()
    }

    pub(super) fn logf(&self, msg: &str) {
        self.srv.logf(msg);
    }

    /// Go: `c.rwc.Close()`.
    pub(super) fn close_rwc(&self, w: &mut WriteSide) {
        w.close_conn();
        close_rwc(&self.closer, self.metrics.as_ref());
    }
}

fn nonzero(d: Duration) -> Option<Duration> {
    (!d.is_zero()).then_some(d)
}

/// Why `readRequest` failed.
#[derive(Debug)]
enum ReadFail {
    /// Go: `errTooLarge`.
    TooLarge,
    Request(RequestError),
}

/// Go: the error replies of `conn.serve`.
async fn reply_error(writer: &SharedWriter, fail: ReadFail) -> End {
    const ERROR_HEADERS: &str =
        "\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n";
    let (text, end) = match fail {
        ReadFail::TooLarge => {
            const PUBLIC_ERR: &str = "431 Request Header Fields Too Large";
            (
                format!("HTTP/1.1 {PUBLIC_ERR}{ERROR_HEADERS}{PUBLIC_ERR}"),
                End::CloseWriteAndWait,
            )
        }
        ReadFail::Request(RequestError::UnsupportedTransferEncoding) => (
            format!("HTTP/1.1 501 Not Implemented{ERROR_HEADERS}Unsupported transfer encoding"),
            End::Close,
        ),
        ReadFail::Request(RequestError::CommonNetRead) => return End::Close,
        ReadFail::Request(RequestError::Status(code, text)) => {
            let st = status_text(code);
            (
                format!("HTTP/1.1 {code} {st}: {text}{ERROR_HEADERS}{code} {st}: {text}"),
                End::Close,
            )
        }
        ReadFail::Request(RequestError::BadRequest) => {
            const PUBLIC_ERR: &str = "400 Bad Request";
            (
                format!("HTTP/1.1 {PUBLIC_ERR}{ERROR_HEADERS}{PUBLIC_ERR}"),
                End::Close,
            )
        }
    };
    writer.lock().await.write_direct(text.as_bytes()).await;
    end
}

/// Go: `connReader.startBackgroundRead` once the body is consumed (`hit`) or at once; the
/// future never completes, and dropping it is `abortPendingRead`.
async fn background_read(reader: &SharedReader, hit: Option<&HitEof>) {
    if let Some(h) = hit {
        h.wait().await;
    }
    reader.lock().await.background_read().await;
    pending::<()>().await;
}

/// Go: `http1ServerSupportsRequest`.
fn http1_server_supports_request(req: &Request) -> bool {
    req.proto_major == 1
        || (req.proto_major == 2
            && req.proto_minor == 0
            && req.method == "PRI"
            && req.request_uri == "*")
}

/// One connection's state for [`Conn::serve`].
struct Conn {
    srv: Arc<Inner>,
    shared: Arc<ConnShared>,
    reader: SharedReader,
    writer: SharedWriter,
    state: Arc<AtomicU64>,
    remote_addr: String,
}

impl Conn {
    fn set_state(&self, state: u64) {
        self.state.store(pack_state(state), Ordering::SeqCst);
    }

    /// Go: `conn.readRequest` up to the creation of the response.
    async fn read_request(&self, last_method_post: &mut bool) -> Result<ReadRequest, ReadFail> {
        let initial = self.srv.initial_read_limit();
        let t0 = Instant::now();
        let hdr_deadline = nonzero(self.srv.read_header_timeout()).map(|d| t0 + d);
        let whole_deadline = nonzero(self.srv.cfg.read_timeout).map(|d| t0 + d);
        let mut br = self.reader.lock().await;
        br.set_deadline(hdr_deadline);
        br.remain = initial;
        if *last_method_post {
            let (peek, _) = br.peek(4).await;
            let n = peek
                .iter()
                .take_while(|&&b| b == b'\r' || b == b'\n')
                .count();
            let _discarded = br.discard(n).await;
        }
        let parsed = match read_request(&mut br).await {
            Err(e) => Err(if br.remain <= 0 {
                ReadFail::TooLarge
            } else {
                ReadFail::Request(e)
            }),
            Ok(rr) if !http1_server_supports_request(&rr.req) => Err(ReadFail::Request(
                RequestError::Status(505, "unsupported protocol version"),
            )),
            Ok(mut rr) => {
                *last_method_post = rr.req.method == "POST";
                br.remain = i64::MAX;
                match check_request(&mut rr.req) {
                    Err(e) => Err(ReadFail::Request(e)),
                    Ok(h2_upgrade) => {
                        rr.h2_upgrade = h2_upgrade;
                        if hdr_deadline != whole_deadline {
                            br.set_deadline(whole_deadline);
                        }
                        Ok(rr)
                    }
                }
            }
        };
        let read_any = br.remain != initial;
        drop(br);
        if let Some(d) = nonzero(self.srv.cfg.write_timeout) {
            self.writer.lock().await.net_mut().deadline = Some(Instant::now() + d);
        }
        if read_any {
            self.set_state(STATE_ACTIVE);
        }
        parsed
    }

    /// Go: `conn.serve`'s loop.
    async fn serve(&self) -> End {
        let mut last_method_post = false;
        loop {
            let parsed = self.read_request(&mut last_method_post).await;
            if self.srv.shutting_down() {
                return End::Close;
            }
            let rr = match parsed {
                Ok(rr) => rr,
                Err(fail) => return reply_error(&self.writer, fail).await,
            };
            if let Some(end) = self.serve_one(rr).await {
                return end;
            }
            self.set_state(STATE_IDLE);
            if !self.srv.do_keep_alives() {
                return End::Close;
            }
            let mut br = self.reader.lock().await;
            br.set_deadline(nonzero(self.srv.idle_timeout()).map(|d| Instant::now() + d));
            if br.peek(4).await.1.is_some() {
                return End::Close;
            }
            br.set_deadline(None);
        }
    }

    /// Go: the request context `conn.serve` derives, cancelled by connection failures.
    async fn request_context(&self) -> RequestContext {
        let ctx = RequestContext::new();
        {
            let mut br = self.reader.lock().await;
            if br.conn_cancelled {
                ctx.cancel();
            }
            br.cancel_on_error = Some(ctx.clone());
        }
        let mut w = self.writer.lock().await;
        if w.conn_cancelled {
            ctx.cancel();
        }
        w.cancel_on_error = Some(ctx.clone());
        drop(w);
        ctx
    }

    /// Go: the deferred recover of `conn.serve` after a handler panicked.
    async fn after_panic(
        &self,
        msg: &str,
        resp: &ServerResponse,
        raw_body: Option<&Arc<ServerBody>>,
    ) {
        self.srv.logf(&format!(
            "http: panic serving {}: {msg}\n",
            self.remote_addr
        ));
        resp.disable_write_continue();
        if let Some(body) = raw_body {
            let _closed = body.close().await;
        }
    }

    /// Serves one request; `None` when the connection may be reused.
    async fn serve_one(&self, rr: ReadRequest) -> Option<End> {
        let ReadRequest {
            mut req,
            framing,
            h2_upgrade,
        } = rr;
        req.remote_addr.clone_from(&self.remote_addr);
        let ctx = self.request_context().await;
        req.ctx = ctx.clone();

        let can_write_continue = Arc::new(AtomicBool::new(false));
        let expects = req.expects_continue();
        let ecr = expects && req.proto_at_least(1, 1) && req.content_length != 0;
        let expectation_failed = !expects && !first_raw(&req.header, "Expect").is_empty();
        let raw_body = framing.map(|f| {
            let body = ServerBody::new(
                f,
                req.close,
                req.trailer.clone(),
                Arc::clone(&self.reader),
                Arc::clone(&self.writer),
            );
            Arc::new(if ecr {
                body.with_expect(ExpectContinue::new(Arc::clone(&can_write_continue)))
            } else {
                body
            })
        });
        if ecr {
            can_write_continue.store(true, Ordering::SeqCst);
        }
        let swapped = raw_body.as_ref().map_or_else(
            || Arc::new(AtomicBool::new(false)),
            |b| Arc::clone(&b.swapped),
        );
        req.body = raw_body
            .as_ref()
            .map_or_else(Body::empty, |b| Body::server(Arc::clone(b), ecr));
        let mut resp = ServerResponse::new(
            Arc::clone(&self.shared),
            ReqFacts::new(&req, ecr),
            raw_body.clone(),
            swapped,
            Arc::clone(&can_write_continue),
        );
        if h2_upgrade {
            resp.reply.close_after_reply = true;
        }
        if expectation_failed {
            resp.send_expectation_failed().await;
            return Some(End::Close);
        }

        let hit = raw_body.as_ref().map(|b| Arc::clone(&b.hit_eof));
        let options: &dyn Handler = &GlobalOptionsHandler;
        let handler: &dyn Handler = if !self.srv.cfg.disable_general_options_handler
            && req.request_uri == "*"
            && req.method == "OPTIONS"
        {
            options
        } else {
            &*self.srv.cfg.handler
        };
        let outcome = {
            let run = CatchUnwind::new(handler.serve_http(&mut resp, &mut req));
            let bg = background_read(&self.reader, hit.as_deref());
            tokio::select! {
                biased;
                out = run => out,
                () = bg => Ok(()),
            }
        };
        ctx.cancel();
        let panicked = match outcome {
            Err(payload) => Some(panic_message(payload.as_ref())),
            Ok(()) => resp.panic.take(),
        };
        if let Some(msg) = panicked {
            self.after_panic(&msg, &resp, raw_body.as_ref()).await;
            return Some(End::Close);
        }
        resp.finish_request().await;
        let werr = {
            let mut w = self.writer.lock().await;
            w.net_mut().deadline = None;
            w.werr.is_some()
        };
        if !resp.should_reuse_connection(werr).await {
            if resp.reply.request_body_limit_hit || resp.closed_request_body_early().await {
                return Some(End::CloseWriteAndWait);
            }
            return Some(End::Close);
        }
        None
    }
}

/// Go: `conn.serve`, including its deferred close.
async fn serve_conn(
    srv: Arc<Inner>,
    accepted: Accepted,
    id: u64,
    state: Arc<AtomicU64>,
    closer: Arc<Closer>,
    metrics: Option<Arc<dyn ConnMetrics>>,
) {
    let Accepted { stream, addrs } = accepted;
    let can_close_write = metrics.is_none();
    let (read_half, write_half) = stream.split();
    let reader: SharedReader = Arc::new(AsyncMutex::new(BufReader::new(NetReader::new(
        read_half,
        Arc::clone(&addrs),
        Arc::clone(&closer),
    ))));
    let writer: SharedWriter = Arc::new(AsyncMutex::new(WriteSide::new(NetWriter::new(
        write_half,
        Arc::clone(&addrs),
    ))));
    let shared = Arc::new(ConnShared {
        writer: Arc::clone(&writer),
        srv: Arc::clone(&srv),
        closer: Arc::clone(&closer),
        metrics: metrics.clone(),
    });
    let conn = Conn {
        srv: Arc::clone(&srv),
        shared,
        reader,
        writer,
        state,
        remote_addr: addrs.remote.clone(),
    };
    let end = conn.serve().await;
    {
        let mut w = conn.writer.lock().await;
        let _flushed = w.flush().await;
        if end == End::CloseWriteAndWait {
            if can_close_write {
                w.net_mut().close_write().await;
            }
            drop(w);
            sleep(RST_AVOIDANCE_DELAY).await;
        }
    }
    conn.writer.lock().await.close_conn();
    close_rwc(&closer, metrics.as_ref());
    conn.set_state(STATE_CLOSED);
    srv.untrack(id);
}

impl Server {
    /// Go: `&http.Server{...}` with `config`.
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                cfg: config,
                in_shutdown: AtomicBool::new(false),
                disable_keep_alives: AtomicBool::new(false),
                shutdown: Notify::new(),
                listeners: AtomicUsize::new(0),
                listeners_done: Notify::new(),
                conns: Mutex::new(HashMap::new()),
                next_conn: AtomicU64::new(0),
            }),
        }
    }

    /// Go: `Server.Addr`.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.inner.cfg.addr
    }

    /// Go: `Server.SetKeepAlivesEnabled`.
    pub fn set_keep_alives_enabled(&self, enabled: bool) {
        self.inner
            .disable_keep_alives
            .store(!enabled, Ordering::SeqCst);
        if !enabled {
            let _quiescent = self.inner.close_idle_conns();
        }
    }

    /// Go: `Server.Serve(l)`: accepts connections until [`Server::shutdown`], serving each on
    /// its own task. The listener is closed when it returns.
    ///
    /// # Errors
    ///
    /// [`ServeError::ServerClosed`] after a shutdown; a permanent accept error otherwise.
    pub async fn serve(&self, listener: Listener) -> Result<(), ServeError> {
        if self.inner.shutting_down() {
            listener.close();
            return Err(ServeError::ServerClosed);
        }
        let _previous = self.inner.listeners.fetch_add(1, Ordering::SeqCst);
        let result = self.accept_loop(&listener).await;
        listener.close();
        if self.inner.listeners.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.listeners_done.notify_waiters();
        }
        result
    }

    async fn accept_loop(&self, listener: &Listener) -> Result<(), ServeError> {
        let mut temp_delay = Duration::ZERO;
        loop {
            let accepted = tokio::select! {
                biased;
                () = self.inner.wait_shutdown() => {
                    listener.accept_closed();
                    return Err(ServeError::ServerClosed);
                }
                r = listener.accept() => r,
            };
            match accepted {
                Err(e) => {
                    if self.inner.shutting_down() {
                        return Err(ServeError::ServerClosed);
                    }
                    if !e.temporary {
                        return Err(ServeError::Accept(e.text));
                    }
                    temp_delay = if temp_delay.is_zero() {
                        Duration::from_millis(5)
                    } else {
                        (temp_delay * 2).min(Duration::from_secs(1))
                    };
                    let nanos = i64::try_from(temp_delay.as_nanos()).unwrap_or(i64::MAX);
                    self.inner.logf(&format!(
                        "http: Accept error: {}; retrying in {}",
                        e.text,
                        GoDuration(nanos)
                    ));
                    sleep(temp_delay).await;
                }
                Ok(accepted) => {
                    temp_delay = Duration::ZERO;
                    let id = self.inner.next_conn.fetch_add(1, Ordering::SeqCst);
                    let state = Arc::new(AtomicU64::new(pack_state(STATE_NEW)));
                    let closer = Arc::new(Closer::default());
                    let metrics = listener.metrics();
                    let _previous = self
                        .inner
                        .conns
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(
                            id,
                            ConnEntry {
                                state: Arc::clone(&state),
                                closer: Arc::clone(&closer),
                                metrics: metrics.clone(),
                            },
                        );
                    let _task = tokio::spawn(serve_conn(
                        Arc::clone(&self.inner),
                        accepted,
                        id,
                        state,
                        closer,
                        metrics,
                    ));
                }
            }
        }
    }

    /// Go: `Server.Shutdown(ctx)` with a context that ends after `timeout` (`None`: never):
    /// stops the listeners, then closes connections as they become idle.
    ///
    /// # Errors
    ///
    /// [`ShutdownError::DeadlineExceeded`] when connections are still active at the deadline.
    pub async fn shutdown(&self, timeout: Option<Duration>) -> Result<(), ShutdownError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        self.inner.in_shutdown.store(true, Ordering::SeqCst);
        self.inner.shutdown.notify_waiters();
        self.inner.wait_listeners().await;
        let mut base = Duration::from_millis(1);
        loop {
            if self.inner.close_idle_conns() {
                return Ok(());
            }
            let wait = with_jitter(base);
            base = (base * 2).min(SHUTDOWN_POLL_INTERVAL_MAX);
            match deadline {
                Some(d) => {
                    tokio::select! {
                        () = sleep_until(d) => return Err(ShutdownError::DeadlineExceeded),
                        () = sleep(wait) => {}
                    }
                }
                None => sleep(wait).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::collections::BTreeSet;

    use super::{SHUTDOWN_POLL_INTERVAL_MAX, with_jitter};

    /// Go: `nextPollInterval` waits `base` plus `rand.Intn(base/10)`.
    #[test]
    fn shutdown_poll_jitter() -> Result<(), String> {
        for base in [
            Duration::from_millis(1),
            Duration::from_millis(64),
            SHUTDOWN_POLL_INTERVAL_MAX,
        ] {
            let mut seen = BTreeSet::new();
            for _ in 0..100 {
                let got = with_jitter(base);
                if got < base || got >= base + base / 10 {
                    return Err(format!("{base:?}: {got:?} is outside [base, base*1.1)"));
                }
                let _new = seen.insert(got);
            }
            if seen.len() < 2 {
                return Err(format!("{base:?}: no jitter in {seen:?}"));
            }
        }
        Ok(())
    }
}
