//! What the test files share.
//!
//! The Go goldens of `testdata/golden` (written by `testdata/go`), lines of tab-separated fields
//! whose values are hex; the currency file decode and Go's rendering of its outcome; the
//! generator Go's seeded corpora come from; loopback servers that answer with the bytes Go's
//! servers did; the fetch replayed against them as `testdata/go/fetch.go` ran it; and checks
//! that return errors.

use std::{
    error::Error,
    fmt::Debug,
    fs,
    io::{self, BufRead as _, BufReader, Write as _},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use pbs_currency::{ConversionRates, RateConverter, Rates, default_http_client};
use pbs_http_client::ProxyEnv;
use pbs_jsonutil::unmarshal_valid;
use pbs_logger::{Capture, Severity};
use tokio::runtime;

/// A failure.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// What every test returns.
pub type TestResult<T = ()> = Result<T, BoxError>;

/// Fails with `message` unless `condition` holds.
///
/// # Errors
///
/// `condition` is false.
pub fn check(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Fails, showing both values, unless they are equal.
///
/// # Errors
///
/// They differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}:\n  got  {actual:?}\n  want {expected:?}").into())
    }
}

/// The lines of `testdata/golden/<name>`, each split at its tabs.
///
/// # Errors
///
/// The file cannot be read or is not UTF-8.
pub fn golden(name: &str) -> TestResult<Vec<Vec<String>>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/golden")
        .join(name);
    let text = fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(text
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect())
}

/// The bytes of a hex field (Go: `hex.EncodeToString`).
///
/// # Errors
///
/// `hex` is not hex.
pub fn unhex(hex: &str) -> TestResult<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return Err(format!("odd hex length in {hex:?}").into());
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
        .collect()
}

/// Lower-case hex of `bytes` (Go: `hex.EncodeToString`).
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0F])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect()
}

/// Go: `fnv64a`, FNV-1a's 64-bit hash of `data` in hex.
#[must_use]
pub fn fnv64a(data: &[u8]) -> String {
    let hash = data.iter().fold(0xCBF2_9CE4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    });
    format!("{hash:016x}")
}

/// Go's `dumpConversions`: `nil`, or each currency in key order, as hex, with `nil` or its rates
/// as hex and the bits of each rate.
#[must_use]
pub fn dump(conversions: Option<&ConversionRates>) -> String {
    let Some(conversions) = conversions else {
        return "nil".to_owned();
    };
    let currencies: Vec<String> = conversions
        .iter()
        .map(|(from, inner)| {
            let rendered = inner.as_ref().map_or_else(
                || "nil;".to_owned(),
                |rates| {
                    let pairs: Vec<String> = rates
                        .iter()
                        .map(|(to, rate)| {
                            format!("{}={:016x},", hex(to.as_bytes()), rate.to_bits())
                        })
                        .collect();
                    format!("{{{}}};", pairs.concat())
                },
            );
            format!("{}:{rendered}", hex(from.as_bytes()))
        })
        .collect();
    format!("{{{}}}", currencies.concat())
}

/// How the fetch decodes `data` (`pbs_jsonutil::unmarshal_valid` into [`Rates`]), rendered as
/// Go's `decodeOutcome` renders jsoniter's: `err`, or `ok:` and [`dump`] of the rates.
#[must_use]
pub fn decode(data: &[u8]) -> String {
    match unmarshal_valid::<Rates>(data) {
        Ok(rates) => format!("ok:{}", dump(rates.conversions.as_ref())),
        Err(_) => "err".to_owned(),
    }
}

