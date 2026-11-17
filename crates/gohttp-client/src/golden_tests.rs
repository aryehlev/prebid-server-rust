//! Differential replay of the Go `net/http` client goldens.
//!
//! `testdata/golden/cases.json` is written by `testdata/go` (run
//! `go run ./rust/crates/gohttp-client/testdata/go` from the repository root with Go 1.26.2). For
//! every case it holds the client configuration (the way Prebid Server's `router.go` builds its
//! clients, with `ProxyFromEnvironment` over the golden's proxy environment), the scripts of raw
//! TCP capture servers, the requests, and what Go produced: the bytes every server connection
//! consumed and, per request, the response head, body, trailer, final URL, error texts and
//! `httptrace` events. This test runs the same scripts against Rust capture servers, sends the
//! same requests through [`Client`], and compares everything after the same normalization
//! (server addresses as `{{A}}`, the client side of a connection as `{{LOCAL}}`, the time in a
//! certificate expiry error as `{{NOW}}`).
//!
//! `testdata/golden/proxy.json`, from the same run, holds the `httpproxy.Config` that
//! `FromEnvironment` reads from each of a list of proxy environments and the proxy its
//! `ProxyFunc` picks for each of a list of URLs; [`go_proxy_selection_goldens`] checks
//! [`ProxyConfig::from_env_lookup`] and [`ProxySelector::proxy_for_url`] against them.

use std::{
    collections::{BTreeMap, VecDeque},
    fs, io, mem,
    net::{SocketAddr, TcpListener as StdListener, TcpStream as StdStream},
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    task::{Context as TaskContext, Poll, ready},
    time::Duration,
};

use bytes::Bytes;
use pbs_gocompat::{http::Header, strconv::quote, url::Url};
use rustls::{
    ServerConfig,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
};
use serde_json::Value;
use socket2::SockRef;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, Sleep, sleep, timeout_at},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use crate::{
    Body, CertPool, CheckRedirect, Client, Context, Dialer, HttpError, Request, RoundTripper,
    TlsConfig, Transport, TransportOptions,
    httpproxy::{Config as ProxyConfig, ProxySelector},
    httptrace::{ClientTrace, ConnectionState, GotConnInfo, with_client_trace},
    transport::ProxyFunc,
};

/// The listeners every case can script (P is also the proxy the environment names).
const SLOTS: [&str; 4] = ["A", "B", "C", "P"];
/// How long a case's server connections may run on after its requests are done.
const GRACE: Duration = Duration::from_secs(3);
/// The size of the corpus: a truncated golden file fails the test.
const MIN_CASES: usize = 371;
/// The number of proxy selections in `proxy.json`.
const MIN_PROXY_RESULTS: usize = 3395;

type Addrs = Arc<BTreeMap<String, String>>;
type TlsConfigs = Arc<BTreeMap<String, Arc<ServerConfig>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- JSON access ----

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    v.get(key).ok_or_else(|| format!("missing field {key:?}"))
}

fn array<'a>(v: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_)) => {
            Err(format!("field {key:?} is not an array"))
        }
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn int(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn millis(v: &Value, key: &str) -> Duration {
    Duration::from_millis(u64::try_from(int(v, key)).unwrap_or(0))
}

fn hex_digit(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("bad hex digit {c}")),
    }
}

/// A golden byte string: a JSON string, or `{"hex": "..."}` for bytes that are not UTF-8.
fn bytes_of(v: &Value) -> Result<Vec<u8>, String> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Object(m) => {
            let hex = m.get("hex").and_then(Value::as_str).unwrap_or_default();
            hex.as_bytes()
                .chunks(2)
                .map(|pair| match pair {
                    [hi, lo] => Ok(hex_digit(*hi)? << 4 | hex_digit(*lo)?),
                    _ => Err("odd hex length".to_owned()),
                })
                .collect()
        }
        Value::Bool(_) | Value::Number(_) | Value::Array(_) => Err(format!("not bytes: {v}")),
    }
}

fn pairs(v: &Value, key: &str) -> Result<Vec<(String, String)>, String> {
    array(v, key)?
        .iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([Value::String(k), Value::String(val)]) => Ok((k.clone(), val.clone())),
            _ => Err(format!("bad pair in {key:?}: {p}")),
        })
        .collect()
}

fn strings(v: &Value, key: &str) -> Result<Vec<String>, String> {
    array(v, key)?
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("bad string in {key:?}: {s}"))
        })
        .collect()
}

// ---- placeholders and normalization ----

fn replace_all(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while !rest.is_empty() {
        if !needle.is_empty() && rest.starts_with(needle) {
            out.extend_from_slice(with);
            rest = rest.get(needle.len()..).unwrap_or_default();
        } else {
            out.extend_from_slice(rest.get(..1).unwrap_or_default());
            rest = rest.get(1..).unwrap_or_default();
        }
    }
    out
}

/// Replaces `{{NAME}}` with the case's addresses.
fn subst(b: &[u8], addrs: &BTreeMap<String, String>) -> Vec<u8> {
    addrs.iter().fold(b.to_vec(), |acc, (name, addr)| {
        replace_all(&acc, format!("{{{{{name}}}}}").as_bytes(), addr.as_bytes())
    })
}

