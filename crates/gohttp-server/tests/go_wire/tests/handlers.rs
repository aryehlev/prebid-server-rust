//! The servers of `testdata/go/wire/main.go` and the handlers of its `handlers.go`, one for one.

use core::{fmt::Write as _, mem, time::Duration};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io,
    panic::resume_unwind,
    path::{Path, PathBuf},
    process,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use pbs_gocompat::{
    strconv::{atoi, quote, quote_bytes},
    strings::to_lower,
    url::Values,
};
use pbs_gohttp_server::{
    Body, BoxFuture, ConnMetrics, Dir, ErrorLog, Handler, HandlerFunc, Header, ListenError,
    Listener, Request, ResponseWriter, ServeMux, Server, ServerConfig, StripPrefix, error,
    httprouter::{Handle, HandleFunc, Params, RouteError, Router},
    max_bytes_reader, not_found,
    prebid::{NoCache, admin_server, main_server, support_cors},
    redirect, serve_file,
};
use tokio::{task::JoinHandle, time::sleep};

/// The modification time of every static file and directory: 2020-01-02T03:04:05Z.
const STATIC_MOD_TIME: u64 = 1_577_934_245;

/// The directories of the static tree, created first and timestamped last.
const STATIC_DIRS: [&str; 3] = ["sub", "nested", "empty"];

/// The files of the static tree (Go: `staticFiles`).
fn static_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "index.html",
            b"<html><body>Prebid Server</body></html>\n".to_vec(),
        ),
        ("a.txt", b"hello, static\n".to_vec()),
        ("data.json", b"{\"a\":1}".to_vec()),
        ("big.txt", b"0123456789".repeat(300)),
        ("noext", b"\x00\x01\x02binary".to_vec()),
        ("spa ce.txt", b"space\n".to_vec()),
        ("sub/index.html", b"<p>sub</p>\n".to_vec()),
        ("sub/b.css", b"body{}\n".to_vec()),
        ("nested/c.js", b"var c = 1;\n".to_vec()),
        ("nested/d.json", b"[1,2]".to_vec()),
    ]
}

/// A scratch directory, removed when dropped.
#[derive(Debug)]
pub(super) struct TempDir {
    pub(super) path: PathBuf,
}

impl TempDir {
    /// A new directory under the system temporary directory, named short enough for a Unix
    /// socket path inside it.
    pub(super) fn create() -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = env::temp_dir().join(format!("gw{}-{nanos}", process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.path);
    }
}

fn set_mod_time(path: &Path) -> io::Result<()> {
    File::open(path)?.set_modified(UNIX_EPOCH + Duration::from_secs(STATIC_MOD_TIME))
}

/// Go: `writeStatic`.
pub(super) fn write_static(root: &Path) -> io::Result<()> {
    for d in STATIC_DIRS {
        fs::create_dir_all(root.join(d))?;
    }
    for (name, body) in static_files() {
        let p = root.join(name);
        fs::write(&p, body)?;
        set_mod_time(&p)?;
    }
    for d in STATIC_DIRS {
        set_mod_time(&root.join(d))?;
    }
    set_mod_time(root)
}

/// Where a server listens.
#[derive(Debug, Clone)]
pub(super) enum Target {
    Tcp(String),
    Unix(String),
}

/// Prebid's `monitorableListener` needs a metrics engine; these tests record nothing.
#[derive(Debug, Clone, Copy)]
struct NoMetrics;

impl ConnMetrics for NoMetrics {
    fn record_connection_accept(&self, _success: bool) {}

    fn record_connection_close(&self, _success: bool) {}
}

