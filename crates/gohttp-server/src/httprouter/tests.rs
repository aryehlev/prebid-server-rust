//! Go: `httprouter/router_test.go`, case for case.
//!
//! Go's handle closures set captured flags; [`Probe`] records its calls instead. Go's
//! `mockResponseWriter` and `httptest.NewRecorder()` are both a [`ResponseRecorder`], and
//! `TestRouterServeFiles` serves a real directory where Go uses a mock file system that only
//! records that it was opened.

use core::{
    error::Error,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{
    env, fs,
    panic::resume_unwind,
    path::PathBuf,
    process,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use pbs_gocompat::url::Url;

use super::{Handle, PanicHandler, Param, Params, Router, params_from_context};
use crate::{
    BoxFuture, Dir, Handler, HandlerFunc, PanicPayload, Request, ResponseWriter,
    httptest::ResponseRecorder,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Go: `http.NewRequest(method, target, nil)`. The URL is `url.Parse(target)`, so targets a
/// server would reject (`""`, `/../path`) reach the router as they do in Go's tests.
fn request(method: &str, target: &str) -> Result<Request, Box<dyn Error>> {
    Ok(Request {
        method: method.to_owned(),
        url: Url::parse(target)?,
        ..Request::default()
    })
}

/// Go: `router.ServeHTTP(w, req)` into a new `httptest.ResponseRecorder`.
async fn serve(
    router: &Router,
    method: &str,
    target: &str,
) -> Result<ResponseRecorder, Box<dyn Error>> {
    let mut req = request(method, target)?;
    let mut w = ResponseRecorder::new();
    router.serve_http(&mut w, &mut req).await;
    Ok(w)
}

/// [`serve`], for a response the test does not look at.
async fn run(router: &Router, method: &str, target: &str) -> TestResult {
    let _w = serve(router, method, target).await?;
    Ok(())
}

/// Serves the request and checks the status code and the `Allow` header.
async fn expect_allow(
    router: &Router,
    method: &str,
    target: &str,
    code: u16,
    allow: &str,
) -> TestResult {
    let w = serve(router, method, target).await?;
    if w.code() != code {
        return Err(format!(
            "{method} {target} handling failed: Code={}, Header={:?}",
            w.code(),
            w.header_map()
        )
        .into());
    }
    let got = w.header_map().get("Allow");
    if got != allow {
        return Err(format!("unexpected Allow header value: {got}").into());
    }
    Ok(())
}

fn params(ps: &[(&str, &str)]) -> Params {
    Params(
        ps.iter()
            .map(|&(key, value)| Param {
                key: key.to_owned(),
                value: value.to_owned(),
            })
            .collect(),
    )
}

/// Go: the handles and handlers of `router_test.go`. Records the parameters of each call (as a
/// [`Handler`], those in the request context), then writes `code` and `body` when set.
#[derive(Debug, Clone, Default)]
struct Probe {
    calls: Arc<Mutex<Vec<Params>>>,
    code: Option<u16>,
    body: &'static str,
}

impl Probe {
    fn replying(code: u16, body: &'static str) -> Self {
        Self {
            code: Some(code),
            body,
            ..Self::default()
        }
    }

    fn calls(&self) -> Result<Vec<Params>, Box<dyn Error>> {
        Ok(self.calls.lock().map_err(|e| e.to_string())?.to_vec())
    }

    fn hit(&self) -> Result<bool, Box<dyn Error>> {
        Ok(!self.calls()?.is_empty())
    }

    fn reply<'a>(&'a self, w: &'a mut dyn ResponseWriter, ps: Params) -> BoxFuture<'a, ()> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(ps);
        }
        Box::pin(async move {
            if let Some(code) = self.code {
                w.write_header(code);
            }
            if !self.body.is_empty() {
                let _written = w.write(self.body.as_bytes()).await;
            }
        })
    }
}

impl Handle for Probe {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        self.reply(w, ps)
    }
}