fn subst_str(s: &str, addrs: &BTreeMap<String, String>) -> String {
    String::from_utf8_lossy(&subst(s.as_bytes(), addrs)).into_owned()
}

/// The Go harness's `loopbackRE` and `nowRE` replacements.
fn norm_bytes(b: &[u8], addrs: &BTreeMap<String, String>) -> Vec<u8> {
    const LOOPBACK: &[u8] = b"127.0.0.1:";
    const NOW: &[u8] = b"current time ";
    let names: BTreeMap<&[u8], &str> = addrs
        .iter()
        .map(|(name, addr)| (addr.as_bytes(), name.as_str()))
        .collect();
    let mut out = Vec::with_capacity(b.len());
    let mut rest = b;
    while !rest.is_empty() {
        if rest.starts_with(LOOPBACK) {
            let tail = rest.get(LOOPBACK.len()..).unwrap_or_default();
            let digits = tail.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 {
                let len = LOOPBACK.len() + digits;
                let addr = rest.get(..len).unwrap_or_default();
                let name = names.get(addr).copied().unwrap_or("LOCAL");
                out.extend_from_slice(format!("{{{{{name}}}}}").as_bytes());
                rest = rest.get(len..).unwrap_or_default();
                continue;
            }
        }
        if rest.starts_with(NOW) {
            let tail = rest.get(NOW.len()..).unwrap_or_default();
            let stamp = tail
                .iter()
                .take_while(|c| c.is_ascii_digit() || b"T:+Z.-".contains(c))
                .count();
            let after = tail.get(stamp..).unwrap_or_default();
            if stamp > 0 && after.starts_with(b" is") {
                out.extend_from_slice(b"current time {{NOW}} is");
                rest = after.get(3..).unwrap_or_default();
                continue;
            }
        }
        out.extend_from_slice(rest.get(..1).unwrap_or_default());
        rest = rest.get(1..).unwrap_or_default();
    }
    out
}

fn norm(s: &str, addrs: &BTreeMap<String, String>) -> String {
    String::from_utf8_lossy(&norm_bytes(s.as_bytes(), addrs)).into_owned()
}

// ---- capture servers ----

/// One step of a server connection's script (the Go harness's `Op`).
#[derive(Debug, Clone)]
enum Op {
    Req,
    Head,
    Body,
    Write(Vec<u8>),
    Sleep(Duration),
    Close,
    Reset,
    Drain,
    Skip(Duration),
    Tls(String),
}

fn parse_op(v: &Value) -> Result<Op, String> {
    let data = v.get("data").map(bytes_of).transpose()?.unwrap_or_default();
    let wait = millis(v, "ms");
    Ok(match text(v, "op").as_str() {
        "req" => Op::Req,
        "head" => Op::Head,
        "body" => Op::Body,
        "write" => Op::Write(data),
        "sleep" => Op::Sleep(wait),
        "close" => Op::Close,
        "reset" => Op::Reset,
        "drain" => Op::Drain,
        "skip" => Op::Skip(wait),
        "tls" => Op::Tls(String::from_utf8(data).map_err(|e| e.to_string())?),
        other => return Err(format!("unknown op {other:?}")),
    })
}

fn parse_conns(server: &Value) -> Result<Vec<Vec<Op>>, String> {
    array(server, "conns")?
        .iter()
        .map(|c| {
            c.as_array()
                .ok_or_else(|| format!("bad connection script: {c}"))?
                .iter()
                .map(parse_op)
                .collect()
        })
        .collect()
}

/// What a listener serves during the current case.
#[derive(Default)]
struct SlotState {
    script: Option<Vec<Vec<Op>>>,
    next: usize,
    conns: Vec<Arc<Mutex<Vec<u8>>>>,
    tasks: Vec<JoinHandle<()>>,
    addrs: Addrs,
    tls: TlsConfigs,
    /// Signalled when the accept loop reaches the marker connection from this address.
    marker: Option<(SocketAddr, oneshot::Sender<()>)>,
}

/// A listener that lives for the whole run (the Go harness's `slot`).
struct Slot {
    addr: String,
    state: Arc<Mutex<SlotState>>,
}