/// The running servers of a test.
#[derive(Debug, Default)]
pub(super) struct Servers {
    pub(super) targets: BTreeMap<&'static str, Target>,
    running: Vec<Arc<Server>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Servers {
    fn spawn(&mut self, name: &'static str, target: Target, cfg: ServerConfig, l: Listener) {
        let server = Arc::new(Server::new(cfg));
        let serving = Arc::clone(&server);
        self.tasks.push(tokio::spawn(async move {
            let _ended = serving.serve(l).await;
        }));
        self.running.push(server);
        let _previous = self.targets.insert(name, target);
    }

    /// Shuts every server down.
    pub(super) async fn stop(self) {
        for s in &self.running {
            let _done = s.shutdown(Some(Duration::from_secs(2))).await;
        }
        for t in self.tasks {
            t.abort();
        }
    }
}

pub(super) fn quiet(mut cfg: ServerConfig) -> ServerConfig {
    let log: ErrorLog = Arc::new(|_line: &str| {});
    cfg.error_log = Some(log);
    cfg
}

async fn bind_local() -> Result<(Listener, String), String> {
    let l = Listener::bind_tcp("127.0.0.1:0")
        .await
        .map_err(|e: ListenError| e.to_string())?;
    let addr = l.addr().to_owned();
    Ok((l, addr))
}

/// Go: `startServers`.
pub(super) async fn start_servers(static_root: &Path, socket: &Path) -> Result<Servers, String> {
    let public = main_handler(static_root)?;
    let mut s = Servers::default();

    let (main_l, main_addr) = bind_local().await?;
    let main_cfg = quiet(main_server(&main_addr, Arc::clone(&public)));
    let main_l = main_l.with_metrics(Arc::new(NoMetrics));
    s.spawn("main", Target::Tcp(main_addr), main_cfg, main_l);

    let path = socket.to_string_lossy().into_owned();
    let unix_l = Listener::bind_unix(&path)
        .map_err(|e| e.to_string())?
        .with_metrics(Arc::new(NoMetrics));
    let unix_cfg = quiet(main_server(&path, Arc::clone(&public)));
    s.spawn("unix", Target::Unix(path), unix_cfg, unix_l);

    let (plain_l, plain_addr) = bind_local().await?;
    let plain_cfg = quiet(main_server(&plain_addr, public));
    s.spawn("plain", Target::Tcp(plain_addr), plain_cfg, plain_l);

    let (admin_l, admin_addr) = bind_local().await?;
    let admin_cfg = quiet(admin_server(&admin_addr, admin_handler()?));
    s.spawn("admin", Target::Tcp(admin_addr), admin_cfg, admin_l);

    let (short_l, short_addr) = bind_local().await?;
    let mut short_cfg = quiet(ServerConfig::new(short_addr.as_str(), short_handler()?));
    short_cfg.read_timeout = Duration::from_millis(300);
    short_cfg.write_timeout = Duration::from_secs(1);
    s.spawn("short", Target::Tcp(short_addr), short_cfg, short_l);
    Ok(s)
}

fn route(result: Result<(), RouteError>) -> Result<(), String> {
    result.map_err(|e| e.to_string())
}

/// Go: `newMainHandler`.
fn main_handler(static_root: &Path) -> Result<Arc<dyn Handler>, String> {
    let root = static_root.to_string_lossy().into_owned();
    let mut r = Router::new();
    route(r.post("/openrtb2/auction", EchoBody(0)))?;
    route(r.post("/openrtb2/video", EchoBody(10)))?;
    route(r.get("/openrtb2/amp", HandleFunc(echo_query)))?;
    route(r.get("/info/bidders", JsonResponse("[\"appnexus\",\"rubicon\"]")))?;
    route(r.get("/info/bidders/:bidderName", HandleFunc(bidder_detail)))?;
    route(r.get("/bidders/params", JsonResponse("{\"appnexus\":{}}")))?;
    route(r.post("/cookie_sync", EchoBody(0)))?;
    route(r.get("/status", HandleFunc(status)))?;
    route(r.get("/", ServeIndex(format!("{root}/index.html"))))?;
    route(r.handler("GET", "/version", Arc::new(HandlerFunc(version))))?;
    route(r.serve_files("/static/*filepath", Dir(root)))?;
    route(r.post("/vtrack", EchoBody(0)))?;
    route(r.get("/event", HandleFunc(echo_query)))?;
    route(r.get("/setuid", HandleFunc(set_uid)))?;
    route(r.get("/getuids", JsonResponse("{\"buyeruids\":{}}")))?;
    route(r.post("/optout", HandleFunc(opt_out)))?;
    route(r.get("/optout", HandleFunc(opt_out)))?;

    route(r.get("/t/write/:n", HandleFunc(write_n)))?;
    route(r.get("/t/writes/:n/:size", HandleFunc(writes_n)))?;
    route(r.get("/t/flush/:mode", HandleFunc(flush_mode)))?;
    route(r.get("/t/cl/:declared/:actual", HandleFunc(declared_length)))?;
    route(r.get("/t/status/:code/:body", HandleFunc(status_code)))?;
    route(r.get("/t/twice", HandleFunc(write_header_twice)))?;
    route(r.get("/t/header/:mode", HandleFunc(header_mode)))?;
    route(r.get("/t/panic/:mode", HandleFunc(panic_mode)))?;
    route(r.get("/t/error/:mode", HandleFunc(error_mode)))?;
    route(r.get("/t/redirect/:code", HandleFunc(redirect_to)))?;
    route(r.post("/t/redirect/:code", HandleFunc(redirect_to)))?;
    let strip = StripPrefix::new("/t/strip", HandlerFunc(echo_url));
    route(r.handler("GET", "/t/strip/*rest", Arc::new(strip)))?;
    route(r.post("/t/body/:mode", HandleFunc(body_mode)))?;
    route(r.get("/t/form", HandleFunc(form)))?;
    route(r.post("/t/form", HandleFunc(form)))?;
    route(r.get("/t/cookie", HandleFunc(cookie)))?;
    for m in ["GET", "POST", "PUT", "DELETE", "PATCH", "PURGE"] {
        route(r.handle(m, "/t/req/*rest", HandleFunc(echo_request)))?;
    }
    route(r.get("/t/remote", HandleFunc(remote_addr)))?;
    route(r.get("/t/plan/:spec", HandleFunc(plan)))?;
    Ok(Arc::new(NoCache {
        handler: support_cors(r),
    }))
}

/// Go: `newAdminHandler`.
fn admin_handler() -> Result<Arc<dyn Handler>, String> {
    let mut mux = ServeMux::new();
    let routes: [(&str, Arc<dyn Handler>); 8] = [
        ("/debug/pprof/", Arc::new(AdminEcho("index"))),
        ("/debug/pprof/cmdline", Arc::new(AdminEcho("cmdline"))),
        ("/debug/pprof/profile", Arc::new(AdminEcho("profile"))),
        ("/debug/pprof/symbol", Arc::new(AdminEcho("symbol"))),
        ("/debug/pprof/trace", Arc::new(AdminEcho("trace"))),
        ("/currency/rates", Arc::new(AdminEcho("rates"))),
        ("/version", Arc::new(HandlerFunc(version))),
        ("/t/plan/", Arc::new(HandlerFunc(admin_plan))),
    ];
    for (pattern, handler) in routes {
        mux.handle(pattern, handler).map_err(|e| e.to_string())?;
    }
    Ok(Arc::new(mux))
}

/// Go: `newShortHandler`.
/// Go: `newDrainHandler`.
pub(super) fn drain_handler() -> Result<Arc<dyn Handler>, String> {
    let mut r = Router::new();
    route(r.get("/status", HandleFunc(status)))?;
    route(r.get("/hold/:ms", HandleFunc(hold)))?;
    Ok(Arc::new(NoCache {
        handler: support_cors(r),
    }))
}

fn short_handler() -> Result<Arc<dyn Handler>, String> {
    let mut r = Router::new();
    route(r.get("/t/sleep/:ms", HandleFunc(sleep_ms)))?;
    route(r.post("/t/slowbody", HandleFunc(slow_body)))?;
    route(r.get("/status", HandleFunc(status)))?;
    Ok(Arc::new(r))
}

/// Go: `formatHeader`.
fn format_header(h: Option<&Header>) -> String {
    let mut out = String::new();
    for (k, vs) in h.into_iter().flat_map(Header::iter) {
        if vs.is_empty() {
            let _ok = write!(out, " {k}=<none>");
        }
        for v in vs {
            let _ok = write!(out, " {k}={}", quote(v));
        }
    }
    out
}

/// Go: `pattern`.
fn pattern(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| b'a' + u8::try_from(i % 26).unwrap_or(0))
        .collect()
}

