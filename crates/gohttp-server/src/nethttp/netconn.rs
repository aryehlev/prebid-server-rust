//! The server side of one accepted connection: Go's `net.Conn` as `net/http` uses it, with
//! read and write deadlines, `CloseWrite` and `Close`, and Go's `*net.OpError` texts.

use core::{
    fmt,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::{io, sync::Arc};

use pbs_logger::goerr::errno_text;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpStream, UnixStream, tcp, unix},
    sync::Notify,
    time::{Instant, timeout_at},
};

/// An error from a connection read, in Go's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NetError {
    /// Go: `io.EOF`.
    Eof,
    /// A `*net.OpError` whose `Timeout()` is true (a deadline passed).
    Timeout(String),
    /// Any other `*net.OpError` (reset, closed connection, ...).
    Op(String),
}

impl NetError {
    /// Go: `err.Error()`.
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Eof => "EOF",
            Self::Timeout(t) | Self::Op(t) => t,
        }
    }

    pub(super) fn into_io(self) -> io::Error {
        match self {
            Self::Eof => io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"),
            Self::Timeout(t) => io::Error::new(io::ErrorKind::TimedOut, t),
            Self::Op(t) => io::Error::other(t),
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// Go: the `Net` of the listener (`"tcp"` or `"unix"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Network {
    Tcp,
    Unix,
}

impl Network {
    const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Unix => "unix",
        }
    }
}

/// Addresses of a connection, for `*net.OpError` texts and `Request.RemoteAddr`.
#[derive(Debug, Clone)]
pub(super) struct ConnAddrs {
    pub(super) network: Network,
    pub(super) local: String,
    pub(super) remote: String,
}

impl ConnAddrs {
    /// Go: `(&net.OpError{Op: op, Net: net, Source: laddr, Addr: raddr, Err: err}).Error()`.
    fn op_error(&self, op: &str, err: &str) -> String {
        let mut s = format!("{op} {}", self.network.name());
        if !self.local.is_empty() {
            s.push(' ');
            s.push_str(&self.local);
        }
        // A Unix peer is an unnamed `*net.UnixAddr`, which is not nil (Go prints `->`).
        if !self.remote.is_empty() || self.network == Network::Unix {
            s.push_str(if self.local.is_empty() { " " } else { "->" });
            s.push_str(&self.remote);
        }
        s.push_str(": ");
        s.push_str(err);
        s
    }

    fn timeout(&self, op: &str) -> String {
        self.op_error(op, "i/o timeout")
    }

    fn closed(&self, op: &str) -> String {
        self.op_error(op, "use of closed network connection")
    }

    /// Go wraps syscall errors in `*os.SyscallError{Syscall: op}` (`read: connection reset by
    /// peer`).
    fn syscall(&self, op: &str, err: &io::Error) -> String {
        self.op_error(op, &format!("{op}: {}", errno_text(err)))
    }
}

/// Go: `rwc.Close()` from another goroutine (`Server.Shutdown` closing idle connections): every
/// blocked and later read fails with `use of closed network connection`.
#[derive(Debug, Default)]
pub(super) struct Closer {
    closed: AtomicBool,
    notify: Notify,
    /// The number of `Close` calls (the second one fails in Go).
    closes: AtomicU8,
}

impl Closer {
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Records one `Close` call; true for the first (Go: the second returns
    /// `use of closed network connection`).
    pub(super) fn record_close(&self) -> bool {
        self.closes.fetch_add(1, Ordering::SeqCst) == 0
    }

    pub(super) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.is_closed() {
                return;
            }
            notified.await;
        }
    }
}

/// The read half of a connection.
#[derive(Debug)]
pub(super) enum ReadHalf {
    Tcp(tcp::OwnedReadHalf),
    Unix(unix::OwnedReadHalf),
}

/// The write half of a connection.
#[derive(Debug)]
pub(super) enum WriteHalf {
    Tcp(tcp::OwnedWriteHalf),
    Unix(unix::OwnedWriteHalf),
}