/// splitmix64, the generator `testdata/go/random.go` draws its corpora from (Go:
/// `splitmix64`).
#[derive(Debug, Clone, Copy)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Go: `&splitmix64{state: seed}`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Go: `next`.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Go: `below(n)`, `next() % n`.
    pub fn below(&mut self, n: usize) -> usize {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next_u64().checked_rem(n).unwrap_or_default()).unwrap_or_default()
    }

    /// Go: `pick`: one of `options`, a copy.
    pub fn pick(&mut self, options: &[Vec<u8>]) -> Vec<u8> {
        let index = self.below(options.len());
        options.get(index).cloned().unwrap_or_default()
    }

    /// Go: `digits`: `count` random decimal digits.
    pub fn digits(&mut self, count: usize) -> Vec<u8> {
        (0..count)
            .map(|_| b'0' + u8::try_from(self.below(10)).unwrap_or_default())
            .collect()
    }
}

/// The text of a hex field.
///
/// # Errors
///
/// `hex` is not hex, or its bytes are not UTF-8.
pub fn unhex_text(hex: &str) -> TestResult<String> {
    Ok(String::from_utf8(unhex(hex)?)?)
}

/// `text` with every loopback port written as `PORT` (Go: `withPorts`).
#[must_use]
pub fn with_ports(text: &str) -> String {
    const HOST: &str = "127.0.0.1:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once(HOST) {
        out.push_str(before);
        out.push_str(HOST);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push_str("PORT");
        }
        rest = after.get(digits..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// What a server does on a connection once it has read the request's head (Go: `fetchAnswer`).
///
/// It writes `write`, then, when `wait` is set, waits for the client to close the connection;
/// then it closes the connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// The bytes written.
    pub write: Vec<u8>,
    /// Whether the server waits for the client to close the connection.
    pub wait: bool,
}

/// What one call of `Run` gave: its error, if any, and the lines it logged, each with glog's
/// letter for its severity, loopback ports written as `PORT`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    /// The error's text.
    pub error: Option<String>,
    /// The lines logged: the severity's letter and the message.
    pub logs: Vec<(String, String)>,
}

/// One scenario of `fetch.golden` (Go: `fetchScenario`), with what Go's fetch did in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scenario {
    /// The scenario's name.
    pub name: String,
    /// The converter's URL, `PORT` standing for the server's port.
    pub url: String,
    /// The converter's `httpTimeout`.
    pub timeout: Duration,
    /// The converter's `staleRatesThreshold`.
    pub stale: Duration,
    /// The answers, one per connection, the last one repeated; none: nothing listens. A proxied
    /// scenario's are the proxy's.
    pub answers: Vec<Answer>,
    /// Each call of `Run`.
    pub runs: Vec<Run>,
    /// Whether the converter served the constant rates after the calls.
    pub constant: bool,
    /// The request heads the server read, loopback ports written as `PORT`.
    pub heads: Vec<String>,
}

impl Scenario {
    /// Whether its request goes through the proxy, which answers it (Go: `viaProxy`): its host
    /// is not on loopback, so the proxy environment applies.
    #[must_use]
    pub fn via_proxy(&self) -> bool {
        self.name.starts_with("proxy")
    }
}

/// The scenario called `name`.
fn scenario<'s>(scenarios: &'s mut [Scenario], name: &str) -> TestResult<&'s mut Scenario> {
    scenarios
        .iter_mut()
        .find(|scenario| scenario.name == name)
        .ok_or_else(|| format!("no scenario {name}").into())
}

/// Call `run` of the scenario called `name`.
fn run<'s>(scenarios: &'s mut [Scenario], name: &str, run: &str) -> TestResult<&'s mut Run> {
    let index: usize = run.parse()?;
    scenario(scenarios, name)?
        .runs
        .get_mut(index)
        .ok_or_else(|| format!("{name} has no run {index}").into())
}