/// Go: `atoi` (`strconv.Atoi`, 0 on error).
fn atoi_or_zero(s: &str) -> i64 {
    atoi(s).unwrap_or(0)
}

fn count(s: &str) -> usize {
    usize::try_from(atoi_or_zero(s)).unwrap_or(0)
}

fn code(s: &str) -> u16 {
    u16::try_from(atoi_or_zero(s)).unwrap_or(0)
}

/// Go's `%v` of an error that may be nil.
fn err_text<E: ToString>(err: Option<E>) -> String {
    err.map_or_else(|| "<nil>".to_owned(), |e| e.to_string())
}

async fn write_str(w: &mut dyn ResponseWriter, s: &str) {
    let _written = w.write(s.as_bytes()).await;
}

/// Go: `io.ReadAll(io.LimitReader(body, limit))`.
async fn read_limited(body: &mut Body, limit: usize) -> (Vec<u8>, Option<io::Error>) {
    let mut out = Vec::new();
    let mut buf = vec![0_u8; 512];
    while out.len() < limit {
        let want = (limit - out.len()).min(buf.len());
        match body.read(buf.get_mut(..want).unwrap_or_default()).await {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(buf.get(..n).unwrap_or_default()),
            Err(e) => return (out, Some(e)),
        }
    }
    (out, None)
}

