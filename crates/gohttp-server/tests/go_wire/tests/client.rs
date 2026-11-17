//! The client of `testdata/go/wire/main.go`: `runAll`, `runCase` and `normalize`.

use core::time::Duration;
use std::{
    collections::BTreeMap,
    io::ErrorKind,
    sync::{Arc, OnceLock},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, split},
    net::{TcpStream, UnixStream},
    sync::Semaphore,
    time::{Instant, sleep, timeout_at},
};

use super::handlers::Target;

/// Go: `parallel`.
const PARALLEL: usize = 64;

/// One thing the client does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// Send these bytes.
    Send(Vec<u8>),
    /// Wait this many milliseconds.
    Pause(u64),
}

/// One case of the golden file.
#[derive(Debug, Clone, Default)]
pub(super) struct Case {
    pub(super) name: String,
    pub(super) server: String,
    pub(super) wait: u64,
    pub(super) steps: Vec<Step>,
    pub(super) response: Vec<u8>,
    pub(super) end: String,
}

/// What the client saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) response: Vec<u8>,
    pub(super) end: String,
}

impl Outcome {
    fn new(got: &[u8], end: &str) -> Self {
        Self {
            response: normalize(got),
            end: end.to_owned(),
        }
    }
}

/// Go: `runAll`.
pub(super) async fn run_all(
    targets: Arc<BTreeMap<&'static str, Target>>,
    cases: Arc<[Case]>,
) -> Vec<Outcome> {
    let sem = Arc::new(Semaphore::new(PARALLEL));
    let mut tasks = Vec::with_capacity(cases.len());
    for i in 0..cases.len() {
        let (sem, targets, cases) = (Arc::clone(&sem), Arc::clone(&targets), Arc::clone(&cases));
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let Some(case) = cases.get(i) else {
                return Outcome::new(b"", "no case");
            };
            match targets.get(case.server.as_str()) {
                Some(target) => run_case(target, case).await,
                None => Outcome::new(b"", "unknown server"),
            }
        }));
    }
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(
            t.await
                .unwrap_or_else(|e| Outcome::new(b"", &format!("task failed: {e}"))),
        );
    }
    out
}

/// Go: `runCase`.
async fn run_case(target: &Target, case: &Case) -> Outcome {
    match target {
        Target::Tcp(addr) => match TcpStream::connect(addr.as_str()).await {
            Ok(stream) => {
                let _nodelay = stream.set_nodelay(true);
                drive(stream, case).await
            }
            Err(_e) => Outcome::new(b"", "dial error"),
        },
        Target::Unix(path) => match UnixStream::connect(path).await {
            Ok(stream) => drive(stream, case).await,
            Err(_e) => Outcome::new(b"", "dial error"),
        },
    }
}

/// Sends the steps on `stream` while reading the reply.
async fn drive<S>(stream: S, case: &Case) -> Outcome
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut rd, mut wr) = split(stream);
    let steps = case.steps.clone();
    let sent_at = Arc::new(OnceLock::new());
    let sent = Arc::clone(&sent_at);
    let writer = tokio::spawn(async move {
        for step in &steps {
            match step {
                Step::Pause(ms) => sleep(Duration::from_millis(*ms)).await,
                Step::Send(data) => {
                    if wr.write_all(data).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _first = sent.set(Instant::now());
        // The write half stays open (dropping it does not shut the socket down either way).
        wr
    });
    let outcome = read_reply(&mut rd, &sent_at, Duration::from_millis(case.wait)).await;
    writer.abort();
    let _half = writer.await;
    outcome
}

async fn read_reply<R: AsyncRead + Unpin>(
    rd: &mut R,
    sent_at: &OnceLock<Instant>,
    wait: Duration,
) -> Outcome {
    let mut got = Vec::new();
    let mut buf = vec![0_u8; 64 << 10];
    let mut last_read = Instant::now();
    loop {
        let finished = sent_at.get().copied();
        let quiet_from = finished.map_or(last_read, |t| t.max(last_read));
        if finished.is_some() && quiet_from.elapsed() >= wait {
            return Outcome::new(&got, "open");
        }
        let deadline = if finished.is_some() {
            quiet_from + wait
        } else {
            Instant::now() + wait
        };
        match timeout_at(deadline, rd.read(&mut buf)).await {
            Err(_elapsed) => {}
            Ok(Ok(0)) => return Outcome::new(&got, "eof"),
            Ok(Ok(n)) => {
                got.extend_from_slice(buf.get(..n).unwrap_or_default());
                last_read = Instant::now();
            }
            Ok(Err(e)) if e.kind() == ErrorKind::ConnectionReset => {
                return Outcome::new(&got, "reset");
            }
            Ok(Err(_e)) => return Outcome::new(&got, "error"),
        }
    }
}

/// Go: `isHTTPDate`.
fn is_http_date(b: &[u8]) -> bool {
    const SHAPE: &[u8] = b"Aaa, 00 Aaa 0000 00:00:00 GMT";
    b.len() == SHAPE.len()
        && b.iter().zip(SHAPE).all(|(&c, &s)| match s {
            b'A' => c.is_ascii_uppercase(),
            b'a' => c.is_ascii_lowercase(),
            b'0' => c.is_ascii_digit(),
            _ => c == s,
        })
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// Go: `normalize`.
pub(super) fn normalize(b: &[u8]) -> Vec<u8> {
    const DATE: &[u8] = b"\r\nDate: ";
    const DATE_LEN: usize = 29;
    const MARKER: &[u8] = b"boundary=";
    let mut out = b.to_vec();
    let mut i = 0;
    while let Some(j) = find(&out, DATE, i) {
        let start = j + DATE.len();
        let end = start + DATE_LEN;
        let is_date = out.get(start..end).is_some_and(is_http_date)
            && out.get(end..end + 2) == Some(b"\r\n".as_slice());
        if is_date {
            let tail = out.split_off(end);
            out.truncate(start);
            out.extend_from_slice(b"<date>");
            out.extend_from_slice(&tail);
        }
        i = start;
    }
    let mut boundaries: Vec<Vec<u8>> = Vec::new();
    let mut k = 0;
    while let Some(j) = find(&out, MARKER, k) {
        let start = j + MARKER.len();
        if let Some(bd) = out.get(start..start + 60)
            && bd
                .iter()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        {
            boundaries.push(bd.to_vec());
        }
        k = start;
    }
    for bd in boundaries {
        out = replace_all(&out, &bd, b"<boundary>");
    }
    out
}

fn replace_all(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut rest = hay;
    while let Some(i) = find(rest, from, 0) {
        out.extend_from_slice(rest.get(..i).unwrap_or_default());
        out.extend_from_slice(to);
        rest = rest.get(i + from.len()..).unwrap_or_default();
    }
    out.extend_from_slice(rest);
    out
}
