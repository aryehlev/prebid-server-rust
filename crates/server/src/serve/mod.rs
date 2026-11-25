//! Serving a router as server/server.go serves Prebid's handlers.
//!
//! Each of Prebid's servers is an `http.Server` (server/server.go:90-118): the main server on TCP
//! (or the Unix-socket server in its place) with `ReadTimeout` and `WriteTimeout` of 15 s, the
//! admin and Prometheus servers with neither, all with net/http's 1 MiB header limit, no body
//! limit and HTTP/1.1 only, since nothing configures HTTP/2. [`serve`] runs a router on
//! hyper-util's connection builder the same way: HTTP/1.1 only (`server.h2c`), the buffer for
//! Go's header limit (the `limits` glue), the [`Budget`] as hyper's header read timeout and
//! tower-http's request-body timeout, and half-closed connections kept open, so a handler runs to
//! its end when its client goes away, as Go's do (`server.client-disconnect`). There is no write
//! budget and no 408 or 503 timeout layer, so a late answer is still sent (`server.timeouts`).
//! What hyper does with a body the handler leaves unread is its own: it keeps the connection only
//! when one more read ends the body, where net/http reads up to 256 KiB of it
//! (`server.unread-body`). It closes a connection at once after a 431 or an unread body, where
//! net/http shuts the write side and waits 500 ms for a client still sending
//! (`server.lingering-close`).
//!
//! On the shutdown signal it stops accepting, closes the connections that wait for a request,
//! lets the others finish their answer (sent with `connection: close`) and waits at most
//! [`SHUTDOWN_TIMEOUT`] for them, as `shutdownAfterSignals` runs `Shutdown`
//! (server/server.go:196-207). hyper closes a connection that has sent nothing at once and
//! serves a request whose head was arriving, where Go waits up to 5 s for the first and drops the
//! second (`server.shutdown-new-connections`).

mod limits;

use std::{future::Future, io, pin::pin, time::Duration};

use axum::{Router, serve::Listener};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::{conn::auto::Builder, graceful::GracefulShutdown},
    service::TowerToHyperService,
};
use tokio::{
    signal::unix::{SignalKind, signal},
    time,
};
use tower_http::timeout::RequestBodyTimeoutLayer;

/// How long a shutdown waits for the connections still open: the deadline
/// `shutdownAfterSignals` gives `Shutdown` (server/server.go:199).
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// The read budget of one of Prebid's servers.
///
/// How long a request head may take to arrive (from when the connection is ready for it, so it
/// also bounds an idle keep-alive connection, as `ReadTimeout` does with `IdleTimeout` unset) and
/// how long the body may stall between two reads. net/http counts head and body against one
/// deadline; that and the write budget hyper has no equivalent for are `server.timeouts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The budget; `None` is no budget.
    read: Option<Duration>,
}

impl Budget {
    /// The main and Unix-socket servers' 15 s `ReadTimeout` (server/server.go:103-104 and
    /// 115-116).
    pub const MAIN: Self = Self::of(Duration::from_secs(15));
    /// The admin and Prometheus servers set no budget (server/server.go:90-95,
    /// server/prometheus.go): a connection may wait for its request forever.
    pub const NONE: Self = Self { read: None };

    /// A budget of `read`.
    #[must_use]
    pub const fn of(read: Duration) -> Self {
        Self { read: Some(read) }
    }

    /// The budget, or `None` for none.
    #[must_use]
    pub const fn read(self) -> Option<Duration> {
        self.read
    }
}

/// How [`serve`] stopped once its shutdown signal came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Every connection finished within [`SHUTDOWN_TIMEOUT`]: Go's `Shutdown` returns nil.
    Drained,
    /// [`SHUTDOWN_TIMEOUT`] passed with connections still open, which go on until the runtime
    /// stops: Go's `Shutdown` returns `context deadline exceeded`.
    TimedOut,
}

/// The signal that stops Prebid's servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignal {
    /// SIGTERM.
    Terminate,
    /// SIGINT.
    Interrupt,
}

/// Listens for SIGTERM and SIGINT, as `Listen` does with `signal.Notify` (server/server.go:23-24).
///
/// The handlers are installed when this returns, and the future completes at the first of the two
/// signals. Must be called within a Tokio runtime.
///
/// # Errors
///
/// When a signal handler cannot be installed.
pub fn shutdown_signal() -> io::Result<impl Future<Output = StopSignal>> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => StopSignal::Terminate,
            _ = interrupt.recv() => StopSignal::Interrupt,
        }
    })
}

/// Serves `router` on `listener` within `budget` until `shutdown` completes.
///
/// Then it shuts down as `shutdownAfterSignals` does: the listener is closed at once, connections
/// that wait for a request are closed, and those serving one finish it and close, for at most
/// [`SHUTDOWN_TIMEOUT`].
pub async fn serve<L, F>(listener: L, router: Router, budget: Budget, shutdown: F) -> Stopped
where
    L: Listener,
    F: Future<Output = ()>,
{
    serve_until(listener, router, budget, shutdown, SHUTDOWN_TIMEOUT).await
}