/// Go: `io.ReadFull(body, buf[:n])`.
async fn read_full(body: &mut Body, n: usize) -> (usize, Option<String>) {
    let mut buf = vec![0_u8; n];
    let mut got = 0;
    while got < n {
        match body.read(buf.get_mut(got..).unwrap_or_default()).await {
            Ok(0) if got == 0 => return (0, Some("EOF".to_owned())),
            Ok(0) => return (got, Some("unexpected EOF".to_owned())),
            Ok(k) => got += k,
            Err(e) => return (got, Some(e.to_string())),
        }
    }
    (got, None)
}

/// Go: `io.ReadAll(body)`.
async fn read_all(body: &mut Body) -> (Vec<u8>, Option<io::Error>) {
    let mut out = Vec::new();
    let err = body.read_to_end(&mut out).await.err();
    (out, err)
}

/// Go: `echoBody(limit)`.
#[derive(Debug, Clone, Copy)]
struct EchoBody(usize);

impl Handle for EchoBody {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        _ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (body, err) = if self.0 > 0 {
                read_limited(&mut r.body, self.0).await
            } else {
                read_all(&mut r.body).await
            };
            w.header().set("Content-Type", "application/json");
            if let Some(e) = err {
                w.write_header(400);
                write_str(w, &format!("read error: {e}\n")).await;
                return;
            }
            let trailer = r.body.trailer().await;
            let text = format!(
                "len={} body={} trailer:{}\n",
                body.len(),
                quote_bytes(&body),
                format_header(trailer.as_ref())
            );
            write_str(w, &text).await;
        })
    }
}

fn echo_query<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let text = format!("raw={} encoded={}", r.url.raw_query, r.url.query().encode());
        write_str(w, &text).await;
    })
}

