//! Pins of pbs-server's listeners against net/http as server/server.go builds them.
//!
//! `testdata/golden/wire.json` is what `testdata/go/wire` read from newMainServer,
//! newSocketServer and newAdminServer on loopback: header order and case, framing, sniffed
//! content types, the header limit and its edge, header counts, long request targets and the
//! bytes of request targets, malformed requests, the HTTP/2 preface, the read and write budgets
//! (scaled to 1 s), bodies a handler leaves unread, clients that go away, gziphandler's Vary and
//! graceful shutdown. Each case is one connection and a client script of
//! writes, pauses and closes; the same scripts run here against the same stand-in handlers on
//! axum, served by [`pbs_server::serve`] with the budget of the Go server they stand for, and what
//! the client read is described as the generator describes Go's answer.

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        fmt::Debug,
        fs,
        future::poll_fn,
        io::{ErrorKind, Read as _},
        iter,
        net::SocketAddr,
        pin::{Pin, pin},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Router,
        body::{Body, Bytes, HttpBody as _},
        extract::Query,
        http::header,
        response::{AppendHeaders, IntoResponse as _, Response},
        routing::any,
    };
    use flate2::{Crc, read::GzDecoder};
    use hyper_util::{
        rt::{TokioExecutor, TokioIo},
        server::conn::auto,
        service::TowerToHyperService,
    };
    use pbs_server::{Budget, SHUTDOWN_TIMEOUT, Stopped, compression, serve};
    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{
            TcpListener, TcpStream,
            tcp::{ReadHalf, WriteHalf},
        },
        runtime::{self, Runtime},
        sync::oneshot,
        task::{self, JoinHandle},
        time::{self, Instant},
    };
    use tower_http::timeout::TimeoutError;

    /// A failed pin.
    type BoxError = Box<dyn Error + Send + Sync>;

    /// What the pins return.
    type TestResult<T = ()> = Result<T, BoxError>;

    /// The query of a stand-in's request.
    type Params = Query<BTreeMap<String, String>>;

    /// The marks `/sleep` sets and `/mark` reads.
    type Marks = Arc<Mutex<BTreeSet<String>>>;

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

    /// The value at `pointer` in `doc`.
    fn at<'v>(doc: &'v Value, pointer: &str) -> TestResult<&'v Value> {
        doc.pointer(pointer)
            .ok_or_else(|| format!("the golden has no {pointer}").into())
    }

    /// The string at `pointer` in `doc`.
    fn text<'v>(doc: &'v Value, pointer: &str) -> TestResult<&'v str> {
        at(doc, pointer)?
            .as_str()
            .ok_or_else(|| format!("{pointer} is not a string").into())
    }

    /// The unsigned integer at `pointer` in `doc`, 0 when there is none (Go leaves zero fields
    /// out).
    fn count(doc: &Value, pointer: &str) -> TestResult<u64> {
        doc.pointer(pointer).map_or(Ok(0), |value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{pointer} is not an unsigned integer").into())
        })
    }

    /// The array at `pointer` in `doc`.
    fn list<'v>(doc: &'v Value, pointer: &str) -> TestResult<&'v [Value]> {
        at(doc, pointer)?
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{pointer} is not an array").into())
    }

    /// `testdata/golden/wire.json`, its header checked: written by the wire generator on amd64
    /// (D-ARCH).
    fn golden() -> TestResult<Value> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/wire.json");
        let doc: Value = serde_json::from_slice(&fs::read(path)?)?;
        ensure_eq(
            text(&doc, "/header/generator")?,
            "crates/server/testdata/go/wire",
            "the golden's generator",
        )?;
        ensure_eq(
            text(&doc, "/header/goarch")?,
            "amd64",
            "the golden's goarch",
        )?;
        Ok(doc)
    }

    /// What one step of a script does before its pause.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Action {
        /// Nothing: the step only pauses.
        Wait,
        /// Writes the bytes.
        Write(Vec<u8>),
        /// Writes `n` bytes of `a`, in writes of `chunk` bytes `gap` apart when `chunk` is set.
        Fill {
            /// How many bytes.
            n: usize,
            /// The bytes of one write; 0 is one write.
            chunk: usize,
            /// The pause after each write.
            gap: Duration,
        },
        /// Closes the connection, with a reset when set (`SO_LINGER` 0), and stops reading.
        Close(bool),
    }

    /// One step of a client script: its action, then a pause.
    #[derive(Debug, Clone)]
    struct Step {
        /// What the step does.
        action: Action,
        /// The pause after it.
        pause: Duration,
    }

    /// One connection of the golden: the server it goes to, the client's script, and how long
    /// the client reads after its last step.
    #[derive(Debug, Clone)]
    struct Case {
        /// The case's group.
        group: String,
        /// The case's name.
        name: String,
        /// The server: main, `main_gzip`, scaled, admin or shutdown.
        server: String,
        /// The script.
        steps: Vec<Step>,
        /// How long the client reads after its last step.
        window: Duration,
    }

    impl Case {
        /// Reads a case of the golden.
        fn read(case: &Value) -> TestResult<Self> {
            let steps = list(case, "/steps")?
                .iter()
                .map(|step| {
                    let fill = usize::try_from(count(step, "/fill")?)?;
                    let action = if let Ok(close) = text(step, "/close") {
                        Action::Close(close == "rst")
                    } else if let Ok(bytes) = text(step, "/text") {
                        Action::Write(bytes.as_bytes().to_vec())
                    } else if let Ok(hex) = text(step, "/hex") {
                        Action::Write(unhex(hex)?)
                    } else if fill > 0 {
                        Action::Fill {
                            n: fill,
                            chunk: usize::try_from(count(step, "/chunk")?)?,
                            gap: Duration::from_millis(count(step, "/chunk_pause_ms")?),
                        }
                    } else {
                        Action::Wait
                    };
                    Ok(Step {
                        action,
                        pause: Duration::from_millis(count(step, "/pause_ms")?),
                    })
                })
                .collect::<TestResult<_>>()?;
            Ok(Self {
                group: text(case, "/group")?.to_owned(),
                name: text(case, "/name")?.to_owned(),
                server: text(case, "/server")?.to_owned(),
                steps,
                window: Duration::from_millis(count(case, "/window_ms")?),
            })
        }

        /// How long the client reads in all, as the generator's `run` sets its read deadline:
        /// the window, every pause and the gaps of paced writes.
        fn total(&self) -> Duration {
            self.steps.iter().fold(self.window, |total, step| {
                let gaps = match &step.action {
                    Action::Fill { n, chunk, gap } if *chunk > 0 => {
                        gap.saturating_mul(u32::try_from(n / chunk).unwrap_or(u32::MAX))
                    }
                    Action::Wait | Action::Write(_) | Action::Fill { .. } | Action::Close(_) => {
                        Duration::ZERO
                    }
                };
                total + step.pause + gaps
            })
        }
    }

    /// The bytes a step's `hex` spells: request bytes that are not UTF-8, which the golden's
    /// text cannot carry.
    fn unhex(hex: &str) -> TestResult<Vec<u8>> {
        let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
        if !rest.is_empty() {
            return Err(format!("{hex:?} is not a whole number of bytes").into());
        }
        pairs
            .iter()
            .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
            .collect()
    }

    /// What a client read on one connection, as the generator describes it, every Date value in
    /// it (a second answer's too) replaced by `<date>`: the head of the first answer, the bytes
    /// after it (as text when they are UTF-8 and at most 8 KiB, with their length and CRC-32
    /// always), the gunzipped body of a gzip answer, and how the connection ended: `eof`,
    /// `reset`, `open` (still open when the window passed) or `client_closed` (the script closed
    /// it).
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Outcome {
        /// The head, with its blank line.
        head: String,
        /// The body as text, or empty.
        body: String,
        /// The body's length.
        body_len: u64,
        /// The body's CRC-32 (IEEE), as eight hex digits.
        body_crc32: String,
        /// The gunzipped body's length and CRC-32, or empty.
        gunzipped: String,
        /// How the connection ended.
        end: String,
    }

    impl Outcome {
        /// Go's outcome of a golden case.
        fn go(case: &Value) -> TestResult<Self> {
            Ok(Self {
                head: text(case, "/head")?.to_owned(),
                body: text(case, "/body")?.to_owned(),
                body_len: count(case, "/body_len")?,
                body_crc32: text(case, "/body_crc32")?.to_owned(),
                gunzipped: text(case, "/gunzipped").unwrap_or_default().to_owned(),
                end: text(case, "/end")?.to_owned(),
            })
        }

        /// Describes what a client read and how the connection ended.
        fn describe(answer: &[u8], end: &str) -> Self {
            let answer = dated(answer);
            let (head, body) = answer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .and_then(|at| answer.split_at_checked(at + 4))
                .unwrap_or((&[], answer.as_slice()));
            let head = String::from_utf8_lossy(head).into_owned();
            let gunzipped = if head
                .to_ascii_lowercase()
                .contains("\r\ncontent-encoding: gzip\r\n")
            {
                gunzip(&head, body)
            } else {
                String::new()
            };
            Self {
                body: if body.len() <= 8 << 10 {
                    String::from_utf8(body.to_vec()).unwrap_or_default()
                } else {
                    String::new()
                },
                body_len: u64::try_from(body.len()).unwrap_or(u64::MAX),
                body_crc32: format!("{:08x}", crc32(body)),
                head,
                gunzipped,
                end: end.to_owned(),
            }
        }

        /// The status line, or empty when the client read no head.
        fn status(&self) -> &str {
            self.head.split("\r\n").next().unwrap_or_default()
        }

        /// The header fields of the head, in order.
        fn fields(&self) -> Vec<(&str, &str)> {
            self.head
                .split("\r\n")
                .skip(1)
                .filter_map(|line| line.split_once(": "))
                .collect()
        }

        /// The value of the field `name` (case-insensitive), or empty.
        fn field(&self, name: &str) -> String {
            self.fields()
                .into_iter()
                .filter(|(field, _)| field.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// `answer` with the value of every line that starts with `Date: ` (in any case) replaced by
    /// `<date>`, a second answer's too, as the generator's `dateValue` replaces them: up to the
    /// line's `\r`.
    fn dated(answer: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(answer.len());
        for line in answer.split_inclusive(|&byte| byte == b'\n') {
            match line.split_at_checked(6) {
                Some((name, value)) if name.eq_ignore_ascii_case(b"date: ") => {
                    let end = value
                        .iter()
                        .position(|&byte| byte == b'\r')
                        .unwrap_or(value.len());
                    out.extend_from_slice(name);
                    out.extend_from_slice(b"<date>");
                    out.extend_from_slice(value.get(end..).unwrap_or_default());
                }
                Some(_) | None => out.extend_from_slice(line),
            }
        }
        out
    }

    /// The CRC-32 (IEEE) of `bytes`.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = Crc::new();
        crc.update(bytes);
        crc.sum()
    }

    /// The length and CRC-32 of a gzip body, undoing the chunked framing first when the head
    /// says so.
    fn gunzip(head: &str, body: &[u8]) -> String {
        let plain = if head
            .to_ascii_lowercase()
            .contains("\r\ntransfer-encoding: chunked\r\n")
        {
            let Some(plain) = unchunk(body) else {
                return "<bad chunked framing>".to_owned();
            };
            plain
        } else {
            body.to_vec()
        };
        let mut out = Vec::new();
        match GzDecoder::new(plain.as_slice()).read_to_end(&mut out) {
            Ok(_) => format!("{}/{:08x}", out.len(), crc32(&out)),
            Err(e) => format!("<{e}>"),
        }
    }

    /// The data of a chunked body.
    fn unchunk(mut body: &[u8]) -> Option<Vec<u8>> {
        let mut plain = Vec::new();
        loop {
            let end = body.windows(2).position(|window| window == b"\r\n")?;
            let (line, rest) = body.split_at_checked(end)?;
            let size = usize::from_str_radix(str::from_utf8(line).ok()?, 16).ok()?;
            if size == 0 {
                return Some(plain);
            }
            let data = rest.get(2..2 + size)?;
            plain.extend_from_slice(data);
            body = rest.get(4 + size..)?;
        }
    }

    /// Reads until the server closes the connection: `eof`, or `reset` when it resets it.
    async fn read_to_end(reader: &mut ReadHalf<'_>, answer: &mut Vec<u8>) -> TestResult<String> {
        let mut buf = vec![0; 64 << 10];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => return Ok("eof".to_owned()),
                Ok(n) => answer.extend_from_slice(buf.get(..n).unwrap_or_default()),
                Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok("reset".to_owned()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Runs the steps of a script and returns the close that ends it, if one does. Write errors
    /// are expected (the server may close the connection first) and ignored, as the generator
    /// ignores them.
    async fn write_steps(writer: &mut WriteHalf<'_>, steps: &[Step]) -> Option<bool> {
        for step in steps {
            match &step.action {
                Action::Wait => {}
                Action::Write(bytes) => drop(writer.write_all(bytes).await),
                Action::Fill { n, chunk: 0, .. } => drop(writer.write_all(&vec![b'a'; *n]).await),
                Action::Fill { n, chunk, gap } => {
                    let mut left = *n;
                    while left > 0 {
                        let size = left.min(*chunk);
                        drop(writer.write_all(&vec![b'a'; size]).await);
                        left -= size;
                        time::sleep(*gap).await;
                    }
                }
                Action::Close(reset) => return Some(*reset),
            }
            time::sleep(step.pause).await;
        }
        None
    }

    /// Runs `case` against `address`: the script on a fresh connection, reading until the
    /// server closes it, the script closes it or the case's time passes.
    async fn run(address: SocketAddr, case: Case) -> TestResult<Outcome> {
        let mut stream = TcpStream::connect(address).await?;
        let deadline = Instant::now() + case.total();
        let mut answer = Vec::new();
        let (end, reset) = {
            let (mut reader, mut writer) = stream.split();
            let mut reading = pin!(read_to_end(&mut reader, &mut answer));
            let mut writing = pin!(write_steps(&mut writer, &case.steps));
            let mut written = false;
            loop {
                tokio::select! {
                    end = &mut reading => break (end?, false),
                    close = &mut writing, if !written => {
                        written = true;
                        if let Some(reset) = close {
                            break ("client_closed".to_owned(), reset);
                        }
                    }
                    () = time::sleep_until(deadline) => break ("open".to_owned(), false),
                }
            }
        };
        if reset {
            stream.set_zero_linger()?;
        }
        drop(stream);
        Ok(Outcome::describe(&answer, &end))
    }

    /// The bytes `/bytes` writes: `n` bytes cycling through the alphabet.
    fn pattern(n: usize) -> Vec<u8> {
        (b'a'..=b'z').cycle().take(n).collect()
    }

    /// The stand-in handlers of the generator's handlers.go on axum. Each answers as a Rust
    /// handler would: the types it sets are those of its response types, so `/sniff`, which
    /// writes bytes, is `application/octet-stream`.
    fn stand_ins() -> Router {
        let marks = Marks::default();
        let read = Arc::clone(&marks);
        Router::new()
            .route("/headers", any(headers))
            .route("/bytes", any(bytes))
            .route("/sniff", any(sniff))
            .route("/typed", any(typed))
            .route("/echo", any(echo))
            .route(
                "/sleep",
                any(move |query: Params| sleep(query, Arc::clone(&marks))),
            )
            .route(
                "/mark",
                any(move |query: Params| {
                    let answer = mark(&query, &read);
                    async move { answer }
                }),
            )
    }

    /// `/headers`: X-Zeta, Content-Type, X-Alpha and two X-Multi values, in that order.
    async fn headers() -> Response {
        (
            AppendHeaders([
                ("x-zeta", "z"),
                ("content-type", "application/json"),
                ("x-alpha", "a"),
                ("x-multi", "1"),
                ("x-multi", "2"),
            ]),
            Body::from(r#"{"ok":true}"#),
        )
            .into_response()
    }

    /// The query's `name` as a number, 0 when it has none (Go's `strconv.Atoi` result).
    fn param(Query(params): &Params, name: &str) -> u64 {
        params
            .get(name)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    /// `/bytes`: `n` pattern bytes as text/plain; charset=utf-8.
    async fn bytes(query: Params) -> Response {
        let n = usize::try_from(param(&query, "n")).unwrap_or(0);
        (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            Body::from(pattern(n)),
        )
            .into_response()
    }

    /// `/sniff`: a body of the query's kind, as bytes.
    async fn sniff(Query(params): Params) -> Vec<u8> {
        match params.get("kind").map(String::as_str) {
            Some("json") => br#"{"id":"req-1","imp":[]}"#.to_vec(),
            Some("html") => b"<html><body>Prebid Server</body></html>".to_vec(),
            Some("xml") => {
                br#"<?xml version="1.0" encoding="UTF-8"?><VAST version="3.0"></VAST>"#.to_vec()
            }
            Some("text") => b"ok".to_vec(),
            Some("binary") => vec![0x00, 0x01, 0x02, 0x03, 0xFE, 0xFF],
            Some(_) | None => Vec::new(),
        }
    }

    /// `/typed`: `{"a":1}` as application/json.
    async fn typed() -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            Body::from(r#"{"a":1}"#),
        )
            .into_response()
    }

    /// Whether a body error is tower-http's request-body timeout.
    fn timed_out(error: &axum::Error) -> bool {
        let first: &(dyn Error + 'static) = error;
        iter::successors(Some(first), |e| (*e).source()).any(<dyn Error>::is::<TimeoutError>)
    }

    /// `/echo`: reads the body and answers `body=<bytes read>`, with ` error=timeout` when a
    /// budget cut the read short or ` error=<text>` for another error.
    async fn echo(mut body: Body) -> String {
        let mut read = 0;
        loop {
            match poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
                None => return format!("body={read}"),
                Some(Ok(frame)) => read += frame.data_ref().map_or(0, Bytes::len),
                Some(Err(error)) if timed_out(&error) => {
                    return format!("body={read} error=timeout");
                }
                Some(Err(error)) => return format!("body={read} error={error}"),
            }
        }
    }

    /// `/sleep`: sleeps the query's `ms`, sets its `mark` and answers `late`.
    async fn sleep(query: Params, marks: Marks) -> &'static str {
        time::sleep(Duration::from_millis(param(&query, "ms"))).await;
        let Query(params) = query;
        if let Some(mark) = params.get("mark")
            && let Ok(mut set) = marks.lock()
        {
            set.extend([mark.clone()]);
        }
        "late"
    }

    /// `/mark`: `completed` when a `/sleep` set the query's `id`, else `pending`.
    fn mark(Query(params): &Params, marks: &Marks) -> &'static str {
        let set = params
            .get("id")
            .is_some_and(|id| marks.lock().is_ok_and(|set| set.contains(id)));
        if set { "completed" } else { "pending" }
    }

    /// A loopback server running [`serve`].
    struct Server {
        /// Where it listens.
        address: SocketAddr,
        /// Sends its shutdown signal; dropping it stops the server too.
        stop: oneshot::Sender<()>,
        /// The task serving it.
        task: JoinHandle<Stopped>,
    }

    impl Server {
        /// Serves `router` within `budget` on a fresh loopback port.
        async fn start(router: Router, budget: Budget) -> TestResult<Self> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let (stop, stopped) = oneshot::channel::<()>();
            let task = task::spawn(serve(listener, router, budget, async move {
                stopped.await.unwrap_or_default();
            }));
            Ok(Self {
                address,
                stop,
                task,
            })
        }
    }

    /// The servers the golden's cases go to: the main server (15 s budget), the main server with
    /// gzip, the main server with its budget scaled to the golden's, and the admin server (no
    /// budget).
    async fn servers(doc: &Value) -> TestResult<BTreeMap<&'static str, Server>> {
        let scaled = Budget::of(Duration::from_millis(count(doc, "/budget_ms")?));
        Ok(BTreeMap::from([
            ("main", Server::start(stand_ins(), Budget::MAIN).await?),
            (
                "main_gzip",
                Server::start(
                    compression::compression_enabled(stand_ins(), true),
                    Budget::MAIN,
                )
                .await?,
            ),
            ("scaled", Server::start(stand_ins(), scaled).await?),
            ("admin", Server::start(stand_ins(), Budget::NONE).await?),
        ]))
    }

    /// A runtime for the loopback servers and clients.
    fn runtime() -> TestResult<Runtime> {
        Ok(runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?)
    }

    /// One golden case with Go's outcome and the Rust server's.
    #[derive(Debug)]
    struct Pair {
        /// The case.
        case: Case,
        /// What the Go server's client read.
        go: Outcome,
        /// What the Rust server's client read.
        rust: Outcome,
    }

    /// Runs every case of `groups` at once, each against its Rust server, and pairs the outcomes
    /// with Go's.
    fn run_groups(groups: &[&str]) -> TestResult<Vec<Pair>> {
        let doc = golden()?;
        let mut cases = Vec::new();
        for case in list(&doc, "/cases")? {
            let read = Case::read(case)?;
            if groups.contains(&read.group.as_str()) {
                cases.push((read, Outcome::go(case)?));
            }
        }
        runtime()?.block_on(async {
            let servers = servers(&doc).await?;
            let mut runs = Vec::new();
            for (case, _) in &cases {
                let server = servers
                    .get(case.server.as_str())
                    .ok_or_else(|| format!("no server {}", case.server))?;
                runs.push(task::spawn(run(server.address, case.clone())));
            }
            let mut pairs = Vec::new();
            for ((case, go), outcome) in cases.into_iter().zip(runs) {
                pairs.push(Pair {
                    case,
                    go,
                    rust: outcome.await??,
                });
            }
            drop(servers);
            Ok(pairs)
        })
    }

    /// Fails with `what` unless `condition` holds.
    fn check(condition: bool, what: &str) -> TestResult {
        if condition { Ok(()) } else { Err(what.into()) }
    }

    /// The pair of the case named `name`.
    fn find<'p>(pairs: &'p [Pair], name: &str) -> TestResult<&'p Pair> {
        pairs
            .iter()
            .find(|pair| pair.case.name == name)
            .ok_or_else(|| format!("no case {name}").into())
    }

    /// `name: <what>` for each case named in `names`, of one side of `pairs`.
    fn rows<F: Fn(&Outcome) -> String>(
        pairs: &[Pair],
        names: &[&str],
        side: fn(&Pair) -> &Outcome,
        what: F,
    ) -> TestResult<Vec<String>> {
        names
            .iter()
            .map(|name| Ok(format!("{name}: {}", what(side(find(pairs, name)?)))))
            .collect()
    }

    /// Go's side of a pair.
    const fn go(pair: &Pair) -> &Outcome {
        &pair.go
    }

    /// The Rust server's side of a pair.
    const fn rust(pair: &Pair) -> &Outcome {
        &pair.rust
    }

    /// The status line, the body and how the connection ended.
    fn answer(outcome: &Outcome) -> String {
        format!("{} | {} | {}", outcome.status(), outcome.body, outcome.end)
    }

    /// Each answer a client read on a connection, in order: its status code, with ` close` when
    /// it carries `Connection: close`.
    fn answers(outcome: &Outcome) -> Vec<String> {
        let read = format!("{}{}", outcome.head, outcome.body);
        read.match_indices("HTTP/1.")
            .filter_map(|(at, _)| read.get(at..))
            .map(|answer| {
                let head = answer.split("\r\n\r\n").next().unwrap_or_default();
                let code = head.get(9..12).unwrap_or_default();
                if head
                    .split("\r\n")
                    .any(|line| line.eq_ignore_ascii_case("connection: close"))
                {
                    format!("{code} close")
                } else {
                    code.to_owned()
                }
            })
            .collect()
    }

    /// How a connection ended, a close and a reset alike: which of the two a client reads when
    /// the server closes a connection with request bytes still unread depends on the system's
    /// TCP stack and the timing.
    fn closed(outcome: &Outcome) -> &str {
        match outcome.end.as_str() {
            "eof" | "reset" => "closed",
            other => other,
        }
    }

    /// The field names of an outcome's head, in order.
    fn names(outcome: &Outcome) -> Vec<&str> {
        outcome.fields().into_iter().map(|(name, _)| name).collect()
    }

    /// Whether `name` is in net/http's canonical form: every hyphen-separated word capitalised.
    fn canonical(name: &str) -> bool {
        name.split('-').all(|word| {
            let mut chars = word.chars();
            chars.next().is_some_and(|c| c.is_ascii_uppercase())
                && chars.all(|c| !c.is_ascii_uppercase())
        })
    }

    /// How an answer's body is framed: `length N`, `chunked`, or `unframed` (the connection's end
    /// ends it).
    fn framing(outcome: &Outcome) -> String {
        let length = outcome.field("content-length");
        if outcome.field("transfer-encoding") == "chunked" {
            "chunked".to_owned()
        } else if length.is_empty() {
            "unframed".to_owned()
        } else {
            format!("length {length}")
        }
    }

    /// Serves `router` on a loopback port with hyper-util's connection builder as it comes:
    /// HTTP/1.1 and HTTP/2, hyper's buffer limit and half-closed connections closed. The pins
    /// use it to show what the crates do without pbs-server's settings.
    async fn start_default(router: Router) -> TestResult<SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let service = TowerToHyperService::new(router);
        drop(task::spawn(async move {
            let builder = auto::Builder::new(TokioExecutor::new());
            while let Ok((io, _)) = listener.accept().await {
                let connection = builder
                    .serve_connection(TokioIo::new(io), service.clone())
                    .into_owned();
                drop(task::spawn(connection));
            }
        }));
        Ok(address)
    }

    /// Runs the golden cases named `names` at once against [`start_default`].
    fn run_default(names: &[&str]) -> TestResult<Vec<(String, Outcome)>> {
        let doc = golden()?;
        let mut cases = Vec::new();
        for case in list(&doc, "/cases")? {
            let read = Case::read(case)?;
            if names.contains(&read.name.as_str()) {
                cases.push(read);
            }
        }
        runtime()?.block_on(async {
            let address = start_default(stand_ins()).await?;
            let runs: Vec<_> = cases
                .into_iter()
                .map(|case| (case.name.clone(), task::spawn(run(address, case))))
                .collect();
            let mut outcomes = Vec::new();
            for (name, outcome) in runs {
                outcomes.push((name, outcome.await??));
            }
            Ok(outcomes)
        })
    }

    /// net/http writes the handler's header fields sorted by name, then Date, Content-Length and
    /// Connection; hyper writes them in the order the handler set them, then content-length,
    /// connection and date.
    // Go: Header.writeSubset sorts the keys (net/http/header.go), and chunkWriter.writeHeader
    // writes Date, Content-Length and Connection after them (net/http/server.go)
    #[test]
    fn header_order() -> TestResult {
        let pairs = run_groups(&["headers"])?;
        let pair = find(&pairs, "handler_order")?;
        ensure_eq(
            &names(&pair.go),
            &[
                "Content-Type",
                "X-Alpha",
                "X-Multi",
                "X-Multi",
                "X-Zeta",
                "Date",
                "Content-Length",
                "Connection",
            ],
            "net/http's order",
        )?;
        ensure_eq(
            &names(&pair.rust),
            &[
                "x-zeta",
                "content-type",
                "x-alpha",
                "x-multi",
                "x-multi",
                "content-length",
                "connection",
                "date",
            ],
            "hyper's order",
        )
    }

    /// net/http writes canonical header names (`Content-Type`), hyper lower-case ones
    /// (D-HDRCASE); ignoring case, the names are the same.
    // Go: Header.Set and Header.Add store textproto.CanonicalMIMEHeaderKey (net/http/header.go)
    #[test]
    fn lower_case_names() -> TestResult {
        for pair in run_groups(&["headers"])? {
            let go = names(&pair.go);
            let rust = names(&pair.rust);
            check(
                go.iter().all(|name| canonical(name)),
                "net/http's names are canonical",
            )?;
            check(
                rust.iter()
                    .all(|name| !name.bytes().any(|b| b.is_ascii_uppercase())),
                "hyper's names are lower case",
            )?;
            let mut folded: Vec<String> = go.iter().map(|name| name.to_ascii_lowercase()).collect();
            folded.sort_unstable();
            let mut lower = rust;
            lower.sort_unstable();
            ensure_eq(&folded, &lower, &pair.case.name)?;
        }
        Ok(())
    }

    /// net/http gives an answer whose handler wrote at most 2048 bytes a Content-Length and
    /// chunks a longer one, which over HTTP/1.0 the closed connection ends, and sends no length
    /// to HEAD; a gzip answer whose compressed bytes fit that buffer gets their length. hyper
    /// sends content-length for every full body, to HEAD and HTTP/1.0 too, and chunks what
    /// tower-http compresses.
    // Go: bufferBeforeChunkingSize and chunkWriter.writeHeader (net/http/server.go)
    #[test]
    fn content_length_always() -> TestResult {
        let pairs = run_groups(&["framing", "gzip"])?;
        let names = [
            "bytes_0",
            "bytes_100",
            "bytes_2048",
            "bytes_2049",
            "bytes_3000",
            "bytes_3000_http10",
            "head_bytes_3000",
            "bytes_n_1400_gzip",
            "bytes_n_3000_gzip",
        ];
        ensure_eq(
            &rows(&pairs, &names, go, framing)?,
            &[
                "bytes_0: length 0",
                "bytes_100: length 100",
                "bytes_2048: length 2048",
                "bytes_2049: chunked",
                "bytes_3000: chunked",
                "bytes_3000_http10: unframed",
                "head_bytes_3000: unframed",
                "bytes_n_1400_gzip: length 62",
                "bytes_n_3000_gzip: length 69",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, framing)?,
            &[
                "bytes_0: length 0",
                "bytes_100: length 100",
                "bytes_2048: length 2048",
                "bytes_2049: length 2049",
                "bytes_3000: length 3000",
                "bytes_3000_http10: length 3000",
                "head_bytes_3000: length 3000",
                "bytes_n_1400_gzip: chunked",
                "bytes_n_3000_gzip: chunked",
            ],
            "hyper",
        )
    }

    /// net/http sniffs the type of an answer whose handler set none (`http.DetectContentType`);
    /// a Rust handler's response type sets it, so the bytes `/sniff` returns are
    /// application/octet-stream whatever they hold. A type the handler sets is sent as set.
    // Go: response.Write calls DetectContentType when no Content-Type is set (net/http/server.go)
    #[test]
    fn no_generic_sniffing() -> TestResult {
        let pairs = run_groups(&["sniffing", "headers"])?;
        let names = [
            "sniff_json",
            "sniff_html",
            "sniff_xml",
            "sniff_text",
            "sniff_binary",
            "sniff_empty",
            "typed",
        ];
        let content_type = |outcome: &Outcome| outcome.field("content-type");
        ensure_eq(
            &rows(&pairs, &names, go, content_type)?,
            &[
                "sniff_json: text/plain; charset=utf-8",
                "sniff_html: text/html; charset=utf-8",
                "sniff_xml: text/xml; charset=utf-8",
                "sniff_text: text/plain; charset=utf-8",
                "sniff_binary: application/octet-stream",
                "sniff_empty: ",
                "typed: application/json",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, content_type)?,
            &[
                "sniff_json: application/octet-stream",
                "sniff_html: application/octet-stream",
                "sniff_xml: application/octet-stream",
                "sniff_text: application/octet-stream",
                "sniff_binary: application/octet-stream",
                "sniff_empty: application/octet-stream",
                "typed: application/json",
            ],
            "axum",
        )
    }

    /// pbs-server reads request heads up to net/http's limit, 1 MiB + 4096 bytes: the heads Go
    /// serves are served and the paced heads past the limit get 431, where hyper's default
    /// buffer (408 KiB) refuses the 600 KiB and 1 MiB heads as well. The 431 answer's form is
    /// `server.bad-requests`.
    // Go: DefaultMaxHeaderBytes and initialReadLimitSize (net/http/server.go), which Prebid's
    // servers keep (server/server.go:90-118)
    #[test]
    fn header_limit_like_go() -> TestResult {
        let pairs = run_groups(&["header_limit"])?;
        let names = [
            "header_600kib",
            "header_1mib",
            "header_at_limit",
            "header_over_64kib_paced",
            "header_2mib_paced",
        ];
        let status = |outcome: &Outcome| outcome.status().to_owned();
        let want = [
            "header_600kib: HTTP/1.1 200 OK",
            "header_1mib: HTTP/1.1 200 OK",
            "header_at_limit: HTTP/1.1 200 OK",
            "header_over_64kib_paced: HTTP/1.1 431 Request Header Fields Too Large",
            "header_2mib_paced: HTTP/1.1 431 Request Header Fields Too Large",
        ];
        ensure_eq(&rows(&pairs, &names, go, status)?, &want, "net/http")?;
        ensure_eq(&rows(&pairs, &names, rust, status)?, &want, "pbs-server")?;
        let defaults: Vec<String> = run_default(&["header_600kib", "header_1mib"])?
            .into_iter()
            .map(|(name, outcome)| format!("{name}: {}", outcome.status()))
            .collect();
        ensure_eq(
            &defaults,
            &[
                "header_600kib: HTTP/1.1 431 Request Header Fields Too Large",
                "header_1mib: HTTP/1.1 431 Request Header Fields Too Large",
            ],
            "hyper's default buffer",
        )
    }

    /// A head one byte over the limit whose last bytes come in one read: net/http counts every
    /// byte and answers 431; hyper parses what it has read before it checks the size, so a head
    /// that ends in the read that crosses the limit is served.
    // Go: the limit is the connection reader's (conn.r.setReadLimit, net/http/server.go)
    #[test]
    fn header_limit_edge_differs() -> TestResult {
        let pairs = run_groups(&["header_limit_edge"])?;
        let pair = find(&pairs, "header_one_over")?;
        ensure_eq(
            pair.go.status(),
            "HTTP/1.1 431 Request Header Fields Too Large",
            "net/http",
        )?;
        ensure_eq(
            answer(&pair.rust).as_str(),
            "HTTP/1.1 200 OK | body=0 | eof",
            "hyper",
        )
    }

    /// net/http bounds a head only by its bytes; hyper also refuses a request with more than 100
    /// header fields (Host and Connection count), with 431.
    // Go: textproto.Reader.ReadMIMEHeader reads any number of fields
    #[test]
    fn header_count_limit() -> TestResult {
        let pairs = run_groups(&["header_count"])?;
        let names = ["headers_100", "headers_101", "headers_1000"];
        let status = |outcome: &Outcome| outcome.status().to_owned();
        ensure_eq(
            &rows(&pairs, &names, go, status)?,
            &[
                "headers_100: HTTP/1.1 200 OK",
                "headers_101: HTTP/1.1 200 OK",
                "headers_1000: HTTP/1.1 200 OK",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, status)?,
            &[
                "headers_100: HTTP/1.1 200 OK",
                "headers_101: HTTP/1.1 431 Request Header Fields Too Large",
                "headers_1000: HTTP/1.1 431 Request Header Fields Too Large",
            ],
            "hyper",
        )
    }

    /// net/http bounds the request target only by the header limit; hyper refuses a target
    /// longer than 65,534 bytes with 414.
    // Go: readRequest takes the request line as the header limit allows (net/http/request.go)
    #[test]
    fn long_request_target() -> TestResult {
        let pairs = run_groups(&["long_target"])?;
        let names = ["target_65534", "target_65535", "target_100000"];
        let status = |outcome: &Outcome| outcome.status().to_owned();
        ensure_eq(
            &rows(&pairs, &names, go, status)?,
            &[
                "target_65534: HTTP/1.1 200 OK",
                "target_65535: HTTP/1.1 200 OK",
                "target_100000: HTTP/1.1 200 OK",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, status)?,
            &[
                "target_65534: HTTP/1.1 200 OK",
                "target_65535: HTTP/1.1 414 URI Too Long",
                "target_100000: HTTP/1.1 414 URI Too Long",
            ],
            "hyper",
        )
    }

    /// Requests net/http refuses before any handler runs get its status text as the body and
    /// Connection: close; hyper's refusals carry no body. Which requests are refused also
    /// differs: hyper serves a request without Host, with two Host fields, with a malformed one
    /// or after a blank line, and one with an expectation other than 100-continue (net/http's
    /// 417), refuses a folded header and an HTTP/1.0 request with Transfer-Encoding (which
    /// net/http ignores), and its codes are 400 where net/http has 505 and 501. A body error
    /// reaches the handler on both, in the library's words.
    // Go: conn.serve's error answers (net/http/server.go) and readRequest's checks
    // (net/http/request.go)
    #[test]
    fn malformed_request() -> TestResult {
        let pairs = run_groups(&["malformed", "header_limit"])?;
        let names = [
            "missing_host",
            "two_hosts",
            "header_without_colon",
            "space_in_header_name",
            "space_before_colon",
            "nul_in_header_value",
            "folded_header",
            "garbage",
            "lower_case_method",
            "unknown_version",
            "http_10_without_host",
            "bad_content_length",
            "two_content_lengths",
            "unknown_transfer_encoding",
            "chunked_and_length",
            "bad_chunk_size",
            "leading_blank_line",
            "absolute_form",
            "malformed_host",
            "unknown_expectation",
            "chunked_http_10",
            "header_2mib_paced",
        ];
        ensure_eq(
            &rows(&pairs, &names, go, answer)?,
            &[
                "missing_host: HTTP/1.1 400 Bad Request: missing required Host header | 400 Bad \
                 Request: missing required Host header | eof",
                "two_hosts: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "header_without_colon: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "space_in_header_name: HTTP/1.1 400 Bad Request: invalid header name | 400 Bad \
                 Request: invalid header name | eof",
                "space_before_colon: HTTP/1.1 400 Bad Request: invalid header name | 400 Bad \
                 Request: invalid header name | eof",
                "nul_in_header_value: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "folded_header: HTTP/1.1 200 OK | body=0 | eof",
                "garbage: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "lower_case_method: HTTP/1.1 200 OK | body=0 | eof",
                "unknown_version: HTTP/1.1 505 HTTP Version Not Supported: unsupported protocol \
                 version | 505 HTTP Version Not Supported: unsupported protocol version | eof",
                "http_10_without_host: HTTP/1.0 200 OK | body=0 | eof",
                "bad_content_length: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "two_content_lengths: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "unknown_transfer_encoding: HTTP/1.1 501 Not Implemented | Unsupported transfer \
                 encoding | eof",
                "chunked_and_length: HTTP/1.1 200 OK | body=3 | eof",
                "bad_chunk_size: HTTP/1.1 200 OK | body=0 error=invalid byte in chunk length | \
                 eof",
                "leading_blank_line: HTTP/1.1 400 Bad Request | 400 Bad Request | eof",
                "absolute_form: HTTP/1.1 200 OK | body=0 | eof",
                "malformed_host: HTTP/1.1 400 Bad Request: malformed Host header | 400 Bad \
                 Request: malformed Host header | eof",
                "unknown_expectation: HTTP/1.1 417 Expectation Failed |  | eof",
                "chunked_http_10: HTTP/1.0 200 OK | body=0 | eof",
                "header_2mib_paced: HTTP/1.1 431 Request Header Fields Too Large | 431 Request \
                 Header Fields Too Large | eof",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, answer)?,
            &[
                "missing_host: HTTP/1.1 200 OK | body=0 | eof",
                "two_hosts: HTTP/1.1 200 OK | body=0 | eof",
                "header_without_colon: HTTP/1.1 400 Bad Request |  | eof",
                "space_in_header_name: HTTP/1.1 400 Bad Request |  | eof",
                "space_before_colon: HTTP/1.1 400 Bad Request |  | eof",
                "nul_in_header_value: HTTP/1.1 400 Bad Request |  | eof",
                "folded_header: HTTP/1.1 400 Bad Request |  | eof",
                "garbage: HTTP/1.1 400 Bad Request |  | eof",
                "lower_case_method: HTTP/1.1 200 OK | body=0 | eof",
                "unknown_version: HTTP/1.1 400 Bad Request |  | eof",
                "http_10_without_host: HTTP/1.0 200 OK | body=0 | eof",
                "bad_content_length: HTTP/1.1 400 Bad Request |  | eof",
                "two_content_lengths: HTTP/1.1 400 Bad Request |  | eof",
                "unknown_transfer_encoding: HTTP/1.1 400 Bad Request |  | eof",
                "chunked_and_length: HTTP/1.1 200 OK | body=3 | eof",
                "bad_chunk_size: HTTP/1.1 200 OK | body=0 error=error reading a body from \
                 connection | eof",
                "leading_blank_line: HTTP/1.1 200 OK | body=0 | eof",
                "absolute_form: HTTP/1.1 200 OK | body=0 | eof",
                "malformed_host: HTTP/1.1 200 OK | body=0 | eof",
                "unknown_expectation: HTTP/1.1 200 OK | body=2 | eof",
                "chunked_http_10: HTTP/1.1 400 Bad Request |  | eof",
                "header_2mib_paced: HTTP/1.1 431 Request Header Fields Too Large |  | eof",
            ],
            "hyper",
        )
    }

    /// A body the handler leaves unread (`/typed` reads none): net/http reads and discards what
    /// is left, up to 256 KiB and waiting for it, before it answers, and keeps the connection for
    /// the next request; with 256 KiB or more left, or with an `Expect: 100-continue` whose body
    /// was not read, it answers with `Connection: close`. hyper answers at once, makes one read
    /// of what is left and keeps the connection only when that read ends the body; otherwise it
    /// closes the connection, without `connection: close`. Both send `100 Continue` to a handler
    /// that reads the body.
    // Go: chunkWriter.writeHeader discards up to maxPostHandlerReadBytes
    // (net/http/server.go:1374-1443)
    #[test]
    fn unread_body() -> TestResult {
        let pairs = run_groups(&["unread_body"])?;
        let names = [
            "unread_buffered",
            "unread_late",
            "unread_100kib",
            "unread_300kib",
            "expect_read",
            "expect_unread",
        ];
        ensure_eq(
            &rows(&pairs, &names, go, |outcome| {
                format!("{} | {}", answers(outcome).join(", "), outcome.end)
            })?,
            &[
                "unread_buffered: 200, 200 close | eof",
                "unread_late: 200, 200 close | eof",
                "unread_100kib: 200, 200 close | eof",
                "unread_300kib: 200 close | eof",
                "expect_read: 100, 200 close | eof",
                "expect_unread: 200 close | eof",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, |outcome| {
                format!("{} | {}", answers(outcome).join(", "), closed(outcome))
            })?,
            &[
                "unread_buffered: 200, 200 close | closed",
                "unread_late: 200 | closed",
                "unread_100kib: 200 | closed",
                "unread_300kib: 200 | closed",
                "expect_read: 100, 200 close | closed",
                "expect_unread: 200 | closed",
            ],
            "hyper",
        )
    }

    /// Request targets with bytes a browser would have percent-encoded: net/http refuses only
    /// control bytes and keeps a `#` in the path or the query, so each of these requests is
    /// routed (to a handler or to the mux's 404). The http crate's `Uri`, which hyper builds
    /// before any service runs, refuses `<`, `>` and `` ` `` in the path, `"`, `<` and `>` in the
    /// query and bytes that are not UTF-8 with 400, and drops a `#` and what follows it, so
    /// `/echo#x` is `/echo` and `/bytes?n=1#0` asks for one byte where Go's `n` is `1#0`.
    // Go: readRequest parses the target with url.ParseRequestURI (net/http/request.go:1123),
    // which refuses only control bytes (net/url/url.go:435)
    #[test]
    fn request_target_bytes() -> TestResult {
        let pairs = run_groups(&["request_target"])?;
        let names = [
            "quote_in_query",
            "json_in_query",
            "angle_in_query",
            "query_punctuation",
            "utf8_in_query",
            "latin1_in_query",
            "angle_in_path",
            "backtick_in_path",
            "quote_braces_in_path",
            "hash_in_path",
            "hash_in_query",
        ];
        ensure_eq(
            &rows(&pairs, &names, go, answer)?,
            &[
                "quote_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "json_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "angle_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "query_punctuation: HTTP/1.1 200 OK | body=0 | eof",
                "utf8_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "latin1_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "angle_in_path: HTTP/1.1 404 Not Found | 404 page not found\n | eof",
                "backtick_in_path: HTTP/1.1 404 Not Found | 404 page not found\n | eof",
                "quote_braces_in_path: HTTP/1.1 404 Not Found | 404 page not found\n | eof",
                "hash_in_path: HTTP/1.1 404 Not Found | 404 page not found\n | eof",
                "hash_in_query: HTTP/1.1 200 OK |  | eof",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, answer)?,
            &[
                "quote_in_query: HTTP/1.1 400 Bad Request |  | eof",
                "json_in_query: HTTP/1.1 400 Bad Request |  | eof",
                "angle_in_query: HTTP/1.1 400 Bad Request |  | eof",
                "query_punctuation: HTTP/1.1 200 OK | body=0 | eof",
                "utf8_in_query: HTTP/1.1 200 OK | body=0 | eof",
                "latin1_in_query: HTTP/1.1 400 Bad Request |  | eof",
                "angle_in_path: HTTP/1.1 400 Bad Request |  | eof",
                "backtick_in_path: HTTP/1.1 400 Bad Request |  | eof",
                "quote_braces_in_path: HTTP/1.1 404 Not Found |  | eof",
                "hash_in_path: HTTP/1.1 200 OK | body=0 | eof",
                "hash_in_query: HTTP/1.1 200 OK | a | eof",
            ],
            "the http crate",
        )
    }

    /// Sends HTTP/2's client preface with prior knowledge to `address` and returns the first
    /// bytes the server answers within a second.
    async fn preface(address: SocketAddr) -> TestResult<Vec<u8>> {
        let mut stream = TcpStream::connect(address).await?;
        stream
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
            .await?;
        let mut buf = vec![0; 64];
        let read = time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await;
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e.into()),
            Err(_elapsed) => 0,
        };
        buf.truncate(n);
        Ok(buf)
    }

    /// Neither server speaks HTTP/2 without TLS: net/http answers the prior-knowledge preface as
    /// a malformed HTTP/1.1 request (400) and serves a request asking to upgrade to h2c as
    /// HTTP/1.1; pbs-server's HTTP/1.1-only builder closes the connection at the preface without
    /// an answer (the answer is `server.bad-requests`) and serves the upgrade request as
    /// HTTP/1.1. hyper-util's builder as it comes would answer the preface with HTTP/2 SETTINGS.
    // Go: Prebid's servers set no Protocols, so net/http serves no unencrypted HTTP/2
    // (server/server.go:90-118)
    #[test]
    fn no_h2c() -> TestResult {
        let pairs = run_groups(&["h2c"])?;
        let names = ["prior_knowledge", "upgrade_header"];
        ensure_eq(
            &rows(&pairs, &names, go, answer)?,
            &[
                "prior_knowledge: HTTP/1.1 400 Bad Request |  | eof",
                "upgrade_header: HTTP/1.1 200 OK | body=0 | open",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, answer)?,
            &[
                "prior_knowledge:  |  | eof",
                "upgrade_header: HTTP/1.1 200 OK | body=0 | open",
            ],
            "pbs-server",
        )?;
        let frame = runtime()?.block_on(async {
            let address = start_default(stand_ins()).await?;
            preface(address).await
        })?;
        ensure_eq(
            &frame.get(3).copied(),
            &Some(0x04),
            "hyper-util's default builder answers with a SETTINGS frame",
        )
    }

    /// The budgets: net/http's `ReadTimeout` is one deadline for head and body from the start of
    /// the request and also ends an idle keep-alive connection, and past `WriteTimeout` the
    /// answer is lost; pbs-server's budget bounds the head and the idle wait alike, the body per
    /// gap between reads, and nothing bounds the handler, so a late answer is still sent. The
    /// admin server has no budget on either.
    // Go: ReadTimeout, WriteTimeout and IdleTimeout (net/http/server.go) as
    // server/server.go:103-116 sets them
    #[test]
    fn timeouts() -> TestResult {
        let pairs = run_groups(&["timeouts"])?;
        let names = [
            "slow_headers",
            "slow_headers_admin",
            "trickled_headers",
            "idle_keep_alive",
            "idle_keep_alive_admin",
            "stalled_body",
            "trickled_body",
            "quick_handler",
            "slow_handler",
            "slow_handler_admin",
        ];
        ensure_eq(
            &rows(&pairs, &names, go, answer)?,
            &[
                "slow_headers:  |  | eof",
                "slow_headers_admin:  |  | open",
                "trickled_headers:  |  | eof",
                "idle_keep_alive: HTTP/1.1 200 OK | body=0 | eof",
                "idle_keep_alive_admin: HTTP/1.1 200 OK | body=0 | open",
                "stalled_body: HTTP/1.1 200 OK | body=5 error=timeout | eof",
                "trickled_body: HTTP/1.1 200 OK | body=2 error=timeout | eof",
                "quick_handler: HTTP/1.1 200 OK | late | eof",
                "slow_handler:  |  | eof",
                "slow_handler_admin: HTTP/1.1 200 OK | late | eof",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, answer)?,
            &[
                "slow_headers:  |  | eof",
                "slow_headers_admin:  |  | open",
                "trickled_headers:  |  | eof",
                "idle_keep_alive: HTTP/1.1 200 OK | body=0 | eof",
                "idle_keep_alive_admin: HTTP/1.1 200 OK | body=0 | open",
                "stalled_body: HTTP/1.1 200 OK | body=5 error=timeout | eof",
                "trickled_body: HTTP/1.1 200 OK | body=5 | eof",
                "quick_handler: HTTP/1.1 200 OK | late | eof",
                "slow_handler: HTTP/1.1 200 OK | late | eof",
                "slow_handler_admin: HTTP/1.1 200 OK | late | eof",
            ],
            "pbs-server",
        )
    }

    /// A client that closes its connection, with a FIN or a reset, while its request is served:
    /// net/http lets the handler run to its end, and so does pbs-server, which keeps half-closed
    /// connections; hyper as it comes notices the close and drops the handler.
    // Go: Prebid's handlers run from context.Background() (endpoints/openrtb2/auction.go), and
    // net/http does not stop a handler whose client went away
    #[test]
    fn handler_outlives_client() -> TestResult {
        let pairs = run_groups(&["client_gone"])?;
        let names = ["gone_fin", "check_fin", "gone_rst", "check_rst"];
        let want = [
            "gone_fin:  |  | client_closed",
            "check_fin: HTTP/1.1 200 OK | completed | eof",
            "gone_rst:  |  | client_closed",
            "check_rst: HTTP/1.1 200 OK | completed | eof",
        ];
        ensure_eq(&rows(&pairs, &names, go, answer)?, &want, "net/http")?;
        ensure_eq(&rows(&pairs, &names, rust, answer)?, &want, "pbs-server")?;
        let defaults: Vec<String> = run_default(&names)?
            .into_iter()
            .map(|(name, outcome)| format!("{name}: {}", answer(&outcome)))
            .collect();
        ensure_eq(
            &defaults,
            &[
                "gone_fin:  |  | client_closed",
                "check_fin: HTTP/1.1 200 OK | pending | eof",
                "gone_rst:  |  | client_closed",
                "check_rst: HTTP/1.1 200 OK | pending | eof",
            ],
            "hyper as it comes",
        )
    }

    /// With gzip on, gziphandler adds `Vary: Accept-Encoding` to every answer; tower-http adds
    /// it only to answers its predicate would compress (1,400 bytes or more), whether or not the
    /// request accepts gzip. The bodies it compresses gunzip to Go's.
    // Go: GzipResponseWriter.WriteHeader and Write add Vary (gziphandler v1.1.1 gzip.go)
    #[test]
    fn vary_only_on_compressible() -> TestResult {
        let pairs = run_groups(&["gzip"])?;
        let names: Vec<&str> = pairs.iter().map(|pair| pair.case.name.as_str()).collect();
        let vary = |outcome: &Outcome| outcome.field("vary");
        let go_rows = rows(&pairs, &names, go, vary)?;
        check(
            go_rows.iter().all(|row| row.ends_with(": Accept-Encoding")),
            "gziphandler varies every answer",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, vary)?,
            &[
                "bytes_n_10_gzip: ",
                "bytes_n_1399_gzip: ",
                "bytes_n_1400_gzip: accept-encoding",
                "bytes_n_3000_gzip: accept-encoding",
                "sniff_kind_empty_gzip: ",
                "headers_gzip: ",
                "bytes_n_10_identity: ",
                "bytes_n_1399_identity: ",
                "bytes_n_1400_identity: accept-encoding",
                "bytes_n_3000_identity: accept-encoding",
                "sniff_kind_empty_identity: ",
                "headers_identity: ",
            ],
            "tower-http",
        )?;
        let gunzipped = |outcome: &Outcome| outcome.gunzipped.clone();
        let compressed = ["bytes_n_1400_gzip", "bytes_n_3000_gzip"];
        ensure_eq(
            &rows(&pairs, &compressed, rust, gunzipped)?,
            &rows(&pairs, &compressed, go, gunzipped)?,
            "the gunzipped bodies",
        )
    }

    /// Go's shutdown and hyper's agree on the connections that are idle, serving a request or
    /// holding one behind it, and on the listener; they differ on a connection whose first
    /// request has not fully arrived. net/http keeps such a connection for up to 5 s and drops a
    /// request it reads after the shutdown began without an answer; hyper closes a connection
    /// that has sent nothing at once and serves a head that was arriving.
    // Go: Server.Shutdown and closeIdleConns (net/http/server.go:3150-3223) and conn.serve's
    // shuttingDown check (net/http/server.go:2000), from shutdownAfterSignals
    // (server/server.go:196-207)
    #[test]
    fn shutdown_new_connections() -> TestResult {
        let doc = golden()?;
        let at = Duration::from_millis(count(&doc, "/shutdown/at_ms")?);
        ensure_eq(
            &Duration::from_millis(count(&doc, "/shutdown/timeout_ms")?),
            &SHUTDOWN_TIMEOUT,
            "the shutdown deadline",
        )?;
        let mut cases = Vec::new();
        for case in list(&doc, "/shutdown/cases")? {
            cases.push((Case::read(case)?, Outcome::go(case)?));
        }
        let (pairs, late, stopped) = runtime()?.block_on(async {
            let server = Server::start(stand_ins(), Budget::MAIN).await?;
            let runs: Vec<_> = cases
                .iter()
                .map(|(case, _)| task::spawn(run(server.address, case.clone())))
                .collect();
            time::sleep(at).await;
            server
                .stop
                .send(())
                .map_err(|()| "the server stopped early")?;
            time::sleep(at / 2).await;
            let late = match TcpStream::connect(server.address).await {
                Ok(_) => "accepted",
                Err(e) if e.kind() == ErrorKind::ConnectionRefused => "refused",
                Err(e) => return Err(BoxError::from(e)),
            };
            let stopped = server.task.await?;
            let mut pairs = Vec::new();
            for ((case, go), outcome) in cases.into_iter().zip(runs) {
                pairs.push(Pair {
                    case,
                    go,
                    rust: outcome.await??,
                });
            }
            Ok((pairs, late, stopped))
        })?;
        ensure_eq(
            text(&doc, "/shutdown/late_dial")?,
            late,
            "a dial after the shutdown began",
        )?;
        ensure_eq(text(&doc, "/shutdown/err")?, "<nil>", "net/http drained")?;
        ensure_eq(&stopped, &Stopped::Drained, "pbs-server drained")?;
        let names = [
            "idle",
            "in_flight",
            "pipelined",
            "silent",
            "partial_head",
            "never_sends",
        ];
        let close = |outcome: &Outcome| {
            format!(
                "{} | close={}",
                answer(outcome),
                outcome.field("connection").eq_ignore_ascii_case("close")
            )
        };
        ensure_eq(
            &rows(&pairs, &names, go, close)?,
            &[
                "idle: HTTP/1.1 200 OK | body=0 | eof | close=false",
                "in_flight: HTTP/1.1 200 OK | late | eof | close=true",
                "pipelined: HTTP/1.1 200 OK | late | eof | close=true",
                "silent:  |  | eof | close=false",
                "partial_head:  |  | eof | close=false",
                "never_sends:  |  | open | close=false",
            ],
            "net/http",
        )?;
        ensure_eq(
            &rows(&pairs, &names, rust, close)?,
            &[
                "idle: HTTP/1.1 200 OK | body=0 | eof | close=false",
                "in_flight: HTTP/1.1 200 OK | late | eof | close=true",
                "pipelined: HTTP/1.1 200 OK | late | eof | close=true",
                "silent:  |  | eof | close=false",
                "partial_head: HTTP/1.1 200 OK | body=0 | eof | close=true",
                "never_sends:  |  | eof | close=false",
            ],
            "hyper",
        )
    }

    /// The budgets the Go constructors set, as the golden records them: 15 s `ReadTimeout` (and
    /// `WriteTimeout`, which has no counterpart, `server.timeouts`) on the main and Unix-socket
    /// servers, none on the admin server.
    // Go: newAdminServer, newMainServer and newSocketServer (server/server.go:90-118)
    #[test]
    fn budgets_like_go() -> TestResult {
        let doc = golden()?;
        for (server, budget) in [
            ("main", Budget::MAIN),
            ("socket", Budget::MAIN),
            ("admin", Budget::NONE),
        ] {
            let read = count(&doc, &format!("/servers/{server}/read_timeout_ns"))?;
            let want = (read > 0).then(|| Duration::from_nanos(read));
            ensure_eq(&budget.read(), &want, server)?;
            ensure_eq(
                &count(&doc, &format!("/servers/{server}/write_timeout_ns"))?,
                &read,
                "the write budget equals the read budget",
            )?;
            ensure_eq(
                &count(&doc, &format!("/servers/{server}/read_header_timeout_ns"))?,
                &0,
                "no header budget of its own",
            )?;
        }
        Ok(())
    }
}