/// [`serve`] with a shutdown that waits at most `drain`.
async fn serve_until<L, F>(
    mut listener: L,
    router: Router,
    budget: Budget,
    shutdown: F,
    drain: Duration,
) -> Stopped
where
    L: Listener,
    F: Future<Output = ()>,
{
    let service = TowerToHyperService::new(app(router, budget));
    let builder = connections(budget);
    let graceful = GracefulShutdown::new();
    let mut shutdown = pin!(shutdown);
    loop {
        let (io, _remote) = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => accepted,
        };
        let connection = builder
            .serve_connection(TokioIo::new(io), service.clone())
            .into_owned();
        drop(tokio::spawn(graceful.watch(connection)));
    }
    drop(listener);
    time::timeout(drain, graceful.shutdown())
        .await
        .map_or(Stopped::TimedOut, |()| Stopped::Drained)
}

/// `router` with the limits and the body budget of a listener.
fn app(router: Router, budget: Budget) -> Router {
    let router = limits::without_body_limit(router);
    match budget.read {
        Some(read) => router.layer(RequestBodyTimeoutLayer::new(read)),
        None => router,
    }
}

/// The connection builder of a listener: HTTP/1.1 only, Go's header limit, half-closed
/// connections kept, and the budget's header read timeout (none when the budget is none, where
/// hyper's default would be 30 s).
fn connections(budget: Budget) -> Builder<TokioExecutor> {
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    // The HTTP/1 settings return the settings borrowed from `builder`, which is all there is.
    let _ = builder
        .http1()
        .timer(TokioTimer::new())
        .half_close(true)
        .max_buf_size(limits::MAX_BUF_SIZE)
        .header_read_timeout(budget.read);
    builder
}

#[cfg(test)]
mod tests {
    use std::{io, time::Duration};

    use axum::{Router, routing::get};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        runtime,
        sync::oneshot,
        task,
        time::{sleep as pause, timeout},
    };

    use super::{Budget, Stopped, serve_until};

    /// Serves a router whose one handler sleeps for `sleep`, sends a request to it, stops the
    /// server while the handler runs and returns how the server stopped and the answer the
    /// client read.
    fn stop_during(sleep: Duration, drain: Duration) -> io::Result<(Stopped, String)> {
        runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let address = listener.local_addr()?;
                let router = Router::new().route(
                    "/sleep",
                    get(move || async move {
                        pause(sleep).await;
                        "late"
                    }),
                );
                let (stop, stopped) = oneshot::channel::<()>();
                let server = task::spawn(serve_until(
                    listener,
                    router,
                    Budget::NONE,
                    async move { stopped.await.unwrap_or_default() },
                    drain,
                ));
                let mut client = TcpStream::connect(address).await?;
                client
                    .write_all(b"GET /sleep HTTP/1.1\r\nHost: pbs.example\r\n\r\n")
                    .await?;
                pause(Duration::from_millis(100)).await;
                stop.send(())
                    .map_err(|()| io::Error::other("the server stopped early"))?;
                let outcome = server.await?;
                let mut answer = Vec::new();
                let read = timeout(sleep * 2, client.read_to_end(&mut answer)).await;
                let ended = read.is_ok();
                let mut text = String::from_utf8_lossy(&answer).into_owned();
                if !ended {
                    text.push_str("<open>");
                }
                Ok((outcome, text))
            })
    }

    /// A handler that ends within the drain time is answered with `connection: close`, and the
    /// server reports it drained.
    // Go: Server.Shutdown waits for the active connections, which close after their answer
    // (net/http/server.go:3150-3223), from shutdownAfterSignals (server/server.go:196-207)
    #[test]
    fn drains_running_handlers() -> io::Result<()> {
        let (outcome, answer) = stop_during(Duration::from_millis(300), Duration::from_secs(5))?;
        if outcome != Stopped::Drained
            || !answer.starts_with("HTTP/1.1 200 OK\r\n")
            || !answer.contains("\r\nconnection: close\r\n")
            || !answer.ends_with("\r\n\r\nlate")
        {
            return Err(io::Error::other(format!("{outcome:?}: {answer:?}")));
        }
        Ok(())
    }

    /// A handler that outlasts the drain time leaves the server reporting it timed out; the
    /// connection goes on, and its answer is still sent.
    // Go: shutdownAfterSignals gives Shutdown a 10 s deadline and logs the error it returns when
    // the deadline passes first (server/server.go:196-207)
    #[test]
    fn times_out_on_long_handlers() -> io::Result<()> {
        let (outcome, answer) = stop_during(Duration::from_secs(1), Duration::from_millis(300))?;
        if outcome != Stopped::TimedOut || !answer.ends_with("\r\n\r\nlate") {
            return Err(io::Error::other(format!("{outcome:?}: {answer:?}")));
        }
        Ok(())
    }
}