/// Go: `jsonResponse(body)`.
#[derive(Debug, Clone, Copy)]
struct JsonResponse(&'static str);

impl Handle for JsonResponse {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
        _ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            w.header().add("Content-Type", "application/json");
            write_str(w, self.0).await;
        })
    }
}

fn bidder_detail<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let response = match to_lower(ps.by_name("bidderName")).as_str() {
            "appnexus" => Some("{\"status\":\"ACTIVE\"}"),
            "all" => Some("{\"appnexus\":{}}"),
            _ => None,
        };
        if response.is_none() {
            w.write_header(404);
        }
        if let Some(body) = response {
            w.header().set("Content-Type", "application/json");
            write_str(w, body).await;
        } else {
            w.write_header(404);
        }
    })
}

fn status<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        w.write_header(204);
    })
}

/// `endpoints.NewVersionEndpoint("", "")`.
fn version<'a>(w: &'a mut dyn ResponseWriter, _r: &'a mut Request) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        write_str(w, "{\"revision\":\"not-set\",\"version\":\"not-set\"}").await;
    })
}

/// Go: `serveIndex(name)`.
#[derive(Debug, Clone)]
struct ServeIndex(String);

impl Handle for ServeIndex {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        _ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            serve_file(w, r, &self.0).await;
        })
    }
}

fn set_uid<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let q = r.url.query();
        if q.get("bidder").is_empty() {
            w.write_header(400);
            write_str(w, "\"bidder\" query param is required").await;
            return;
        }
        let cookie = format!("uids={}; Path=/; Max-Age=60", q.get("uid"));
        w.header().add("Set-Cookie", cookie);
        w.header().set("Content-Type", "image/gif");
        write_str(w, "GIF89a").await;
    })
}

fn opt_out<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let url = format!("https://example.com/optout?from={}", r.method);
        redirect(w, r, &url, 301).await;
    })
}

fn write_n<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let _written = w.write(&pattern(count(ps.by_name("n")))).await;
    })
}

fn writes_n<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let chunk = pattern(count(ps.by_name("size")));
        for _ in 0..count(ps.by_name("n")) {
            let _written = w.write(&chunk).await;
        }
    })
}

fn flush_mode<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        match ps.by_name("mode") {
            "early" => {
                let _flushed = w.flush().await;
                write_str(w, "x").await;
            }
            "mid" => {
                write_str(w, "a").await;
                let _flushed = w.flush().await;
                write_str(w, "b").await;
            }
            "end" => {
                write_str(w, "abc").await;
                let _flushed = w.flush().await;
            }
            "header" => {
                w.write_header(202);
                let _flushed = w.flush().await;
            }
            "html" => {
                write_str(w, "<html><body>").await;
                let _flushed = w.flush().await;
                write_str(w, "</body></html>").await;
            }
            _ => {}
        }
    })
}

fn declared_length<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        w.header().set("Content-Length", ps.by_name("declared"));
        let actual = pattern(count(ps.by_name("actual")));
        if let Err(e) = w.write(&actual).await {
            write_str(w, &e.to_string()).await;
        }
    })
}

fn status_code<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        w.header().set("X-Before", "1");
        w.write_header(code(ps.by_name("code")));
        if ps.by_name("body") == "y" {
            write_str(w, "body").await;
        }
    })
}

/// Go: `planBodies`.
fn plan_body(name: &str) -> &'static [u8] {
    match name {
        "json" => b"{\"a\":1}",
        "html" => b"<html>",
        "htmlws" => b" \t\r\n<!DOCTYPE HTML>",
        "htmlopen" => b"<html",
        "bin" => b"\x00\x01\x02",
        "xml" => b"<?xml version=\"1.0\"?>",
        "text" => b"hello",
        "pdf" => b"%PDF-1.4",
        "bom" => b"\xEF\xBB\xBFx",
        "gif" => b"GIF89a",
        "lt" => b"<",
        "ws" => b" \t\r\n",
        "ht" => b"<ht",
        "ml" => b"ml>",
        _ => b"",
    }
}

