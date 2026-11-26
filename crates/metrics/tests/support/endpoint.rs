//! A raw HTTP/1.1 endpoint on 127.0.0.1 (Go: `fakeEndpoint` in `testdata/go/influx.go`).
//!
//! It reads requests (a head up to the blank line, then a body of the head's `Content-Length`),
//! records their bytes and answers the n-th request of a connection with the n-th response, the
//! last one once they run out. An empty response closes the connection without answering; one
//! that starts with U+0004 is written without it, and the connection is closed after it.

use std::{
    io::{self, BufRead as _, BufReader, ErrorKind, Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

/// The requests one endpoint read, in the order they arrived, with when they did.
type Requests = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

/// A listening endpoint; it stops accepting when dropped.
#[derive(Debug)]
pub struct Endpoint {
    addr: SocketAddr,
    requests: Requests,
    closed: Arc<AtomicBool>,
}

impl Endpoint {
    /// Go: `newFakeEndpoint(responses...)`.
    ///
    /// # Errors
    ///
    /// When it cannot listen.
    pub fn new(responses: Vec<Vec<u8>>) -> io::Result<Self> {
        Self::for_addr(|_| responses)
    }

    /// Go: `newFakeEndpointFor(responses)`: the responses that `responses` returns for the
    /// endpoint's address, which they can name.
    ///
    /// # Errors
    ///
    /// When it cannot listen.
    pub fn for_addr<F: FnOnce(&str) -> Vec<Vec<u8>>>(responses: F) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let requests = Requests::default();
        let closed = Arc::new(AtomicBool::new(false));
        let accepted = Arc::clone(&requests);
        let stop = Arc::clone(&closed);
        let responses = Arc::new(responses(&addr.to_string()));
        drop(
            thread::Builder::new()
                .name("fake-endpoint".to_owned())
                .spawn(move || {
                    for conn in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        let Ok(conn) = conn else {
                            continue;
                        };
                        let kept = Arc::clone(&accepted);
                        let answers = Arc::clone(&responses);
                        drop(thread::spawn(move || handle(conn, &answers, &kept)));
                    }
                })?,
        );
        Ok(Self {
            addr,
            requests,
            closed,
        })
    }

    /// The address, `127.0.0.1:<port>`.
    #[must_use]
    pub fn addr(&self) -> String {
        self.addr.to_string()
    }

    /// The requests read so far.
    #[must_use]
    pub fn requests(&self) -> Vec<String> {
        self.arrivals()
            .into_iter()
            .map(|(_, request)| request)
            .collect()
    }

    /// The requests read so far, with when each one was read in full.
    #[must_use]
    pub fn arrivals(&self) -> Vec<(Instant, String)> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(at, request)| (*at, String::from_utf8_lossy(request).into_owned()))
            .collect()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // Wakes the accepting thread, which then sees the flag.
        drop(TcpStream::connect(self.addr));
    }
}

/// Go: `fakeEndpoint.handle`: one connection.
fn handle(conn: TcpStream, responses: &[Vec<u8>], requests: &Mutex<Vec<(Instant, Vec<u8>)>>) {
    let Ok(mut writer) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(conn);
    for n in 0.. {
        let Ok(request) = read_request(&mut reader) else {
            return;
        };
        requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Instant::now(), request));
        let Some(response) = responses.get(n).or_else(|| responses.last()) else {
            return;
        };
        if response.is_empty() {
            return;
        }
        let (last, bytes) = response
            .strip_prefix(b"\x04")
            .map_or((false, response.as_slice()), |rest| (true, rest));
        if writer.write_all(bytes).is_err() || last {
            return;
        }
    }
}

/// Go: `readRequest`: the head up to the blank line and a body of its `Content-Length`.
fn read_request(reader: &mut BufReader<TcpStream>) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut length = 0;
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        request.extend_from_slice(&line);
        if line == b"\r\n" {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        if let Some((name, value)) = text.trim_end_matches(['\r', '\n']).split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            length = value.trim().parse().map_err(io::Error::other)?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    request.extend_from_slice(&body);
    Ok(request)
}

/// Go: `refusedAddr`: an address nothing listens on.
///
/// # Errors
///
/// When it cannot listen to find one.
pub fn refused_addr() -> io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.to_string())
}