impl Slot {
    async fn bind() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let addr = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let state = Arc::new(Mutex::new(SlotState::default()));
        let accepting = Arc::clone(&state);
        drop(tokio::spawn(async move {
            while let Ok((stream, peer)) = listener.accept().await {
                register(&accepting, stream, peer);
            }
        }));
        Ok(Self { addr, state })
    }

    /// Installs a case's scripts and forgets earlier connections.
    fn start(&self, script: Vec<Vec<Op>>, addrs: &Addrs, tls: &TlsConfigs) {
        let mut st = lock(&self.state);
        st.script = Some(script);
        st.next = 0;
        st.conns.clear();
        st.tasks.clear();
        st.addrs = Arc::clone(addrs);
        st.tls = Arc::clone(tls);
    }

    /// Returns once the accept loop has taken every connection the client opened: it queues a
    /// marker connection behind them (the accept queue is FIFO) and waits for the loop to reach
    /// it.
    async fn settle(&self) {
        let Some((marker, reached)) = self.queue_marker() else {
            return;
        };
        let _reached = timeout_at(Instant::now() + GRACE, reached).await;
        lock(&self.state).marker = None;
        drop(marker);
    }

    /// Connects the marker while holding the lock, so the accept loop cannot take it before it
    /// is known.
    fn queue_marker(&self) -> Option<(StdStream, oneshot::Receiver<()>)> {
        let mut st = lock(&self.state);
        let marker = StdStream::connect(self.addr.as_str()).ok()?;
        let local = marker.local_addr().ok()?;
        let (signal, reached) = oneshot::channel();
        st.marker = Some((local, signal));
        drop(st);
        Some((marker, reached))
    }

    /// Waits for the case's connections (aborting them after the grace period) and returns
    /// what each consumed, in accept order.
    async fn finish(&self) -> Vec<Vec<u8>> {
        self.settle().await;
        let tasks = mem::take(&mut lock(&self.state).tasks);
        let deadline = Instant::now() + GRACE;
        for mut task in tasks {
            if timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
                let _aborted = task.await;
            }
        }
        let mut st = lock(&self.state);
        st.script = None;
        st.conns.iter().map(|c| lock(c).clone()).collect()
    }
}

/// Starts serving an accepted connection with the next script of the case (or, for the marker
/// connection, signals [`Slot::settle`] and closes it).
fn register(state: &Arc<Mutex<SlotState>>, stream: TcpStream, peer: SocketAddr) {
    let mut st = lock(state);
    if let Some((_, signal)) = st.marker.take_if(|(addr, _)| *addr == peer) {
        let _sent = signal.send(());
        return;
    }
    let ops = st.script.as_ref().and_then(|s| s.get(st.next)).cloned();
    st.next += 1;
    let cap = Arc::new(Mutex::new(Vec::new()));
    st.conns.push(Arc::clone(&cap));
    let conn = ServerConn {
        io: Some(Io::Plain(stream)),
        buf: Vec::new(),
        cap,
        head: Vec::new(),
        addrs: Arc::clone(&st.addrs),
        tls: Arc::clone(&st.tls),
    };
    st.tasks.push(tokio::spawn(conn.serve(ops)));
}

/// A server connection's transport: plain TCP, or TLS after a `tls` op.
enum Io {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

/// A server connection as its script sees it (the Go harness's `stream`).
struct ServerConn {
    io: Option<Io>,
    /// Bytes read from the connection and not yet consumed by an op.
    buf: Vec<u8>,
    cap: Arc<Mutex<Vec<u8>>>,
    head: Vec<u8>,
    addrs: Addrs,
    tls: TlsConfigs,
}

impl ServerConn {
    async fn serve(mut self, ops: Option<Vec<Op>>) {
        // A connection the script did not expect is hung up on at once.
        let Some(ops) = ops else {
            return;
        };
        for op in ops {
            if !self.run(op).await {
                self.close().await;
                return;
            }
        }
        // The script is over: record whatever else the client sends until it hangs up.
        self.drain(true).await;
        self.close().await;
    }

    async fn run(&mut self, op: Op) -> bool {
        match op {
            Op::Req => self.read_head().await && self.read_body().await,
            Op::Head => self.read_head().await,
            Op::Body => self.read_body().await,
            Op::Write(data) => self.write(&subst(&data, &self.addrs)).await,
            Op::Sleep(wait) => {
                sleep(wait).await;
                true
            }
            Op::Close => {
                self.close().await;
                false
            }
            Op::Reset => {
                self.reset();
                false
            }
            Op::Drain => {
                self.drain(true).await;
                false
            }
            Op::Skip(wait) => {
                self.skip(wait).await;
                true
            }
            Op::Tls(cert) => self.start_tls(&cert).await,
        }
    }

    fn record(&self, b: &[u8]) {
        lock(&self.cap).extend_from_slice(b);
    }

    /// Reads more bytes into the buffer: the count, zero at EOF or on an error.
    async fn fill(&mut self) -> usize {
        let mut chunk = vec![0_u8; 4096];
        let read = match &mut self.io {
            Some(Io::Plain(s)) => s.read(&mut chunk).await,
            Some(Io::Tls(s)) => s.read(&mut chunk).await,
            None => Ok(0),
        };
        let n = read.unwrap_or(0);
        self.buf
            .extend_from_slice(chunk.get(..n).unwrap_or_default());
        n
    }

    async fn read_line(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(end) = self.buf.iter().position(|&c| c == b'\n') {
                let rest = self.buf.split_off(end + 1);
                let line = mem::replace(&mut self.buf, rest);
                self.record(&line);
                return Some(line);
            }
            if self.fill().await == 0 {
                let partial = mem::take(&mut self.buf);
                self.record(&partial);
                return None;
            }
        }
    }