/// Go: `runPlan`.
async fn run_plan(w: &mut dyn ResponseWriter, spec: &str) {
    for op in spec.split('.') {
        let Some(kind) = op.bytes().next() else {
            continue;
        };
        let arg = op.get(1..).unwrap_or_default();
        match kind {
            b'w' => {
                let _written = w.write(&pattern(count(arg))).await;
            }
            b'x' => {
                let (n, err) = match w.write(&pattern(count(arg))).await {
                    Ok(n) => (n, None),
                    Err(e) => (0, Some(e)),
                };
                write_str(w, &format!("[n={n} err={}]", err_text(err))).await;
            }
            b'b' => {
                let _written = w.write(plan_body(arg)).await;
            }
            b'f' => {
                let _flushed = w.flush().await;
            }
            b's' => w.write_header(code(arg)),
            b't' => match arg {
                "nil" => w.header().insert_raw("Content-Type", Vec::new()),
                "empty" => w.header().set("Content-Type", ""),
                _ => w.header().set("Content-Type", format!("application/{arg}")),
            },
            b'l' => w.header().set("Content-Length", arg),
            b'c' => w.header().set("Connection", arg),
            _ => {}
        }
    }
}

/// Go: the main server's `/t/plan/:spec` handler.
fn plan<'a>(w: &'a mut dyn ResponseWriter, _r: &'a mut Request, ps: Params) -> BoxFuture<'a, ()> {
    Box::pin(async move { run_plan(w, ps.by_name("spec")).await })
}

/// Go: the admin server's `/t/plan/` handler.
fn admin_plan<'a>(w: &'a mut dyn ResponseWriter, r: &'a mut Request) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let spec = r.url.path.strip_prefix("/t/plan/").unwrap_or(&r.url.path);
        run_plan(w, spec).await;
    })
}

fn write_header_twice<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        w.write_header(201);
        w.write_header(202);
        write_str(w, "x").await;
    })
}

/// The `/t/header/:mode` modes that write the body themselves (Go's early `return`s).
async fn header_mode_writing(w: &mut dyn ResponseWriter, mode: &str) -> bool {
    match mode {
        "trailer" => {
            w.header().set("Trailer", "X-T1, X-T2");
            write_str(w, "body").await;
            w.header().set("X-T1", "v1");
            w.header().set("X-T2", "v2");
            w.header().set("X-Undeclared", "u");
        }
        "trailerbig" => {
            w.header().set("Trailer", "X-T1");
            let _written = w.write(&pattern(3000)).await;
            w.header().set("X-T1", "v1");
        }
        "trailerprefix" => {
            write_str(w, "body").await;
            w.header().set("Trailer:X-Late", "late");
        }
        "afterwrite" => {
            write_str(w, "x").await;
            w.header().set("X-After", "1");
        }
        "connupgrade" => {
            w.header().set("Connection", "Upgrade");
            w.header().set("Upgrade", "websocket");
            w.write_header(101);
        }
        "location" => {
            w.header().set("Location", "/elsewhere");
            w.write_header(302);
        }
        _ => return false,
    }
    true
}