impl Handler for Probe {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        self.reply(w, params_from_context(&r.ctx))
    }
}

/// Go: `func(...) { panic("oops!") }`.
#[derive(Debug)]
struct Panics;

impl Handle for Panics {
    fn handle<'a>(
        &'a self,
        _w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
        _ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async { resume_unwind(Box::new("oops!")) })
    }
}

/// A [`PanicHandler`] from a closure.
fn panic_handler<F>(f: F) -> PanicHandler
where
    F: for<'a> Fn(&'a mut dyn ResponseWriter, &'a mut Request, PanicPayload) -> BoxFuture<'a, ()>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

/// Whether [`handler_func`] ran (Go: the `handlerFunc` flag of `TestRouterAPI`).
static HANDLER_FUNC_HIT: AtomicBool = AtomicBool::new(false);

/// Go: the `router.HandlerFunc` function of `TestRouterAPI`.
fn handler_func<'a>(_w: &'a mut dyn ResponseWriter, _r: &'a mut Request) -> BoxFuture<'a, ()> {
    HANDLER_FUNC_HIT.store(true, Ordering::SeqCst);
    Box::pin(async {})
}

/// A directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, Box<dyn Error>> {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .subsec_nanos();
        let dir = env::temp_dir().join(format!("pbs-httprouter-{}-{nanos}", process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn params_by_name() -> TestResult {
    // Go: TestParams.
    let ps = params(&[
        ("param1", "value1"),
        ("param2", "value2"),
        ("param3", "value3"),
    ]);
    for p in &ps.0 {
        let val = ps.by_name(&p.key);
        if val != p.value {
            return Err(format!("Wrong value for {}: Got {val}; Want {}", p.key, p.value).into());
        }
    }
    let missing = ps.by_name("noKey");
    if !missing.is_empty() {
        return Err(format!("Expected empty string for not found key; got: {missing}").into());
    }
    Ok(())
}

#[tokio::test]
async fn router_routes() -> TestResult {
    // Go: TestRouter.
    let mut router = Router::new();
    let probe = Probe::default();
    router.handle("GET", "/user/:name", probe.clone())?;
    run(&router, "GET", "/user/gopher").await?;
    let calls = probe.calls()?;
    if calls.is_empty() {
        return Err("routing failed".into());
    }
    let want = vec![params(&[("name", "gopher")])];
    if calls != want {
        return Err(format!("wrong wildcard values: want {want:?}, got {calls:?}").into());
    }
    Ok(())
}

#[tokio::test]
async fn router_api() -> TestResult {
    // Go: TestRouterAPI. The port registers functions through `handler` (Go's
    // `router.HandlerFunc` is `router.Handler` with an `http.HandlerFunc`).
    let get = Probe::default();
    let head = Probe::default();
    let options = Probe::default();
    let post = Probe::default();
    let put = Probe::default();
    let patch = Probe::default();
    let delete = Probe::default();
    let handler = Probe::default();

    let mut router = Router::new();
    router.get("/GET", get.clone())?;
    router.head("/GET", head.clone())?;
    router.options("/GET", options.clone())?;
    router.post("/POST", post.clone())?;
    router.put("/PUT", put.clone())?;
    router.patch("/PATCH", patch.clone())?;
    router.delete("/DELETE", delete.clone())?;
    router.handler("GET", "/Handler", Arc::new(handler.clone()))?;
    router.handler("GET", "/HandlerFunc", Arc::new(HandlerFunc(handler_func)))?;

    for (method, path, probe, what) in [
        ("GET", "/GET", &get, "GET"),
        ("HEAD", "/GET", &head, "HEAD"),
        ("OPTIONS", "/GET", &options, "OPTIONS"),
        ("POST", "/POST", &post, "POST"),
        ("PUT", "/PUT", &put, "PUT"),
        ("PATCH", "/PATCH", &patch, "PATCH"),
        ("DELETE", "/DELETE", &delete, "DELETE"),
        ("GET", "/Handler", &handler, "Handler"),
    ] {
        run(&router, method, path).await?;
        if !probe.hit()? {
            return Err(format!("routing {what} failed").into());
        }
    }
    run(&router, "GET", "/HandlerFunc").await?;
    if !HANDLER_FUNC_HIT.load(Ordering::SeqCst) {
        return Err("routing HandlerFunc failed".into());
    }
    Ok(())
}

#[test]
fn router_root() -> TestResult {
    // Go: TestRouterRoot.
    let mut router = Router::new();
    match router.get("noSlashRoot", Probe::default()) {
        Ok(()) => Err("registering path not beginning with '/' did not panic".into()),
        Err(e) if e.0 == "path must begin with '/' in path 'noSlashRoot'" => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[tokio::test]
async fn router_chaining() -> TestResult {
    // Go: TestRouterChaining.
    let mut router1 = Router::new();
    let mut router2 = Router::new();
    let foo = Probe::replying(200, "");
    router1.post("/foo", foo.clone())?;
    let bar = Probe::replying(200, "");
    router2.post("/bar", bar.clone())?;
    router1.set_not_found(Some(Arc::new(router2)));

    let w_foo = serve(&router1, "POST", "/foo").await?;
    if w_foo.code() != 200 || !foo.hit()? {
        return Err("Regular routing failed with router chaining.".into());
    }
    let w_bar = serve(&router1, "POST", "/bar").await?;
    if w_bar.code() != 200 || !bar.hit()? {
        return Err("Chained routing failed with router chaining.".into());
    }
    let w_qax = serve(&router1, "POST", "/qax").await?;
    if w_qax.code() != 404 {
        return Err("NotFound behavior failed with router chaining.".into());
    }
    Ok(())
}

#[tokio::test]
async fn router_options() -> TestResult {
    // Go: TestRouterOPTIONS.
    let mut router = Router::new();
    router.post("/path", Probe::default())?;

    // test not allowed
    // * (server)
    expect_allow(&router, "OPTIONS", "*", 200, "OPTIONS, POST").await?;
    // path
    expect_allow(&router, "OPTIONS", "/path", 200, "OPTIONS, POST").await?;
    let w_missing = serve(&router, "OPTIONS", "/doesnotexist").await?;
    if w_missing.code() != 404 {
        return Err(format!(
            "OPTIONS handling failed: Code={}, Header={:?}",
            w_missing.code(),
            w_missing.header_map()
        )
        .into());
    }

    // add another method
    router.get("/path", Probe::default())?;
    // set a global OPTIONS handler (it adjusts the status code to 204)
    router.set_global_options(Some(Arc::new(Probe::replying(204, ""))));

    // test again
    expect_allow(&router, "OPTIONS", "*", 204, "GET, OPTIONS, POST").await?;
    expect_allow(&router, "OPTIONS", "/path", 204, "GET, OPTIONS, POST").await?;

    // custom handler
    let custom = Probe::default();
    router.options("/path", custom.clone())?;

    // test again
    expect_allow(&router, "OPTIONS", "*", 204, "GET, OPTIONS, POST").await?;
    if custom.hit()? {
        return Err("custom handler called on *".into());
    }
    let w_custom = serve(&router, "OPTIONS", "/path").await?;
    if w_custom.code() != 200 {
        return Err(format!("OPTIONS handling failed: Code={}", w_custom.code()).into());
    }
    if !custom.hit()? {
        return Err("custom handler not called".into());
    }
    Ok(())
}

#[tokio::test]
async fn router_not_allowed() -> TestResult {
    // Go: TestRouterNotAllowed.
    let mut router = Router::new();
    router.post("/path", Probe::default())?;

    // test not allowed
    expect_allow(&router, "GET", "/path", 405, "OPTIONS, POST").await?;

    // add another method
    router.delete("/path", Probe::default())?;
    router.options("/path", Probe::default())?; // must be ignored

    // test again
    expect_allow(&router, "GET", "/path", 405, "DELETE, OPTIONS, POST").await?;

    // test custom handler
    let response_text = "custom method";
    router.set_method_not_allowed(Some(Arc::new(Probe::replying(418, response_text))));
    let w = serve(&router, "GET", "/path").await?;
    let got = w.body_string();
    if got != response_text {
        return Err(format!("unexpected response got {got:?} want {response_text:?}").into());
    }
    if w.code() != 418 {
        return Err(format!("unexpected response code {} want 418", w.code()).into());
    }
    let allow = w.header_map().get("Allow");
    if allow != "DELETE, OPTIONS, POST" {
        return Err(format!("unexpected Allow header value: {allow}").into());
    }
    Ok(())
}

#[tokio::test]
async fn router_not_found() -> TestResult {
    // Go: TestRouterNotFound.
    let mut router = Router::new();
    router.get("/path", Probe::default())?;
    router.get("/dir/", Probe::default())?;
    router.get("/", Probe::default())?;

    for (route, code, location) in [
        ("/path/", 301, "/path"),   // TSR -/
        ("/dir", 301, "/dir/"),     // TSR +/
        ("", 301, "/"),             // TSR +/
        ("/PATH", 301, "/path"),    // Fixed Case
        ("/DIR/", 301, "/dir/"),    // Fixed Case
        ("/PATH/", 301, "/path"),   // Fixed Case -/
        ("/DIR", 301, "/dir/"),     // Fixed Case +/
        ("/../path", 301, "/path"), // CleanPath
        ("/nope", 404, ""),         // NotFound
    ] {
        let w = serve(&router, "GET", route).await?;
        let got = w.header_map().get("Location");
        if w.code() != code || (w.code() != 404 && got != location) {
            return Err(format!(
                "NotFound handling route {route} failed: Code={}, Header={got}",
                w.code()
            )
            .into());
        }
    }

    // Test custom not found handler
    let not_found = Probe::replying(404, "");
    router.set_not_found(Some(Arc::new(not_found.clone())));
    let w_custom = serve(&router, "GET", "/nope").await?;
    if w_custom.code() != 404 || !not_found.hit()? {
        return Err(format!(
            "Custom NotFound handler failed: Code={}, Header={:?}",
            w_custom.code(),
            w_custom.header_map()
        )
        .into());
    }

    // Test other method than GET (want 307 instead of 301)
    router.patch("/path", Probe::default())?;
    let w_patch = serve(&router, "PATCH", "/path/").await?;
    let header: Vec<(&str, &[String])> = w_patch.header_map().iter().collect();
    if w_patch.code() != 307 || header != [("Location", &["/path".to_owned()][..])] {
        return Err(format!(
            "Custom NotFound handler failed: Code={}, Header={:?}",
            w_patch.code(),
            w_patch.header_map()
        )
        .into());
    }

    // Test special case where no node for the prefix "/" exists
    let mut no_root = Router::new();
    no_root.get("/a", Probe::default())?;
    let w_root = serve(&no_root, "GET", "/").await?;
    if w_root.code() != 404 {
        return Err(format!("NotFound handling route / failed: Code={}", w_root.code()).into());
    }
    Ok(())
}

#[tokio::test]
async fn router_panic_handler() -> TestResult {
    // Go: TestRouterPanicHandler.
    let mut router = Router::new();
    let panic_handled = Arc::new(AtomicBool::new(false));
    let handled = Arc::clone(&panic_handled);
    router.set_panic_handler(Some(panic_handler(move |_w, _r, p| {
        // Go: `p` is the value the handle panicked with.
        if p.downcast_ref::<&str>() == Some(&"oops!") {
            handled.store(true, Ordering::SeqCst);
        }
        Box::pin(async {})
    })));
    router.handle("PUT", "/user/:name", Panics)?;

    // A panic escaping ServeHTTP fails the test, as Go's deferred recover does.
    run(&router, "PUT", "/user/gopher").await?;
    if !panic_handled.load(Ordering::SeqCst) {
        return Err("simulating failed".into());
    }
    Ok(())
}

#[tokio::test]
async fn router_lookup() -> TestResult {
    // Go: TestRouterLookup.
    let want_handle = Probe::default();
    let want_params = params(&[("name", "gopher")]);
    let mut router = Router::new();

    // try empty router first
    let empty = router.lookup("GET", "/nope");
    if empty.handle.is_some() {
        return Err(format!("Got handle for unregistered pattern: {empty:?}").into());
    }
    if empty.tsr {
        return Err("Got wrong TSR recommendation!".into());
    }

    // insert route and try again
    router.get("/user/:name", want_handle.clone())?;
    let found = router.lookup("GET", "/user/gopher");
    let Some(handle) = &found.handle else {
        return Err("Got no handle!".into());
    };
    // Go calls `handle(nil, nil, nil)`.
    let mut req = request("GET", "/")?;
    let mut w = ResponseRecorder::new();
    handle.handle(&mut w, &mut req, Params::default()).await;
    if !want_handle.hit()? {
        return Err("Routing failed!".into());
    }
    if found.params != want_params {
        return Err(format!(
            "Wrong parameter values: want {want_params:?}, got {:?}",
            found.params
        )
        .into());
    }

    let slash = router.lookup("GET", "/user/gopher/");
    if slash.handle.is_some() {
        return Err(format!("Got handle for unregistered pattern: {slash:?}").into());
    }
    if !slash.tsr {
        return Err("Got no TSR recommendation!".into());
    }

    let nope = router.lookup("GET", "/nope");
    if nope.handle.is_some() {
        return Err(format!("Got handle for unregistered pattern: {nope:?}").into());
    }
    if nope.tsr {
        return Err("Got wrong TSR recommendation!".into());
    }
    Ok(())
}

#[tokio::test]
async fn router_params_from_context() -> TestResult {
    // Go: TestRouterParamsFromContext.
    let with_params = Probe::default();
    let nil_params = Probe::default();
    let mut router = Router::new();
    router.handler("GET", "/user", Arc::new(nil_params.clone()))?;
    router.handler("GET", "/user/:name", Arc::new(with_params.clone()))?;

    run(&router, "GET", "/user/gopher").await?;
    let want = vec![params(&[("name", "gopher")])];
    let got = with_params.calls()?;
    if got != want {
        return Err(format!("Wrong parameter values: want {want:?}, got {got:?}").into());
    }

    run(&router, "GET", "/user").await?;
    let got_nil = nil_params.calls()?;
    if got_nil != [Params::default()] {
        return Err(format!("Wrong parameter values: want [], got {got_nil:?}").into());
    }
    Ok(())
}

#[tokio::test]
async fn router_serve_files() -> TestResult {
    // Go: TestRouterServeFiles.
    let dir = TempDir::new()?;
    fs::write(dir.0.join("favicon.ico"), b"icon")?;
    let root = Dir(dir.0.to_string_lossy().into_owned());
    let mut router = Router::new();

    match router.serve_files("/noFilepath", root.clone()) {
        Ok(()) => {
            return Err("registering path not ending with '*filepath' did not panic".into());
        }
        Err(e) if e.0 == "path must end with /*filepath in path '/noFilepath'" => {}
        Err(e) => return Err(e.into()),
    }

    router.serve_files("/*filepath", root)?;
    let w = serve(&router, "GET", "/favicon.ico").await?;
    if w.code() != 200 || w.body() != b"icon" {
        return Err(format!("serving file failed: {} {:?}", w.code(), w.body_string()).into());
    }
    Ok(())
}
