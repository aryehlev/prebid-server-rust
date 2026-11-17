//! Go: `router/router_test.go` (`TestCORSSupport`, `TestNoCache`) and the server settings of
//! `server/server_test.go` (`TestNewSocketServer`, `TestNewMainServer`, `TestNewAdminServer`).
//! Go's server constructors build `Addr` from the configuration; here the caller passes it.

use core::time::Duration;
use std::sync::Arc;

use pbs_gocompat::url::Url;

use super::{NoCache, admin_server, main_server, support_cors};
use crate::{BoxFuture, Handler, Request, ResponseWriter, httptest::ResponseRecorder};

/// Go: `http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {})`.
#[derive(Debug, Clone, Copy)]
struct Nothing;

impl Handler for Nothing {
    fn serve_http<'a>(
        &'a self,
        _w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Go: `http.NewRequest(method, target, nil)`.
fn request(method: &str, target: &str) -> Result<Request, String> {
    Ok(Request {
        method: method.to_owned(),
        url: Url::parse(target).map_err(|e| e.to_string())?,
        ..Request::default()
    })
}

#[tokio::test]
async fn cors_support() -> Result<(), String> {
    // Go: TestCORSSupport ("Prevents #648").
    const ORIGIN: &str = "https://publisher-domain.com";
    let cors = support_cors(Nothing);
    let mut rr = ResponseRecorder::new();
    let mut req = request("OPTIONS", "http://some-domain.com/openrtb2/auction")?;
    req.header.set("Access-Control-Request-Method", "POST");
    req.header.set("Access-Control-Request-Headers", "origin");
    req.header.set("Origin", ORIGIN);
    cors.serve_http(&mut rr, &mut req).await;
    let got = rr.header_map().get("Access-Control-Allow-Origin");
    if got != ORIGIN {
        return Err(format!(
            "Access-Control-Allow-Origin: {got:?}, want {ORIGIN:?}"
        ));
    }
    Ok(())
}

#[tokio::test]
async fn no_cache() -> Result<(), String> {
    // Go: TestNoCache.
    let nc = NoCache { handler: Nothing };
    let mut rw = ResponseRecorder::new();
    let mut req = request("GET", "http://localhost/nocache")?;
    req.header.set("ETag", "abcdef");
    nc.serve_http(&mut rw, &mut req).await;
    let h = rw.header_map();
    for (name, expected) in [
        ("Cache-Control", "no-cache, no-store, must-revalidate"),
        ("Pragma", "no-cache"),
        ("Expires", "0"),
        ("ETag", ""),
    ] {
        let got = h.get(name);
        if got != expected {
            return Err(format!(
                "invalid {name} header: expected: {expected} got: {got}"
            ));
        }
    }
    Ok(())
}

#[test]
fn new_socket_server() -> Result<(), String> {
    // Go: TestNewSocketServer.
    const MOCK_SOCKET: &str = "socket_addr:socket_port";
    let ret = main_server(MOCK_SOCKET, Arc::new(Nothing));
    let timeouts = (ret.read_timeout, ret.write_timeout);
    if ret.addr != MOCK_SOCKET || timeouts != (Duration::from_secs(15), Duration::from_secs(15)) {
        return Err(format!("{}: {timeouts:?}", ret.addr));
    }
    Ok(())
}

#[test]
fn new_main_server() -> Result<(), String> {
    // Go: TestNewMainServer (`cfg.Host + ":" + strconv.Itoa(cfg.Port)`).
    let ret = main_server("prebid.com:8000", Arc::new(Nothing));
    let timeouts = (ret.read_timeout, ret.write_timeout);
    if ret.addr != "prebid.com:8000"
        || timeouts != (Duration::from_secs(15), Duration::from_secs(15))
    {
        return Err(format!("{}: {timeouts:?}", ret.addr));
    }
    Ok(())
}

#[test]
fn new_admin_server() -> Result<(), String> {
    // Go: TestNewAdminServer (no timeouts: the zero values of `http.Server`).
    let ret = admin_server("prebid.com:6060", Arc::new(Nothing));
    let timeouts = (
        ret.read_timeout,
        ret.read_header_timeout,
        ret.write_timeout,
        ret.idle_timeout,
    );
    if ret.addr != "prebid.com:6060"
        || timeouts
            != (
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            )
    {
        return Err(format!("{}: {timeouts:?}", ret.addr));
    }
    Ok(())
}