/// The `/t/header/:mode` modes that only set headers, and the body they then write.
fn header_mode_setting(h: &mut Header, mode: &str) -> &'static [u8] {
    let one = |v: &str| vec![v.to_owned()];
    match mode {
        "noncanon" => {
            h.insert_raw("x-lower", one("v"));
            h.insert_raw("X-Upper", one("V"));
            h.insert_raw("x-Mixed_Case", one("m"));
        }
        "newline" => h.set("X-Nl", "a\nb\r\nc"),
        "badkey" => {
            h.insert_raw("Bad Key", one("v"));
            h.insert_raw("Bad:Key", one("v"));
            h.insert_raw("Good-Key", one("v"));
        }
        "multi" => {
            h.add("X-M", "1");
            h.add("X-M", "2");
            h.add("x-m", "3");
        }
        "empty" => {
            h.set("X-Empty", "");
            h.insert_raw("X-None", Vec::new());
        }
        "trailerhead" => {
            h.set("Trailer", "X-T1");
            h.set("X-T1", "early");
        }
        "date" => h.set("Date", "yesterday"),
        "emptyctype" => {
            h.insert_raw("Content-Type", Vec::new());
            return b"<html>";
        }
        "ce" => {
            h.set("Content-Encoding", "gzip");
            return b"<html>";
        }
        "space" => h.set("X-Sp", "  padded \t "),
        "connclose" => h.set("Connection", "close"),
        "connka" => h.set("Connection", "keep-alive"),
        "teidentity" => h.set("Transfer-Encoding", "identity"),
        "techunked" => h.set("Transfer-Encoding", "chunked"),
        "tegzip" => h.set("Transfer-Encoding", "gzip"),
        "clte" => {
            h.set("Content-Length", "1");
            h.set("Transfer-Encoding", "chunked");
        }
        "clbad" => h.set("Content-Length", "abc"),
        "clneg" => h.set("Content-Length", "-1"),
        "clzero" => {
            h.set("Content-Length", "0");
            return b"";
        }
        "ctype" => {
            h.set("Content-Type", "text/csv");
            return b"<html>";
        }
        "sniffhtml" => return b"  <!DOCTYPE html><p>",
        "sniffbin" => return b"\x00\x01\x02",
        "sniffpng" => return b"\x89PNG\x0D\x0A\x1A\x0A....",
        "sniffjson" => return b"{\"a\":1}",
        "sniffutf16" => return b"\xFE\xFFab",
        "none" => return b"",
        _ => {}
    }
    b"x"
}

fn header_mode<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let mode = ps.by_name("mode");
        if header_mode_writing(w, mode).await {
            return;
        }
        let body = header_mode_setting(w.header(), mode);
        let _written = w.write(body).await;
    })
}

fn panic_mode<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        match ps.by_name("mode") {
            "small" => write_str(w, "partial").await,
            "big" => {
                let _written = w.write(&pattern(3000)).await;
            }
            "flush" => {
                write_str(w, "flushed").await;
                let _flushed = w.flush().await;
            }
            "header" => w.write_header(202),
            _ => {}
        }
        resume_unwind(Box::new("boom"))
    })
}

fn error_mode<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        match ps.by_name("mode") {
            "plain" => error(w, "boom", 500).await,
            "headers" => {
                let h = w.header();
                h.set("Content-Length", "99");
                h.set("Content-Encoding", "gzip");
                h.set("Content-Type", "application/json");
                h.set("X-Keep", "k");
                error(w, "bad <request>", 400).await;
            }
            "notfound" => not_found(w).await,
            "multiline" => error(w, "line1\nline2", 418).await,
            _ => {}
        }
    })
}

fn redirect_to<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let q = r.url.query();
        if !q.get("ct").is_empty() {
            w.header().set("Content-Type", q.get("ct"));
        }
        redirect(w, r, q.get("to"), code(ps.by_name("code"))).await;
    })
}

fn echo_url<'a>(w: &'a mut dyn ResponseWriter, r: &'a mut Request) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let text = format!(
            "path={} rawpath={} uri={}",
            r.url.path, r.url.raw_path, r.request_uri
        );
        write_str(w, &text).await;
    })
}