    async fn read_n(&mut self, n: usize) -> bool {
        while self.buf.len() < n {
            if self.fill().await == 0 {
                break;
            }
        }
        let rest = self.buf.split_off(n.min(self.buf.len()));
        let got = mem::replace(&mut self.buf, rest);
        self.record(&got);
        got.len() == n
    }

    async fn read_head(&mut self) -> bool {
        let mut head = Vec::new();
        loop {
            let Some(line) = self.read_line().await else {
                return false;
            };
            head.extend_from_slice(&line);
            if line == b"\r\n" || line == b"\n" {
                self.head = head;
                return true;
            }
        }
    }

    /// Reads the body the last head announced (chunked, `Content-Length` or none).
    async fn read_body(&mut self) -> bool {
        let mut chunked = false;
        let mut length = 0_usize;
        for line in String::from_utf8_lossy(&self.head).split('\n') {
            let Some((key, value)) = line.trim_end_matches('\r').split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.to_ascii_lowercase().as_str() {
                "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
                "content-length" => {
                    if let Ok(n) = value.parse() {
                        length = n;
                    }
                }
                _ => {}
            }
        }
        if chunked {
            self.read_chunked().await
        } else {
            self.read_n(length).await
        }
    }

    async fn read_chunked(&mut self) -> bool {
        loop {
            let Some(line) = self.read_line().await else {
                return false;
            };
            let size = String::from_utf8_lossy(&line).trim().to_owned();
            let size = size.split(';').next().unwrap_or_default();
            let Ok(n) = usize::from_str_radix(size, 16) else {
                return false;
            };
            if n == 0 {
                loop {
                    match self.read_line().await {
                        None => return false,
                        Some(l) if l == b"\r\n" || l == b"\n" => return true,
                        Some(_) => {}
                    }
                }
            }
            if !self.read_n(n + 2).await {
                return false;
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> bool {
        let written = match &mut self.io {
            Some(Io::Plain(s)) => s.write_all(data).await,
            Some(Io::Tls(s)) => match s.write_all(data).await {
                Ok(()) => s.flush().await,
                Err(e) => Err(e),
            },
            None => Ok(()),
        };
        written.is_ok()
    }

    async fn drain(&mut self, record: bool) {
        loop {
            let pending = mem::take(&mut self.buf);
            if record {
                self.record(&pending);
            }
            if self.fill().await == 0 {
                return;
            }
        }
    }

    async fn skip(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        while let Ok(n) = timeout_at(deadline, self.fill()).await {
            self.buf.clear();
            if n == 0 {
                break;
            }
        }
        self.buf.clear();
    }

    /// Closes the connection (after a TLS `close_notify`, as Go's `tls.Conn.Close` sends).
    async fn close(&mut self) {
        if let Some(Io::Tls(s)) = &mut self.io {
            let _shutdown = s.shutdown().await;
        }
        self.io = None;
    }

    /// Closes the connection with an RST (`SO_LINGER` 0).
    fn reset(&mut self) {
        let tcp = match &self.io {
            Some(Io::Plain(s)) => Some(s),
            Some(Io::Tls(s)) => Some(s.get_ref().0),
            None => None,
        };
        if let Some(tcp) = tcp {
            let _linger = SockRef::from(tcp).set_linger(Some(Duration::ZERO));
        }
        self.io = None;
    }

    async fn start_tls(&mut self, cert: &str) -> bool {
        let Some(config) = self.tls.get(cert).map(Arc::clone) else {
            return false;
        };
        if !self.buf.is_empty() {
            return false;
        }
        let Some(Io::Plain(tcp)) = self.io.take() else {
            return false;
        };
        match TlsAcceptor::from(config).accept(tcp).await {
            Ok(s) => {
                self.io = Some(Io::Tls(Box::new(s)));
                true
            }
            Err(_handshake) => false,
        }
    }
}

fn server_config(cert: &Value) -> Result<Arc<ServerConfig>, String> {
    let chain = text(cert, "chain");
    let key = text(cert, "key");
    let certs = CertificateDer::pem_slice_iter(chain.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|e| e.to_string())?;
    let config = ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

/// A loopback address nothing listens on.
fn refused_addr() -> Result<String, String> {
    let listener = StdListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    drop(listener);
    Ok(addr.to_string())
}

// ---- the client side ----

/// The state every case shares (the Go harness's `runEnv`).
struct RunEnv {
    slots: BTreeMap<String, Slot>,
    tls: TlsConfigs,
    ca: String,
    proxy: Arc<ProxySelector>,
}

impl RunEnv {
    async fn new(golden: &Value) -> Result<Self, String> {
        let mut slots = BTreeMap::new();
        for name in SLOTS {
            let _previous = slots.insert(name.to_owned(), Slot::bind().await?);
        }
        let addrs: BTreeMap<String, String> = slots
            .iter()
            .map(|(name, slot)| (name.clone(), slot.addr.clone()))
            .collect();
        let env = field(golden, "proxy_env")?;
        let proxy = ProxyConfig {
            http_proxy: subst_str(&text(env, "HTTP_PROXY"), &addrs),
            https_proxy: subst_str(&text(env, "HTTPS_PROXY"), &addrs),
            no_proxy: subst_str(&text(env, "NO_PROXY"), &addrs),
            cgi: false,
        }
        .proxy_selector();
        let mut tls = BTreeMap::new();
        if let Some(certs) = field(golden, "certs")?.as_object() {
            for (name, cert) in certs {
                let _previous = tls.insert(name.clone(), server_config(cert)?);
            }
        }
        Ok(Self {
            slots,
            tls: Arc::new(tls),
            ca: text(golden, "ca"),
            proxy: Arc::new(proxy),
        })
    }

    /// Prebid's client for a case: `router.go`'s `http.Transport` fields.
    fn client(&self, cfg: &Value) -> Result<(Client, Arc<Transport>), String> {
        let selector = Arc::clone(&self.proxy);
        let proxy: ProxyFunc = Arc::new(move |req: &Request| selector.proxy_for_url(&req.url));
        let mut opts = TransportOptions {
            proxy: Some(proxy),
            dialer: Dialer {
                timeout: millis(cfg, "dial_timeout_ms"),
                keep_alive: Some(millis(cfg, "keep_alive_ms")),
            },
            tls_handshake_timeout: millis(cfg, "tls_handshake_timeout_ms"),
            disable_keep_alives: flag(cfg, "disable_keep_alives"),
            disable_compression: flag(cfg, "disable_compression"),
            max_idle_conns: int(cfg, "max_idle_conns"),
            max_idle_conns_per_host: int(cfg, "max_idle_conns_per_host"),
            max_conns_per_host: int(cfg, "max_conns_per_host"),
            idle_conn_timeout: millis(cfg, "idle_conn_timeout_ms"),
            response_header_timeout: millis(cfg, "response_header_timeout_ms"),
            expect_continue_timeout: millis(cfg, "expect_continue_timeout_ms"),
            ..TransportOptions::default()
        };
        if flag(cfg, "tls_config") {
            let mut pool = CertPool::new();
            if flag(cfg, "trust_ca") && !pool.append_certs_from_pem(self.ca.as_bytes()) {
                return Err("the golden CA does not parse".to_owned());
            }
            opts.tls_client_config = Some(TlsConfig {
                root_cas: Some(pool),
                server_name: String::new(),
            });
        }
        let transport = Arc::new(Transport::new(opts));
        let check_redirect: Option<CheckRedirect> = match text(cfg, "redirect").as_str() {
            "use_last" => Some(Arc::new(|_: &Request, _: &[Request]| {
                Err(HttpError::UseLastResponse)
            })),
            "error" => Some(Arc::new(|_: &Request, _: &[Request]| {
                Err(HttpError::Static("redirect refused"))
            })),
            _ => None,
        };
        let round_tripper: Arc<dyn RoundTripper> = Arc::<Transport>::clone(&transport);
        let client = Client {
            transport: Some(round_tripper),
            check_redirect,
            timeout: millis(cfg, "timeout_ms"),
        };
        Ok((client, transport))
    }
}

/// A request body of unknown length that returns one piece per read (the Go harness's
/// `pieces`); its first read waits for `delay`, measured from when that read starts.
struct PieceReader {
    pieces: VecDeque<Vec<u8>>,
    delay: Duration,
    /// The running wait of the first read: a read dropped mid-wait (the body probe giving up)
    /// leaves it running, as Go's probe goroutine keeps sleeping.
    wait: Option<Pin<Box<Sleep>>>,
}

impl AsyncRead for PieceReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.delay.is_zero() {
            let delay = this.delay;
            ready!(
                this.wait
                    .get_or_insert_with(|| Box::pin(sleep(delay)))
                    .as_mut()
                    .poll(cx)
            );
            this.delay = Duration::ZERO;
            this.wait = None;
        }
        if let Some(front) = this.pieces.front_mut() {
            let n = front.len().min(buf.remaining());
            let rest = front.split_off(n);
            buf.put_slice(front);
            *front = rest;
            if front.is_empty() {
                let _done = this.pieces.pop_front();
            }
        }
        Poll::Ready(Ok(()))
    }
}

fn parse_body(v: &Value) -> Result<Body, String> {
    Ok(match text(v, "kind").as_str() {
        "nobody" => Body::NoBody,
        "bytes" => Body::Bytes(Bytes::from(
            v.get("data").map(bytes_of).transpose()?.unwrap_or_default(),
        )),
        "reader" => {
            let pieces = array(v, "pieces")?
                .iter()
                .map(bytes_of)
                .collect::<Result<VecDeque<_>, _>>()?;
            Body::Reader(Box::new(PieceReader {
                pieces,
                delay: millis(v, "delay_ms"),
                wait: None,
            }))
        }
        _ => Body::Nil,
    })
}

/// The `httptrace` events of one request, in order.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<String>>>);

impl Events {
    fn push(&self, event: String) {
        lock(&self.0).push(event);
    }

    fn take(&self) -> Vec<String> {
        mem::take(&mut lock(&self.0))
    }
}

fn trace_err(err: Option<&HttpError>) -> String {
    err.map_or_else(|| "<nil>".to_owned(), ToString::to_string)
}

/// The Go harness's `tracer.trace()`.
fn recorder(events: &Events) -> ClientTrace {
    let (e1, e2, e3, e4, e5, e6) = (
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
    );
    let (e7, e8, e9, e10, e11, e12) = (
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
        events.clone(),
    );
    ClientTrace {
        get_conn: Some(Arc::new(move |hp: &str| e1.push(format!("get_conn {hp}")))),
        got_conn: Some(Arc::new(move |info: GotConnInfo| {
            e2.push(format!(
                "got_conn reused={} idle={}",
                info.reused, info.was_idle
            ));
        })),
        put_idle_conn: Some(Arc::new(move |err: Option<&HttpError>| {
            e3.push(format!("put_idle {}", trace_err(err)));
        })),
        got_first_response_byte: Some(Arc::new(move || e4.push("first_byte".to_owned()))),
        dns_start: Some(Arc::new(move |info| {
            e5.push(format!("dns_start {}", info.host));
        })),
        dns_done: Some(Arc::new(move |info| {
            e6.push(format!(
                "dns_done [{}] {}",
                info.addrs.join(" "),
                trace_err(info.err.as_ref())
            ));
        })),
        connect_start: Some(Arc::new(move |net: &str, addr: &str| {
            e7.push(format!("connect_start {net} {addr}"));
        })),
        connect_done: Some(Arc::new(
            move |net: &str, addr: &str, err: Option<&HttpError>| {
                e8.push(format!("connect_done {net} {addr} {}", trace_err(err)));
            },
        )),
        tls_handshake_start: Some(Arc::new(move || e9.push("tls_start".to_owned()))),
        tls_handshake_done: Some(Arc::new(
            move |cs: &ConnectionState, err: Option<&HttpError>| {
                e10.push(format!(
                    "tls_done complete={} version={:04x} {}",
                    cs.handshake_complete,
                    cs.version,
                    trace_err(err)
                ));
            },
        )),
        wrote_headers: Some(Arc::new(move || e11.push("wrote_headers".to_owned()))),
        wrote_request: Some(Arc::new(move |info| {
            e12.push(format!("wrote_request {}", trace_err(info.err.as_ref())));
        })),
    }
}

fn header_text(h: Option<&Header>) -> String {
    h.map_or_else(
        || "<nil>".to_owned(),
        |h| {
            h.iter()
                .flat_map(|(k, vs)| vs.iter().map(move |v| format!("{k}: {v}\n")))
                .collect()
        },
    )
}

/// What one `Client.Do` call produced (the Go harness's `Result`).
#[derive(Debug, Default, PartialEq, Eq)]
struct Outcome {
    err: String,
    status: String,
    proto: String,
    header: String,
    content_length: i64,
    transfer_encoding: Vec<String>,
    close: bool,
    uncompressed: bool,
    body: Vec<u8>,
    body_err: String,
    trailer: String,
    url: String,
    tls: String,
    trace: Vec<String>,
}

fn want_outcome(v: &Value) -> Result<Outcome, String> {
    Ok(Outcome {
        err: text(v, "err"),
        status: text(v, "status"),
        proto: text(v, "proto"),
        header: text(v, "header"),
        content_length: int(v, "content_length"),
        transfer_encoding: strings(v, "transfer_encoding")?,
        close: flag(v, "close"),
        uncompressed: flag(v, "uncompressed"),
        body: v.get("body").map(bytes_of).transpose()?.unwrap_or_default(),
        body_err: text(v, "body_err"),
        trailer: text(v, "trailer"),
        url: text(v, "url"),
        tls: text(v, "tls"),
        trace: strings(v, "trace")?,
    })
}

/// Builds the request of a step (the Go harness's `runStep` up to `Client.Do`).
fn build_request(
    spec: &Value,
    ctx: Context,
    addrs: &BTreeMap<String, String>,
) -> Result<Request, String> {
    let body = parse_body(field(spec, "body")?)?;
    let url = subst_str(&text(spec, "url"), addrs);
    let mut req = Request::new_with_context(ctx, &text(spec, "method"), &url, body)
        .map_err(|e| format!("new: {}", norm(&e.to_string(), addrs)))?;
    for (k, v) in pairs(spec, "header")? {
        req.header.add(k, subst_str(&v, addrs));
    }
    for (k, v) in pairs(spec, "raw_header")? {
        let mut values = req.header.get_raw(&k).map_or_default(<[String]>::to_vec);
        values.push(v);
        req.header.insert_raw(k, values);
    }
    if let Some(n) = spec.get("content_length").and_then(Value::as_i64) {
        req.content_length = n;
    }
    let host = text(spec, "host");
    if !host.is_empty() {
        req.host = subst_str(&host, addrs);
    }
    let method = text(spec, "method_override");
    if !method.is_empty() {
        req.method = method;
    }
    if flag(spec, "no_get_body") {
        req.get_body = None;
    }
    req.close = flag(spec, "close");
    let te = strings(spec, "transfer_encoding")?;
    if !te.is_empty() {
        req.transfer_encoding = te;
    }
    let trailer = pairs(spec, "trailer")?;
    if !trailer.is_empty() {
        let mut t = Header::new();
        for (k, v) in trailer {
            t.add(k, v);
        }
        req.trailer = Some(t);
    }
    Ok(req)
}

async fn run_step(client: Arc<Client>, step: Value, addrs: Addrs) -> Result<Outcome, String> {
    let events = Events::default();
    let spec = field(&step, "req")?;
    let traced = with_client_trace(&Context::background(), recorder(&events));
    let ctx_timeout = millis(spec, "ctx_timeout_ms");
    let (ctx, cancel) = if ctx_timeout.is_zero() {
        (traced, None)
    } else {
        let (ctx, cancel) = traced.with_timeout(ctx_timeout);
        (ctx, Some(cancel))
    };
    let mut out = Outcome::default();
    let req = match build_request(spec, ctx, &addrs) {
        Ok(req) => req,
        Err(e) => {
            out.err = e;
            return Ok(out);
        }
    };
    let (resp, err) = client.do_full(req).await;
    out.err = err.map_or_default(|e| norm(&e.to_string(), &addrs));
    if let Some(mut resp) = resp {
        out.status.clone_from(&resp.status);
        out.proto.clone_from(&resp.proto);
        out.header = norm(&header_text(Some(&resp.header)), &addrs);
        out.content_length = resp.content_length;
        out.transfer_encoding.clone_from(&resp.transfer_encoding);
        out.close = resp.close;
        out.uncompressed = resp.uncompressed;
        if text(&step, "read") == "all" {
            let (body, body_err) = resp.body.read_all_go().await;
            out.body = body;
            out.body_err = body_err.map_or_default(|e| norm(&e.to_string(), &addrs));
        }
        resp.body.close();
        out.trailer = norm(
            &header_text(resp.body.trailer().or(resp.trailer.as_ref())),
            &addrs,
        );
        out.url = resp
            .request
            .as_ref()
            .map_or_default(|r| norm(&r.url.to_string(), &addrs));
        out.tls = resp.tls.as_ref().map_or_default(tls_text);
    }
    drop(cancel);
    let settle = millis(&step, "settle_ms");
    if !settle.is_zero() {
        sleep(settle).await;
    }
    out.trace = events.take().iter().map(|e| norm(e, &addrs)).collect();
    Ok(out)
}

fn tls_text(cs: &ConnectionState) -> String {
    format!(
        "complete={} version={:04x} proto={}",
        cs.handshake_complete,
        cs.version,
        quote(&cs.negotiated_protocol)
    )
}

fn diff_outcome(step: usize, got: &Outcome, want: &Outcome) -> Vec<String> {
    let mut diffs = Vec::new();
    let mut check = |name: &str, g: String, w: String| {
        if g != w {
            diffs.push(format!("  step {step} {name}:\n    got  {g}\n    want {w}"));
        }
    };
    check("err", format!("{:?}", got.err), format!("{:?}", want.err));
    check(
        "status",
        format!("{:?}", got.status),
        format!("{:?}", want.status),
    );
    check(
        "proto",
        format!("{:?}", got.proto),
        format!("{:?}", want.proto),
    );
    check(
        "header",
        format!("{:?}", got.header),
        format!("{:?}", want.header),
    );
    check(
        "content_length",
        got.content_length.to_string(),
        want.content_length.to_string(),
    );
    check(
        "transfer_encoding",
        format!("{:?}", got.transfer_encoding),
        format!("{:?}", want.transfer_encoding),
    );
    check("close", got.close.to_string(), want.close.to_string());
    check(
        "uncompressed",
        got.uncompressed.to_string(),
        want.uncompressed.to_string(),
    );
    check(
        "body",
        format!("\"{}\"", got.body.escape_ascii()),
        format!("\"{}\"", want.body.escape_ascii()),
    );
    check(
        "body_err",
        format!("{:?}", got.body_err),
        format!("{:?}", want.body_err),
    );
    check(
        "trailer",
        format!("{:?}", got.trailer),
        format!("{:?}", want.trailer),
    );
    check("url", format!("{:?}", got.url), format!("{:?}", want.url));
    check("tls", format!("{:?}", got.tls), format!("{:?}", want.tls));
    check("trace", got.trace.join(" | "), want.trace.join(" | "));
    diffs
}

/// Runs one case and returns its differences from Go.
async fn run_case(run: &RunEnv, case: &Value) -> Result<Vec<String>, String> {
    let mut addrs: BTreeMap<String, String> = run
        .slots
        .iter()
        .map(|(name, slot)| (name.clone(), slot.addr.clone()))
        .collect();
    let mut scripted = Vec::new();
    for server in array(case, "servers")? {
        let name = text(server, "name");
        if flag(server, "closed") {
            let _previous = addrs.insert(name, refused_addr()?);
        } else {
            scripted.push((name, parse_conns(server)?));
        }
    }
    let addrs: Addrs = Arc::new(addrs);
    for (name, conns) in &scripted {
        let slot = run
            .slots
            .get(name)
            .ok_or_else(|| format!("no listener {name:?}"))?;
        slot.start(conns.clone(), &addrs, &run.tls);
    }
    let (client, transport) = run.client(field(case, "client")?)?;
    let client = Arc::new(client);
    let mut outcomes = BTreeMap::new();
    let mut pending = Vec::new();
    for (i, step) in array(case, "steps")?.iter().enumerate() {
        let wait = millis(step, "sleep_ms");
        if !wait.is_zero() {
            sleep(wait).await;
        }
        let fut = run_step(Arc::clone(&client), step.clone(), Arc::clone(&addrs));
        if flag(step, "async") {
            pending.push((i, tokio::spawn(fut)));
        } else {
            let _previous = outcomes.insert(i, fut.await?);
        }
    }
    for (i, task) in pending {
        let outcome = task.await.map_err(|e| e.to_string())??;
        let _previous = outcomes.insert(i, outcome);
    }
    transport.close_idle_connections();
    let mut diffs = Vec::new();
    for (i, want) in array(case, "results")?.iter().enumerate() {
        let want = want_outcome(want)?;
        match outcomes.get(&i) {
            Some(got) => diffs.extend(diff_outcome(i, got, &want)),
            None => diffs.push(format!("  step {i}: no result")),
        }
    }
    let captured = field(case, "captured")?;
    for (name, _conns) in &scripted {
        let slot = run
            .slots
            .get(name)
            .ok_or_else(|| format!("no listener {name:?}"))?;
        let got: Vec<String> = slot
            .finish()
            .await
            .iter()
            .map(|c| format!("\"{}\"", norm_bytes(c, &addrs).escape_ascii()))
            .collect();
        let want: Vec<String> = array(captured, name)?
            .iter()
            .map(|c| bytes_of(c).map(|b| format!("\"{}\"", b.escape_ascii())))
            .collect::<Result<_, _>>()?;
        if got != want {
            diffs.push(format!(
                "  server {name} connections:\n    got  {}\n    want {}",
                got.join("\n         "),
                want.join("\n         ")
            ));
        }
    }
    Ok(diffs)
}

fn load_golden(name: &str) -> Result<Value, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("golden")
        .join(name);
    let content = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

/// `httpproxy.FromEnvironment` and its `ProxyFunc` (what `http.ProxyFromEnvironment` runs) for
/// every probe environment and URL of `proxy.json`.
#[test]
fn go_proxy_selection_goldens() -> Result<(), String> {
    let golden = load_golden("proxy.json")?;
    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for probe in array(&golden, "probes")? {
        let env: BTreeMap<String, String> = field(probe, "env")?
            .as_object()
            .ok_or_else(|| format!("env is not an object: {probe}"))?
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
            .collect();
        let config = ProxyConfig::from_env_lookup(|name| env.get(name).cloned());
        let want = field(probe, "config")?;
        let want = ProxyConfig {
            http_proxy: text(want, "http_proxy"),
            https_proxy: text(want, "https_proxy"),
            no_proxy: text(want, "no_proxy"),
            cgi: flag(want, "cgi"),
        };
        if config != want {
            failures.push(format!(
                "env {env:?}:\n    got  {config:?}\n    want {want:?}"
            ));
        }
        let selector = config.proxy_selector();
        for result in array(probe, "results")? {
            let url = text(result, "url");
            let (proxy, err) = match Url::parse(&url) {
                Err(e) => (String::new(), format!("parse: {e}")),
                Ok(u) => match selector.proxy_for_url(&u) {
                    Ok(p) => (p.map_or_default(|p| p.to_string()), String::new()),
                    Err(e) => (String::new(), e.to_string()),
                },
            };
            checked += 1;
            let (want_proxy, want_err) = (text(result, "proxy"), text(result, "err"));
            if proxy != want_proxy || err != want_err {
                failures.push(format!(
                    "env {env:?} url {url:?}:\n    got  ({proxy:?}, {err:?})\n    want \
                     ({want_proxy:?}, {want_err:?})"
                ));
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "{} of {checked} proxy selections differ from Go:\n{}",
            failures.len(),
            failures.join("\n")
        ));
    }
    if checked < MIN_PROXY_RESULTS {
        return Err(format!(
            "only {checked} proxy selections, expected at least {MIN_PROXY_RESULTS}"
        ));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn go_net_http_goldens() -> Result<(), String> {
    let golden = load_golden("cases.json")?;
    let run = RunEnv::new(&golden).await?;
    let cases = array(&golden, "cases")?;
    let mut failures = Vec::new();
    for case in cases {
        let diffs = run_case(&run, case).await?;
        if !diffs.is_empty() {
            failures.push(format!(
                "case {}:\n{}",
                text(case, "name"),
                diffs.join("\n")
            ));
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "{} of {} cases differ from Go:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        ));
    }
    if cases.len() < MIN_CASES {
        return Err(format!(
            "only {} cases, expected at least {MIN_CASES}",
            cases.len()
        ));
    }
    Ok(())
}
