//! Prebid Server's own HTTP pieces built on this crate: `router.NoCache`,
//! `router.SupportCORS`, and the `http.Server` settings of `server/server.go`.

use core::time::Duration;
use std::sync::Arc;

use crate::{
    BoxFuture, Handler, Request, ResponseWriter, ServerConfig,
    cors::{Cors, CorsHandler, Options},
};

/// Go: the `ReadTimeout` and `WriteTimeout` of the main and Unix-socket servers.
pub const MAIN_SERVER_TIMEOUT: Duration = Duration::from_secs(15);

/// Go: the timeout `shutdownAfterSignals` gives `server.Shutdown`.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Go: `router.NoCache`: adds headers forbidding caching, then serves `handler`.
#[derive(Debug, Clone, Default)]
pub struct NoCache<H> {
    /// Go: `Handler`.
    pub handler: H,
}

impl<H: Handler> Handler for NoCache<H> {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        let h = w.header();
        h.add("Cache-Control", "no-cache, no-store, must-revalidate");
        h.add("Pragma", "no-cache");
        h.add("Expires", "0");
        self.handler.serve_http(w, r)
    }
}

/// Go: `router.SupportCORS(handler)`: credentialed CORS for every origin, allowing the
/// `Origin`, `X-Requested-With`, `Content-Type` and `Accept` request headers.
pub fn support_cors<H: Handler>(handler: H) -> CorsHandler<H> {
    Cors::new(Options {
        allow_credentials: true,
        allow_origin_func: Some(Arc::new(|_: &str| true)),
        allowed_headers: ["Origin", "X-Requested-With", "Content-Type", "Accept"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ..Options::default()
    })
    .handler(handler)
}

/// Go: `newMainServer` (and `newSocketServer`, whose `addr` is the socket path): 15 second
/// read and write timeouts. Response compression is a separate wrapper of `handler`.
#[must_use]
pub fn main_server(addr: &str, handler: Arc<dyn Handler>) -> ServerConfig {
    let mut cfg = ServerConfig::new(addr, handler);
    cfg.read_timeout = MAIN_SERVER_TIMEOUT;
    cfg.write_timeout = MAIN_SERVER_TIMEOUT;
    cfg
}

/// Go: `newAdminServer`: no timeouts.
#[must_use]
pub fn admin_server(addr: &str, handler: Arc<dyn Handler>) -> ServerConfig {
    ServerConfig::new(addr, handler)
}
