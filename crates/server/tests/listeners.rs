//! pbs-server's listeners: the Unix socket `unix_socket_enable` asks for in place of TCP, and
//! the SIGTERM and SIGINT that stop the servers, as server/server.go's `Listen` sets them up
//! (server/server.go:22-88). The signal test sends the signals to this test binary, which runs
//! only these tests.

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt::Debug,
        fs,
        path::Path,
        process::{self, Command},
        time::Duration,
    };

    use axum::{Router, routing::get};
    use pbs_server::{Budget, StopSignal, Stopped, serve, shutdown_signal};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream, UnixListener, UnixStream},
        runtime::{self, Runtime},
        sync::oneshot,
        task, time,
    };

    /// A failed test.
    type BoxError = Box<dyn Error + Send + Sync>;

    /// What the tests return.
    type TestResult<T = ()> = Result<T, BoxError>;

    /// The request both tests send.
    const REQUEST: &[u8] =
        b"GET /status HTTP/1.1\r\nHost: pbs.example\r\nConnection: close\r\n\r\n";

    /// Fails, showing both values, unless `got` equals `want`.
    fn ensure_eq<A: PartialEq<B> + Debug + ?Sized, B: Debug + ?Sized>(
        got: &A,
        want: &B,
        what: &str,
    ) -> TestResult {
        if got == want {
            Ok(())
        } else {
            Err(format!("{what}:\n got {got:#?}\nwant {want:#?}").into())
        }
    }

    /// A router with one route, `/status`, answering `ok`.
    fn status() -> Router {
        Router::new().route("/status", get(|| async { "ok" }))
    }

    /// A runtime with the IO, time and signal drivers.
    fn runtime() -> TestResult<Runtime> {
        Ok(runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?)
    }

    /// Whether an answer is the status route's 200.
    fn answered(answer: &[u8]) -> bool {
        answer.starts_with(b"HTTP/1.1 200 OK\r\n") && answer.ends_with(b"\r\n\r\nok")
    }

    /// The main server on the Unix socket `unix_socket_name` names (server/server.go:33-43):
    /// served within the main server's budget, it answers on the socket and drains on shutdown.
    #[test]
    fn unix_socket() -> TestResult {
        let socket = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pbs-server-{}.sock", process::id()));
        if socket.exists() {
            fs::remove_file(&socket)?;
        }
        let path = socket.clone();
        let (stopped, answer) = runtime()?.block_on(async move {
            let listener = UnixListener::bind(&path)?;
            let (stop, stop_signal) = oneshot::channel::<()>();
            let server = task::spawn(serve(listener, status(), Budget::MAIN, async move {
                stop_signal.await.unwrap_or_default();
            }));
            let mut client = UnixStream::connect(&path).await?;
            client.write_all(REQUEST).await?;
            let mut answer = Vec::new();
            let _read =
                time::timeout(Duration::from_secs(5), client.read_to_end(&mut answer)).await??;
            stop.send(()).map_err(|()| "the server stopped early")?;
            Ok::<_, BoxError>((server.await?, answer))
        })?;
        fs::remove_file(socket)?;
        ensure_eq(&stopped, &Stopped::Drained, "the shutdown")?;
        if answered(&answer) {
            Ok(())
        } else {
            Err(format!("the answer {:?}", String::from_utf8_lossy(&answer)).into())
        }
    }

    /// Sends `signal` (a `kill` signal name) to this process.
    fn send(signal: &str) -> TestResult {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(process::id().to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("kill -{signal} failed: {status}").into())
        }
    }

    /// SIGTERM and SIGINT each stop a server waiting on [`shutdown_signal`], which tells them
    /// apart, as `Listen`'s `signal.Notify(stopSignals, syscall.SIGTERM, syscall.SIGINT)` does
    /// (server/server.go:23-24); the server answered before the signal and drains.
    #[test]
    fn stops_on_signals() -> TestResult {
        for (signal, want) in [
            ("TERM", StopSignal::Terminate),
            ("INT", StopSignal::Interrupt),
        ] {
            let (stopped, which, answer) = runtime()?.block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let address = listener.local_addr()?;
                let stop = shutdown_signal()?;
                let (seen, which) = oneshot::channel::<StopSignal>();
                let server = task::spawn(serve(listener, status(), Budget::MAIN, async move {
                    let _sent = seen.send(stop.await);
                }));
                let mut client = TcpStream::connect(address).await?;
                client.write_all(REQUEST).await?;
                let mut answer = Vec::new();
                let _read = time::timeout(Duration::from_secs(5), client.read_to_end(&mut answer))
                    .await??;
                send(signal)?;
                let stopped = time::timeout(Duration::from_secs(5), server).await??;
                Ok::<_, BoxError>((stopped, which.await?, answer))
            })?;
            ensure_eq(&which, &want, signal)?;
            ensure_eq(&stopped, &Stopped::Drained, signal)?;
            if !answered(&answer) {
                return Err(format!("the answer {:?}", String::from_utf8_lossy(&answer)).into());
            }
        }
        Ok(())
    }
}