/// A connection before it is split.
#[derive(Debug)]
pub(super) enum Stream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Stream {
    pub(super) fn split(self) -> (ReadHalf, WriteHalf) {
        match self {
            Self::Tcp(s) => {
                let (r, w) = s.into_split();
                (ReadHalf::Tcp(r), WriteHalf::Tcp(w))
            }
            Self::Unix(s) => {
                let (r, w) = s.into_split();
                (ReadHalf::Unix(r), WriteHalf::Unix(w))
            }
        }
    }
}

/// Reads with Go's deadline and close semantics.
#[derive(Debug)]
pub(super) struct NetReader {
    half: ReadHalf,
    pub(super) deadline: Option<Instant>,
    addrs: Arc<ConnAddrs>,
    closer: Arc<Closer>,
}

impl NetReader {
    pub(super) const fn new(half: ReadHalf, addrs: Arc<ConnAddrs>, closer: Arc<Closer>) -> Self {
        Self {
            half,
            deadline: None,
            addrs,
            closer,
        }
    }

    /// Go: `rwc.Read(p)`: `Ok(0)` is never returned for a non-empty `p`; end of stream is
    /// [`NetError::Eof`].
    pub(super) async fn read(&mut self, p: &mut [u8]) -> Result<usize, NetError> {
        if self.closer.is_closed() {
            return Err(NetError::Op(self.addrs.closed("read")));
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(NetError::Timeout(self.addrs.timeout("read")));
        }
        let read = async {
            match &mut self.half {
                ReadHalf::Tcp(h) => h.read(p).await,
                ReadHalf::Unix(h) => h.read(p).await,
            }
        };
        let deadline = self.deadline;
        let timed = async {
            match deadline {
                Some(d) => timeout_at(d, read).await.ok(),
                None => Some(read.await),
            }
        };
        let result = tokio::select! {
            biased;
            () = self.closer.wait() => return Err(NetError::Op(self.addrs.closed("read"))),
            r = timed => r,
        };
        match result {
            None => Err(NetError::Timeout(self.addrs.timeout("read"))),
            Some(Ok(0)) => Err(NetError::Eof),
            Some(Ok(n)) => Ok(n),
            Some(Err(e)) => Err(NetError::Op(self.addrs.syscall("read", &e))),
        }
    }
}

/// Writes with Go's deadline semantics.
#[derive(Debug)]
pub(super) struct NetWriter {
    half: Option<WriteHalf>,
    pub(super) deadline: Option<Instant>,
    addrs: Arc<ConnAddrs>,
}

impl NetWriter {
    pub(super) const fn new(half: WriteHalf, addrs: Arc<ConnAddrs>) -> Self {
        Self {
            half: Some(half),
            deadline: None,
            addrs,
        }
    }

    /// Go: `rwc.Write(p)`: all of `p` or an error.
    pub(super) async fn write_all(&mut self, p: &[u8]) -> Result<(), String> {
        let addrs = Arc::clone(&self.addrs);
        let Some(half) = self.half.as_mut() else {
            return Err(addrs.closed("write"));
        };
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(addrs.timeout("write"));
        }
        let write = async {
            match half {
                WriteHalf::Tcp(h) => h.write_all(p).await,
                WriteHalf::Unix(h) => h.write_all(p).await,
            }
        };
        let result = match self.deadline {
            Some(d) => timeout_at(d, write).await.ok(),
            None => Some(write.await),
        };
        match result {
            None => Err(addrs.timeout("write")),
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(addrs.syscall("write", &e)),
        }
    }

    /// Go: `(*net.TCPConn).CloseWrite`.
    pub(super) async fn close_write(&mut self) {
        if let Some(half) = self.half.as_mut() {
            let _ignored = match half {
                WriteHalf::Tcp(h) => h.shutdown().await,
                WriteHalf::Unix(h) => h.shutdown().await,
            };
        }
    }

    /// Go: `rwc.Close()` for the write side: the descriptor closes once the read half is gone,
    /// without a separate FIN (dropping an owned write half would shut it down first).
    pub(super) fn close(&mut self) {
        match self.half.take() {
            Some(WriteHalf::Tcp(h)) => h.forget(),
            Some(WriteHalf::Unix(h)) => h.forget(),
            None => {}
        }
    }
}