fn body_mode<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let text = match ps.by_name("mode") {
            "none" => "ok".to_owned(),
            "some" => {
                let (n, err) = read_full(&mut r.body, 10).await;
                format!("n={n} err={}", err_text(err))
            }
            "all" => {
                let (body, err) = read_all(&mut r.body).await;
                format!("n={} err={}", body.len(), err_text(err))
            }
            mode @ ("max" | "maxok") => {
                let limit = if mode == "max" { 16 } else { 64 };
                let inner = mem::take(&mut r.body);
                r.body = max_bytes_reader(w.body_limit_notifier(), inner, limit);
                let (body, err) = read_all(&mut r.body).await;
                format!("n={} err={}", body.len(), err_text(err))
            }
            "close" => {
                let close_err = r.body.close().await.err();
                let (body, err) = read_all(&mut r.body).await;
                format!(
                    "close={} n={} err={}",
                    err_text(close_err),
                    body.len(),
                    err_text(err)
                )
            }
            "big" => {
                let _written = w.write(&pattern(3000)).await;
                return;
            }
            _ => return,
        };
        write_str(w, &text).await;
    })
}

fn form<'a>(w: &'a mut dyn ResponseWriter, r: &'a mut Request, _ps: Params) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let err = r.parse_form().await.err();
        let post = r
            .post_form
            .as_ref()
            .map_or_else(String::new, Values::encode);
        let all = r.form.as_ref().map_or_else(String::new, Values::encode);
        let text = format!("err={} post={post} form={all}", err_text(err));
        write_str(w, &text).await;
    })
}

fn cookie<'a>(w: &'a mut dyn ResponseWriter, r: &'a mut Request, _ps: Params) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let n = r.cookies().len();
        let text = match r.cookie("uids") {
            Err(e) => format!("err={e} count={n}"),
            Ok(c) => format!("value={} quoted={} count={n}", c.value, c.quoted),
        };
        write_str(w, &text).await;
    })
}

fn echo_request<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        // Go writes the two Fprintf calls separately, which shapes the chunks.
        let first = format!(
            "method={}\nuri={}\nurl={}\npath={}\nrawpath={}\nrawquery={}\nhost={}\n",
            r.method, r.request_uri, r.url, r.url.path, r.url.raw_path, r.url.raw_query, r.host
        );
        write_str(w, &first).await;
        let second = format!(
            "proto={} {}.{}\nclose={}\ncl={}\nte={}\nrest={}\nheader:{}\n",
            r.proto,
            r.proto_major,
            r.proto_minor,
            r.close,
            r.content_length,
            r.transfer_encoding.join(","),
            ps.by_name("rest"),
            format_header(Some(&r.header))
        );
        write_str(w, &second).await;
    })
}

fn remote_addr<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let text = format!("remote={}", quote(&r.remote_addr));
        write_str(w, &text).await;
    })
}

/// Go: `adminEcho(name)`.
#[derive(Debug, Clone, Copy)]
struct AdminEcho(&'static str);

impl Handler for AdminEcho {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let text = format!("{} {}", self.0, r.url.path);
            write_str(w, &text).await;
        })
    }
}

fn sleep_ms<'a>(
    w: &'a mut dyn ResponseWriter,
    _r: &'a mut Request,
    ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let ms = u64::try_from(atoi_or_zero(ps.by_name("ms"))).unwrap_or(0);
        sleep(Duration::from_millis(ms)).await;
        write_str(w, "late").await;
    })
}

/// Go: `hold`.
fn hold<'a>(w: &'a mut dyn ResponseWriter, _r: &'a mut Request, ps: Params) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let ms = u64::try_from(atoi_or_zero(ps.by_name("ms"))).unwrap_or(0);
        sleep(Duration::from_millis(ms)).await;
        write_str(w, "held").await;
    })
}

fn slow_body<'a>(
    w: &'a mut dyn ResponseWriter,
    r: &'a mut Request,
    _ps: Params,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let (body, err) = read_all(&mut r.body).await;
        let timeout = err.is_some_and(|e| e.to_string().contains("i/o timeout"));
        let text = format!("n={} timeout={timeout}", body.len());
        write_str(w, &text).await;
    })
}
