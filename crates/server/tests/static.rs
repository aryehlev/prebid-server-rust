//! The pin of `/static` on tower-http's `ServeDir` against `http.FileServer` behind httprouter's
//! `ServeFiles`, as router/router.go:295 serves Prebid's static directory.
//!
//! The `static` cases of `testdata/golden/wire.json` are what `testdata/go/wire` read from
//! `ServeFiles("/static/*filepath", http.Dir(...))` behind `NoCache` and `SupportCORS`, serving a
//! fixture shaped like Prebid's static directory with every file modified at the golden's
//! `static_mod_time`. The same fixture is written here under Cargo's test directory and served
//! by `ServeDir` on `/static`, `/static/` and `/static/{*path}`, rooted at the fixture's parent
//! so that the request path is the file path, behind [`pbs_router::middleware::public`] on
//! [`pbs_server::serve`]. The `static_gzip` cases are the same handler behind gziphandler, as
//! server/server.go serves it with `compression.response.enable_gzip` set, and run here behind
//! [`pbs_server::compression::compression_enabled`].

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt::Debug,
        fs::{self, File},
        io::ErrorKind,
        net::SocketAddr,
        path::{Path, PathBuf},
        process,
        time::{Duration, SystemTime},
    };

    use axum::Router;
    use pbs_router::middleware;
    use pbs_server::{Budget, Stopped, compression, serve};
    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        runtime,
        sync::oneshot,
        task, time,
    };
    use tower_http::services::ServeDir;

    /// A failed pin.
    type BoxError = Box<dyn Error + Send + Sync>;

    /// What the pins return.
    type TestResult<T = ()> = Result<T, BoxError>;

    /// The fixture's files: the generator's `staticFiles` (handlers.go).
    const FILES: [(&str, &[u8]); 7] = [
        ("index.html", b"<html><body>Prebid Server</body></html>\n"),
        ("optout.html", b"<html><body>Opt out</body></html>\n"),
        (
            "pbs-logo.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>\n",
        ),
        ("notes.txt", b"hello, static\n"),
        ("bidder-params/appnexus.json", b"{\"title\":\"AppNexus\"}\n"),
        ("noext", b"\x00\x01\x02binary"),
        ("data.unknownext", b"plain words\n"),
    ];

    /// The fixture's big file and its length: the generator's `big.txt`, `pattern(3000)` bytes
    /// cycling through the alphabet, long enough for gzip.
    const BIG_FILE: (&str, usize) = ("big.txt", 3000);

    /// The fixture's modification time, 2020-01-02T03:04:05Z, in seconds since the epoch.
    const MOD_TIME: u64 = 1_577_934_245;

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

    /// The string at `pointer` in `doc`.
    fn text<'v>(doc: &'v Value, pointer: &str) -> TestResult<&'v str> {
        doc.pointer(pointer)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the golden has no string at {pointer}").into())
    }

    /// One static case: its request and Go's answer.
    #[derive(Debug, Clone)]
    struct Case {
        /// The case's name.
        name: String,
        /// The request, the case's one step.
        request: String,
        /// Go's answer.
        go: Answer,
    }

    /// One answer: its head (the Date value replaced by `<date>`) and its body.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Answer {
        /// The status line and the header fields.
        head: String,
        /// The body, as text.
        body: String,
    }

    impl Answer {
        /// Splits what a client read into head and body.
        fn read(bytes: &[u8]) -> Self {
            let text = String::from_utf8_lossy(bytes);
            let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
            Self {
                head: head
                    .split("\r\n")
                    .map(|line| match line.get(..6) {
                        Some(name) if name.eq_ignore_ascii_case("date: ") => {
                            format!("{name}<date>")
                        }
                        Some(_) | None => line.to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join("\r\n"),
                body: body.to_owned(),
            }
        }

        /// The status line.
        fn status(&self) -> &str {
            self.head.split("\r\n").next().unwrap_or_default()
        }

        /// The value of the field `name` (case-insensitive), or `-`.
        fn field(&self, name: &str) -> &str {
            self.head
                .split("\r\n")
                .skip(1)
                .filter_map(|line| line.split_once(": "))
                .find(|(field, _)| field.eq_ignore_ascii_case(name))
                .map_or("-", |(_, value)| value)
        }
    }

    /// The golden's cases of `group`, its header and the fixture's time checked.
    fn cases(group: &str) -> TestResult<Vec<Case>> {
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
        ensure_eq(
            text(&doc, "/static_mod_time")?,
            "2020-01-02T03:04:05Z",
            "the fixture's time",
        )?;
        doc.pointer("/cases")
            .and_then(Value::as_array)
            .ok_or("the golden has no cases")?
            .iter()
            .filter(|case| case.get("group").and_then(Value::as_str) == Some(group))
            .map(|case| {
                let head = text(case, "/head")?;
                Ok(Case {
                    name: text(case, "/name")?.to_owned(),
                    request: text(case, "/steps/0/text")?.to_owned(),
                    go: Answer {
                        head: head.strip_suffix("\r\n\r\n").unwrap_or(head).to_owned(),
                        body: text(case, "/body")?.to_owned(),
                    },
                })
            })
            .collect()
    }

    /// Writes the fixture under a fresh directory of Cargo's test directory, named after `group`,
    /// and returns that directory, the parent of `static`: every file and directory modified at
    /// [`MOD_TIME`].
    fn fixture(group: &str) -> TestResult<PathBuf> {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pbs-server-{group}-{}", process::id()));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let dir = root.join("static");
        for (path, content) in FILES {
            let file = dir.join(path);
            fs::create_dir_all(file.parent().ok_or("a file without a directory")?)?;
            fs::write(&file, content)?;
        }
        let (big, length) = BIG_FILE;
        fs::write(
            dir.join(big),
            (b'a'..=b'z').cycle().take(length).collect::<Vec<_>>(),
        )?;
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(MOD_TIME);
        let mut paths = vec![dir.clone(), dir.join("bidder-params"), dir.join(big)];
        paths.extend(FILES.iter().map(|(path, _)| dir.join(path)));
        for path in paths {
            File::open(path)?.set_modified(time)?;
        }
        Ok(root)
    }

    /// Sends `request` to `address` on a fresh connection and reads the answer until the server
    /// closes the connection.
    async fn exchange(address: SocketAddr, request: String) -> TestResult<Answer> {
        let mut stream = TcpStream::connect(address).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut answer = Vec::new();
        match time::timeout(Duration::from_secs(20), stream.read_to_end(&mut answer)).await? {
            Ok(_) => Ok(Answer::read(&answer)),
            Err(e) if e.kind() == ErrorKind::ConnectionReset => Ok(Answer::read(&answer)),
            Err(e) => Err(e.into()),
        }
    }

    /// `/static` as the pin serves it: `ServeDir` rooted at `root` on the three paths that
    /// reach the static directory, behind Prebid's middleware.
    fn static_files(root: &Path) -> Router {
        let files = ServeDir::new(root);
        middleware::public(
            Router::new()
                .route_service("/static", files.clone())
                .route_service("/static/", files.clone())
                .route_service("/static/{*path}", files),
        )
    }

    /// Runs every case of `group` at once against [`static_files`] on [`serve`], behind gzip
    /// when `gzip` is set, and pairs the answers with Go's.
    fn run(group: &str, gzip: bool) -> TestResult<Vec<(Case, Answer)>> {
        let cases = cases(group)?;
        let root = fixture(group)?;
        let router = compression::compression_enabled(static_files(&root), gzip);
        let pairs = runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let address = listener.local_addr()?;
                let (stop, stopped) = oneshot::channel::<()>();
                let server = task::spawn(serve(listener, router, Budget::MAIN, async move {
                    stopped.await.unwrap_or_default();
                }));
                let runs: Vec<_> = cases
                    .iter()
                    .map(|case| task::spawn(exchange(address, case.request.clone())))
                    .collect();
                let mut pairs = Vec::new();
                for (case, run) in cases.into_iter().zip(runs) {
                    pairs.push((case, run.await??));
                }
                stop.send(()).map_err(|()| "the server stopped early")?;
                ensure_eq(&server.await?, &Stopped::Drained, "the server's shutdown")?;
                Ok::<_, BoxError>(pairs)
            })?;
        fs::remove_dir_all(root)?;
        Ok(pairs)
    }

    /// The parts of a static answer the pin compares.
    fn summary(answer: &Answer) -> String {
        let has = |name: &str| {
            if answer.field(name) == "-" {
                "no"
            } else {
                "yes"
            }
        };
        format!(
            "{} | type {} | location {} | modified {} | cache {} | {:?}",
            answer.status(),
            answer.field("content-type"),
            answer.field("location"),
            has("last-modified"),
            has("cache-control"),
            answer.body.lines().next().unwrap_or_default()
        )
    }

    /// What `http.FileServer` behind httprouter answered to each static case.
    const FILE_SERVER: [&str; 21] = [
        "text_file: HTTP/1.1 200 OK | type text/plain; charset=utf-8 | location - | \
         modified yes | cache yes | \"hello, static\"",
        "html_file: HTTP/1.1 200 OK | type text/html; charset=utf-8 | location - | \
         modified yes | cache yes | \"<html><body>Opt out</body></html>\"",
        "svg_file: HTTP/1.1 200 OK | type image/svg+xml | location - | modified yes | \
         cache yes | \"<svg xmlns=\\\"http://www.w3.org/2000/svg\\\"></svg>\"",
        "json_file: HTTP/1.1 200 OK | type application/json | location - | modified yes | \
         cache yes | \"{\\\"title\\\":\\\"AppNexus\\\"}\"",
        "no_extension: HTTP/1.1 200 OK | type application/octet-stream | location - | \
         modified yes | cache yes | \"\\0\\u{1}\\u{2}binary\"",
        "unknown_extension: HTTP/1.1 200 OK | type text/plain; charset=utf-8 | location - \
         | modified yes | cache yes | \"plain words\"",
        "directory_index: HTTP/1.1 200 OK | type text/html; charset=utf-8 | location - | \
         modified yes | cache yes | \"<html><body>Prebid Server</body></html>\"",
        "index_file: HTTP/1.1 301 Moved Permanently | type - | location ./ | modified no \
         | cache yes | \"\"",
        "directory_listing: HTTP/1.1 200 OK | type text/html; charset=utf-8 | location - \
         | modified yes | cache yes | \"<!doctype html>\"",
        "directory_without_slash: HTTP/1.1 301 Moved Permanently | type - | location \
         bidder-params/ | modified no | cache yes | \"\"",
        "static_without_slash: HTTP/1.1 301 Moved Permanently | type text/html; \
         charset=utf-8 | location /static/ | modified no | cache yes | \"<a \
         href=\\\"/static/\\\">Moved Permanently</a>.\"",
        "file_with_slash: HTTP/1.1 301 Moved Permanently | type - | location \
         ../notes.txt | modified no | cache yes | \"\"",
        "missing_file: HTTP/1.1 404 Not Found | type text/plain; charset=utf-8 | location \
         - | modified no | cache no | \"404 page not found\"",
        "dot_dot: HTTP/1.1 404 Not Found | type text/plain; charset=utf-8 | location - | \
         modified no | cache no | \"404 page not found\"",
        "encoded_dot_dot: HTTP/1.1 404 Not Found | type text/plain; charset=utf-8 | \
         location - | modified no | cache no | \"404 page not found\"",
        "range: HTTP/1.1 206 Partial Content | type text/plain; charset=utf-8 | location - \
         | modified yes | cache yes | \"hello\"",
        "bad_range: HTTP/1.1 416 Requested Range Not Satisfiable | type text/plain; \
         charset=utf-8 | location - | modified no | cache no | \"invalid range: failed \
         to overlap\"",
        "not_modified: HTTP/1.1 304 Not Modified | type - | location - | modified yes | \
         cache yes | \"\"",
        "modified: HTTP/1.1 200 OK | type text/plain; charset=utf-8 | location - | \
         modified yes | cache yes | \"hello, static\"",
        "head_file: HTTP/1.1 405 Method Not Allowed | type text/plain; charset=utf-8 | \
         location - | modified no | cache yes | \"\"",
        "post_file: HTTP/1.1 405 Method Not Allowed | type text/plain; charset=utf-8 | \
         location - | modified no | cache yes | \"Method Not Allowed\"",
    ];

    /// What `ServeDir` behind Prebid's middleware answers to each static case.
    const SERVE_DIR: [&str; 21] = [
        "text_file: HTTP/1.1 200 OK | type text/plain | location - | modified yes | cache \
         yes | \"hello, static\"",
        "html_file: HTTP/1.1 200 OK | type text/html | location - | modified yes | cache \
         yes | \"<html><body>Opt out</body></html>\"",
        "svg_file: HTTP/1.1 200 OK | type image/svg+xml | location - | modified yes | \
         cache yes | \"<svg xmlns=\\\"http://www.w3.org/2000/svg\\\"></svg>\"",
        "json_file: HTTP/1.1 200 OK | type application/json | location - | modified yes | \
         cache yes | \"{\\\"title\\\":\\\"AppNexus\\\"}\"",
        "no_extension: HTTP/1.1 200 OK | type application/octet-stream | location - | \
         modified yes | cache yes | \"\\0\\u{1}\\u{2}binary\"",
        "unknown_extension: HTTP/1.1 200 OK | type application/octet-stream | location - \
         | modified yes | cache yes | \"plain words\"",
        "directory_index: HTTP/1.1 200 OK | type text/html | location - | modified yes | \
         cache yes | \"<html><body>Prebid Server</body></html>\"",
        "index_file: HTTP/1.1 200 OK | type text/html | location - | modified yes | cache \
         yes | \"<html><body>Prebid Server</body></html>\"",
        "directory_listing: HTTP/1.1 404 Not Found | type - | location - | modified no | \
         cache yes | \"\"",
        "directory_without_slash: HTTP/1.1 307 Temporary Redirect | type - | location \
         /static/bidder-params/ | modified no | cache yes | \"\"",
        "static_without_slash: HTTP/1.1 307 Temporary Redirect | type - | location \
         /static/ | modified no | cache yes | \"\"",
        "file_with_slash: HTTP/1.1 200 OK | type text/plain | location - | modified yes | \
         cache yes | \"hello, static\"",
        "missing_file: HTTP/1.1 404 Not Found | type - | location - | modified no | cache \
         yes | \"\"",
        "dot_dot: HTTP/1.1 404 Not Found | type - | location - | modified no | cache yes | \
         \"\"",
        "encoded_dot_dot: HTTP/1.1 404 Not Found | type - | location - | modified no | \
         cache yes | \"\"",
        "range: HTTP/1.1 206 Partial Content | type text/plain | location - | modified yes \
         | cache yes | \"hello\"",
        "bad_range: HTTP/1.1 416 Range Not Satisfiable | type text/plain | location - | \
         modified yes | cache yes | \"\"",
        "not_modified: HTTP/1.1 304 Not Modified | type - | location - | modified no | \
         cache yes | \"\"",
        "modified: HTTP/1.1 200 OK | type text/plain | location - | modified yes | cache \
         yes | \"hello, static\"",
        "head_file: HTTP/1.1 200 OK | type text/plain | location - | modified yes | cache \
         yes | \"\"",
        "post_file: HTTP/1.1 405 Method Not Allowed | type - | location - | modified no | \
         cache yes | \"\"",
    ];

    /// `/static`: `http.FileServer` behind `ServeFiles` lists a directory without
    /// `index.html`, redirects `/index.html` to `./`, a directory without its slash (301, a
    /// relative Location) and a file with one, types files with mime's table (`text/plain;
    /// charset=utf-8`) and sniffs the ones whose extension it does not know, answers errors with
    /// its own text and without `Cache-Control` (Go 1.23's `serveError`), and httprouter answers
    /// HEAD with 405. tower-http's `ServeDir` answers a directory without `index.html` with 404,
    /// serves `/index.html` and a file with a trailing slash, redirects a directory with 307 to
    /// an absolute Location, types files with `mime_guess`'s table (`text/plain`, unknown
    /// extensions `application/octet-stream`), answers errors with an empty body and `NoCache`'s
    /// headers kept, sends no Last-Modified with a 304 and serves HEAD. Files, ranges and the
    /// conditional requests are otherwise answered alike.
    // Go: ServeFiles (httprouter v1.3.0 router.go:296-307) over FileServer: serveFile
    // (net/http/fs.go:679-762), dirList (fs.go:139), serveError (fs.go:188-212) and serveContent's
    // typing (fs.go:274-293)
    #[test]
    fn file_server() -> TestResult {
        let pairs = run("static", false)?;
        let go: Vec<String> = pairs
            .iter()
            .map(|(case, _)| format!("{}: {}", case.name, summary(&case.go)))
            .collect();
        ensure_eq(&go, &FILE_SERVER, "http.FileServer")?;
        let rust: Vec<String> = pairs
            .iter()
            .map(|(case, rust)| format!("{}: {}", case.name, summary(rust)))
            .collect();
        ensure_eq(&rust, &SERVE_DIR, "ServeDir")
    }

    /// `/static` with gzip on: gziphandler compresses a file answer of 1,400 bytes or more,
    /// a 206 range included, and keeps `http.FileServer`'s `Accept-Ranges`; tower-http compresses
    /// the whole file and drops its `accept-ranges`, and sends a range as it is. A range under
    /// 1,400 bytes is sent as it is on both.
    // Go: GzipResponseWriter.Write compresses when the handler's Content-Length is 1,400 or more,
    // and startGzip touches neither Accept-Ranges nor the status (gziphandler v1.1.1
    // gzip.go:100-183)
    #[test]
    fn gzip_ranges() -> TestResult {
        let pairs = run("static_gzip", true)?;
        let summary = |answer: &Answer| {
            format!(
                "{} | encoding {} | accept-ranges {} | range {}",
                answer.status(),
                answer.field("content-encoding"),
                answer.field("accept-ranges"),
                answer.field("content-range")
            )
        };
        let go: Vec<String> = pairs
            .iter()
            .map(|(case, _)| format!("{}: {}", case.name, summary(&case.go)))
            .collect();
        ensure_eq(
            &go,
            &[
                "big_file_identity: HTTP/1.1 200 OK | encoding - | accept-ranges bytes | range -",
                "big_file_gzip: HTTP/1.1 200 OK | encoding gzip | accept-ranges bytes | range -",
                "big_range_gzip: HTTP/1.1 206 Partial Content | encoding gzip | accept-ranges \
                 bytes | range bytes 0-1499/3000",
                "small_range_gzip: HTTP/1.1 206 Partial Content | encoding - | accept-ranges \
                 bytes | range bytes 0-99/3000",
            ],
            "gziphandler",
        )?;
        let rust: Vec<String> = pairs
            .iter()
            .map(|(case, rust)| format!("{}: {}", case.name, summary(rust)))
            .collect();
        ensure_eq(
            &rust,
            &[
                "big_file_identity: HTTP/1.1 200 OK | encoding - | accept-ranges bytes | range -",
                "big_file_gzip: HTTP/1.1 200 OK | encoding gzip | accept-ranges - | range -",
                "big_range_gzip: HTTP/1.1 206 Partial Content | encoding - | accept-ranges \
                 bytes | range bytes 0-1499/3000",
                "small_range_gzip: HTTP/1.1 206 Partial Content | encoding - | accept-ranges \
                 bytes | range bytes 0-99/3000",
            ],
            "tower-http",
        )
    }
}