/// The scenarios of `fetch.golden`, with Go's outcomes.
///
/// # Errors
///
/// The golden cannot be read, or a line is not one `writeFetch` writes.
pub fn fetch_golden() -> TestResult<Vec<Scenario>> {
    let mut scenarios: Vec<Scenario> = Vec::new();
    for fields in golden("fetch.golden")? {
        let Some((kind, rest)) = fields.split_first() else {
            continue;
        };
        match (kind.as_str(), rest) {
            ("proxy", [url]) => check_eq(
                unhex_text(url)?.as_str(),
                "http://127.0.0.1:PORT",
                "the proxy",
            )?,
            ("scenario", [name, url, timeout, stale, runs]) => scenarios.push(Scenario {
                name: name.clone(),
                url: unhex_text(url)?,
                timeout: Duration::from_millis(timeout.parse()?),
                stale: Duration::from_nanos(stale.parse()?),
                runs: vec![Run::default(); runs.parse()?],
                ..Scenario::default()
            }),
            ("answer", [name, _index, wait, write]) => {
                scenario(&mut scenarios, name)?.answers.push(Answer {
                    write: unhex(write)?,
                    wait: wait == "1",
                });
            }
            ("result", [name, index, outcome]) if outcome == "ok" => {
                run(&mut scenarios, name, index)?.error = None;
            }
            ("result", [name, index, outcome, text]) if outcome == "err" => {
                run(&mut scenarios, name, index)?.error = Some(unhex_text(text)?);
            }
            ("log", [name, index, severity, text]) => {
                let line = (severity.clone(), unhex_text(text)?);
                run(&mut scenarios, name, index)?.logs.push(line);
            }
            ("rates", [name, constant]) => {
                scenario(&mut scenarios, name)?.constant = constant.parse()?;
            }
            ("head", [name, _index, head]) => {
                let head = unhex_text(head)?;
                scenario(&mut scenarios, name)?.heads.push(head);
            }
            _ => return Err(format!("not a line of fetch.golden: {fields:?}").into()),
        }
    }
    Ok(scenarios)
}

/// What the Rust fetch did in a scenario: [`Scenario`]'s outcomes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replayed {
    /// Each call of [`RateConverter::run`].
    pub runs: Vec<Run>,
    /// Whether the converter served the constant rates after the calls.
    pub constant: bool,
    /// The request heads the server read, loopback ports written as `PORT`.
    pub heads: Vec<String>,
}

/// glog's letter for `severity`.
fn letter(severity: Severity) -> String {
    severity.name().chars().take(1).collect()
}

/// Replays every scenario as `writeFetch` ran it.
///
/// Loopback servers answer with Go's bytes, the proxy environment names a loopback proxy, which
/// only the proxied scenarios' hosts go through, and a converter on [`default_http_client`]'s
/// client runs as often as Go's did, its lines captured. Each call runs on this thread, where
/// the capture is, and the client's connections on a worker, which closes them once their
/// request is gone, as Go's Transport does.
///
/// # Errors
///
/// A server, the client or the runtime cannot start, or a server fails.
pub fn replay(scenarios: &[Scenario]) -> TestResult<Vec<Replayed>> {
    let proxy_answers = scenarios
        .iter()
        .find(|scenario| scenario.via_proxy())
        .map_or_else(Vec::new, |scenario| scenario.answers.clone());
    let proxy = RawServer::start(proxy_answers)?;
    let proxy_url = format!("http://127.0.0.1:{}", proxy.port());
    let client = default_http_client(ProxyEnv::from_lookup(|name| {
        (name == "HTTP_PROXY").then(|| proxy_url.clone())
    }))?;
    let runtime = runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let mut replayed = Vec::new();
    for scenario in scenarios {
        let server = if scenario.via_proxy() || scenario.answers.is_empty() {
            None
        } else {
            Some(RawServer::start(scenario.answers.clone())?)
        };
        let proxied = proxy.heads().len();
        let port = match &server {
            Some(server) => server.port(),
            None => closed_port()?,
        };
        let converter = RateConverter::new(
            client.clone(),
            scenario.timeout,
            scenario.url.replace("PORT", &port.to_string()),
            scenario.stale,
        );
        let mut runs = Vec::new();
        for _ in &scenario.runs {
            let capture = Capture::new();
            let result = capture.run(|| runtime.block_on(converter.run()));
            runs.push(Run {
                error: result.err().map(|err| with_ports(&err.to_string())),
                logs: capture
                    .lines()
                    .into_iter()
                    .map(|line| (letter(line.severity), with_ports(&line.message)))
                    .collect(),
            });
        }
        let mut heads = match server {
            Some(server) => server.close()?,
            None => Vec::new(),
        };
        if scenario.via_proxy() {
            heads.extend(proxy.heads().into_iter().skip(proxied));
        }
        replayed.push(Replayed {
            runs,
            constant: converter.rates().get_rates().is_none(),
            heads: heads.iter().map(|head| with_ports(head)).collect(),
        });
    }
    let _proxy_heads = proxy.close()?;
    Ok(replayed)
}

/// A loopback port nothing listens on (Go: `closedPort`).
///
/// # Errors
///
/// No port can be bound.
pub fn closed_port() -> TestResult<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A loopback server that answers its connections in turn with its answers, the last one again
/// for any after it, and keeps the request heads it read (Go: `rawServer`).
#[derive(Debug)]
pub struct RawServer {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
    stopping: Arc<AtomicBool>,
    acceptor: JoinHandle<io::Result<()>>,
}

impl RawServer {
    /// A server listening on a loopback port of its own.
    ///
    /// # Errors
    ///
    /// No port can be bound.
    pub fn start(answers: Vec<Answer>) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let heads = Arc::clone(&heads);
            let stopping = Arc::clone(&stopping);
            thread::spawn(move || accept(&listener, &answers, &heads, &stopping))
        };
        Ok(Self {
            port,
            heads,
            stopping,
            acceptor,
        })
    }

    /// The port it listens on.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The request heads read so far, in the order the connections came.
    #[must_use]
    pub fn heads(&self) -> Vec<String> {
        lock(&self.heads).clone()
    }

    /// Stops listening, waits for the connections to end and returns the request heads.
    ///
    /// # Errors
    ///
    /// A connection failed.
    pub fn close(self) -> TestResult<Vec<String>> {
        self.stopping.store(true, Ordering::SeqCst);
        // Wake the acceptor, which then sees that it is stopping.
        drop(TcpStream::connect((Ipv4Addr::LOCALHOST, self.port))?);
        self.acceptor
            .join()
            .map_err(|panic| format!("the acceptor panicked: {panic:?}"))??;
        Ok(lock(&self.heads).clone())
    }
}

/// Answers the connections until the server stops, then waits for them to end.
fn accept(
    listener: &TcpListener,
    answers: &[Answer],
    heads: &Arc<Mutex<Vec<String>>>,
    stopping: &AtomicBool,
) -> io::Result<()> {
    let mut connections = Vec::new();
    loop {
        let (conn, _peer) = listener.accept()?;
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        let answer = answers
            .get(connections.len())
            .or_else(|| answers.last())
            .cloned()
            .unwrap_or_default();
        let heads = Arc::clone(heads);
        connections.push(thread::spawn(move || serve(conn, &answer, &heads)));
    }
    for connection in connections {
        connection
            .join()
            .map_err(|panic| io::Error::other(format!("a connection panicked: {panic:?}")))??;
    }
    Ok(())
}

/// Reads the request's head, through its blank line, and answers (Go: `readHead` and the
/// connection's goroutine).
fn serve(conn: TcpStream, answer: &Answer, heads: &Mutex<Vec<String>>) -> io::Result<()> {
    let mut reader = BufReader::new(conn);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        head.push_str(&line);
        if read == 0 || line == "\r\n" {
            break;
        }
    }
    lock(heads).push(head);
    let stream = reader.get_mut();
    stream.write_all(&answer.write)?;
    if answer.wait {
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        // Until the client closes the connection, or the timeout ends the wait, as in Go.
        let _drained = io::copy(stream, &mut io::sink());
    }
    Ok(())
}
