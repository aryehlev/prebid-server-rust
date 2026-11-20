//! Pins of axum's routing behind pbs-router's middleware against httprouter v1.3.0 behind
//! `router.NoCache` and `router.SupportCORS`.
//!
//! `testdata/golden/routing.json` is what `testdata/go` read from Prebid's public handler, built
//! as main.go builds it, on loopback: router/router.go's routes, each answered by a stand-in that
//! names the route, its parameters and the body bytes it read, and requests at httprouter's and
//! net/http's edges (unknown paths, redirects, 405 and `Allow`, OPTIONS, HEAD, percent-encoded
//! paths, a body over axum's default limit and the middleware's headers). The same routes, with
//! httprouter's `:name` and `*name` as axum's `{name}` and `{*name}`, run here with the same
//! stand-ins behind [`pbs_router::middleware::public`] on [`pbs_server::serve`], and the same
//! bytes go to them.

#[cfg(test)]
mod tests {
    use std::{
        error::Error, fmt::Debug, fs, io::ErrorKind, net::SocketAddr, pin::pin, time::Duration,
    };

    use axum::{
        Router,
        body::Bytes,
        extract::Path,
        routing::{MethodFilter, on},
    };
    use pbs_router::middleware;
    use pbs_server::{Budget, Stopped, serve};
    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream, tcp::ReadHalf},
        runtime::{self, Runtime},
        sync::oneshot,
        task, time,
    };

    /// A failed pin.
    type BoxError = Box<dyn Error + Send + Sync>;

    /// What the pins return.
    type TestResult<T = ()> = Result<T, BoxError>;

    /// A stand-in's path parameters, decoded, in the order of the route's path.
    type Params = Option<Path<Vec<(String, String)>>>;

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

    /// The array at `pointer` in `doc`.
    fn list<'v>(doc: &'v Value, pointer: &str) -> TestResult<&'v [Value]> {
        doc.pointer(pointer)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("the golden has no array at {pointer}").into())
    }

    /// `testdata/golden/routing.json`, its header checked: written by the routing generator on
    /// amd64 (D-ARCH) with httprouter v1.3.0.
    fn golden() -> TestResult<Value> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/routing.json");
        let doc: Value = serde_json::from_slice(&fs::read(path)?)?;
        ensure_eq(
            text(&doc, "/header/generator")?,
            "crates/router/testdata/go",
            "the golden's generator",
        )?;
        ensure_eq(
            text(&doc, "/header/goarch")?,
            "amd64",
            "the golden's goarch",
        )?;
        ensure_eq(
            text(
                &doc,
                "/header/modules/github.com~1julienschmidt~1httprouter",
            )?,
            "v1.3.0",
            "the golden's httprouter",
        )?;
        Ok(doc)
    }

    /// One request of the golden and Go's answer to it.
    #[derive(Debug, Clone)]
    struct Case {
        /// The pin group.
        group: String,
        /// The case's name.
        name: String,
        /// The request line and headers, with their blank line.
        request_head: String,
        /// How many pattern bytes follow the head.
        body_len: usize,
        /// Go's answer.
        go: Answer,
    }

    /// One answer: its head without the blank line (the Date value replaced by `<date>`) and its
    /// body.
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

        /// The values of the field `name` (case-insensitive), joined with `, `, or `-` when the
        /// answer has none.
        fn field(&self, name: &str) -> String {
            let values: Vec<&str> = self
                .head
                .split("\r\n")
                .skip(1)
                .filter_map(|line| line.split_once(": "))
                .filter(|(field, _)| field.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
                .collect();
            if values.is_empty() {
                "-".to_owned()
            } else {
                values.join(", ")
            }
        }
    }

    /// The golden's cases.
    fn cases(doc: &Value) -> TestResult<Vec<Case>> {
        list(doc, "/cases")?
            .iter()
            .map(|case| {
                Ok(Case {
                    group: text(case, "/group")?.to_owned(),
                    name: text(case, "/name")?.to_owned(),
                    request_head: text(case, "/request_head")?.to_owned(),
                    body_len: case
                        .get("body_len")
                        .and_then(Value::as_u64)
                        .map_or(Ok(0), usize::try_from)?,
                    go: Answer {
                        head: text(case, "/response_head")?.to_owned(),
                        body: text(case, "/body")?.to_owned(),
                    },
                })
            })
            .collect()
    }

    /// httprouter's path syntax in axum's: `:name` is `{name}` and `*name` is `{*name}`.
    fn axum_path(path: &str) -> String {
        path.split('/')
            .map(|segment| {
                segment.strip_prefix(':').map_or_else(
                    || {
                        segment
                            .strip_prefix('*')
                            .map_or_else(|| segment.to_owned(), |name| format!("{{*{name}}}"))
                    },
                    |name| format!("{{{name}}}"),
                )
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// What a stand-in answers: `route=<name>`, each parameter as ` key=value` and ` body=<n>`,
    /// on one line, as the generator's stub writes it. httprouter's catch-all value starts with
    /// the slash before it, which axum's leaves out, so `catch_all` puts it back.
    fn line(name: &str, params: Params, body: &Bytes, catch_all: bool) -> String {
        let slash = if catch_all { "/" } else { "" };
        let mut words = vec![format!("route={name}")];
        words.extend(
            params
                .map_or_default(|Path(params)| params)
                .into_iter()
                .map(|(key, value)| format!("{key}={slash}{value}")),
        );
        words.push(format!("body={}", body.len()));
        format!("{}\n", words.join(" "))
    }

    /// router/router.go's routes as the golden lists them, each answered by its stand-in. A
    /// catch-all route also gets the route of its bare prefix, `/static/`, which httprouter's
    /// catch-all matches with the value `/` and axum's does not match.
    fn routes(doc: &Value) -> TestResult<Router> {
        let mut router = Router::new();
        for route in list(doc, "/routes")? {
            let method = match text(route, "/method")? {
                "GET" => MethodFilter::GET,
                "POST" => MethodFilter::POST,
                other => return Err(format!("no method filter for {other}").into()),
            };
            let path = text(route, "/path")?;
            let name = text(route, "/name")?.to_owned();
            let catch_all = path.contains('*');
            let stand_in = name.clone();
            router = router.route(
                &axum_path(path),
                on(method, move |params: Params, body: Bytes| {
                    let answer = line(&stand_in, params, &body, catch_all);
                    async move { answer }
                }),
            );
            if let Some((prefix, key)) = path.split_once('*') {
                let key = key.to_owned();
                router = router.route(
                    prefix,
                    on(method, move |body: Bytes| {
                        let answer = format!("route={name} {key}=/ body={}\n", body.len());
                        async move { answer }
                    }),
                );
            }
        }
        Ok(router)
    }

    /// The bytes the generator sends after a request head: `n` bytes cycling through the
    /// alphabet.
    fn pattern(n: usize) -> Vec<u8> {
        (b'a'..=b'z').cycle().take(n).collect()
    }

    /// Reads until the server closes the connection or resets it; a reset after an answer is
    /// expected when the server answers before it reads the whole body.
    async fn read_all(reader: &mut ReadHalf<'_>) -> TestResult<Vec<u8>> {
        let mut answer = Vec::new();
        let mut buf = vec![0; 64 << 10];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => return Ok(answer),
                Ok(n) => answer.extend_from_slice(buf.get(..n).unwrap_or_default()),
                Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok(answer),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Sends `case`'s request to `address` on a fresh connection and reads the answer. The head
    /// and body go out while the answer is read, and a write error is ignored, as the generator
    /// ignores it: the server may answer and close before it reads the body.
    async fn exchange(address: SocketAddr, case: Case) -> TestResult<Answer> {
        let mut stream = TcpStream::connect(address).await?;
        let mut request = case.request_head.into_bytes();
        request.extend(pattern(case.body_len));
        let (mut reader, mut writer) = stream.split();
        let mut reading = pin!(read_all(&mut reader));
        let mut writing = pin!(writer.write_all(&request));
        let mut written = false;
        let answer = time::timeout(Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    answer = &mut reading => break answer,
                    _ = &mut writing, if !written => written = true,
                }
            }
        })
        .await??;
        Ok(Answer::read(&answer))
    }

    /// A runtime for the loopback servers and clients.
    fn runtime() -> TestResult<Runtime> {
        Ok(runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?)
    }

    /// One golden case with the Rust server's answer.
    #[derive(Debug)]
    struct Pair {
        /// The case, with Go's answer.
        case: Case,
        /// The Rust server's answer.
        rust: Answer,
    }

    /// Sends every case of `groups` at once to `router` behind [`middleware::public`] on
    /// [`serve`] (the main server's budget), and pairs the answers with Go's.
    fn run_groups(groups: &[&str]) -> TestResult<Vec<Pair>> {
        let doc = golden()?;
        let router = middleware::public(routes(&doc)?);
        let cases: Vec<Case> = cases(&doc)?
            .into_iter()
            .filter(|case| groups.contains(&case.group.as_str()))
            .collect();
        runtime()?.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let (stop, stopped) = oneshot::channel::<()>();
            let server = task::spawn(serve(listener, router, Budget::MAIN, async move {
                stopped.await.unwrap_or_default();
            }));
            let runs: Vec<_> = cases
                .iter()
                .map(|case| task::spawn(exchange(address, case.clone())))
                .collect();
            let mut pairs = Vec::new();
            for (case, run) in cases.into_iter().zip(runs) {
                pairs.push(Pair {
                    case,
                    rust: run.await??,
                });
            }
            stop.send(()).map_err(|()| "the server stopped early")?;
            ensure_eq(&server.await?, &Stopped::Drained, "the server's shutdown")?;
            Ok(pairs)
        })
    }

    /// Sends the golden cases named `names` at once to the same routes behind the same
    /// middleware, served by `axum::serve` as it comes: axum's 2 MiB `DefaultBodyLimit` on, which
    /// pbs-server turns off.
    fn run_axum_default(names: &[&str]) -> TestResult<Vec<String>> {
        let doc = golden()?;
        let router = middleware::public(routes(&doc)?);
        let cases: Vec<Case> = cases(&doc)?
            .into_iter()
            .filter(|case| names.contains(&case.name.as_str()))
            .collect();
        runtime()?.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            drop(task::spawn(
                async move { axum::serve(listener, router).await },
            ));
            let runs: Vec<_> = cases
                .into_iter()
                .map(|case| (case.name.clone(), task::spawn(exchange(address, case))))
                .collect();
            let mut answers = Vec::new();
            for (name, run) in runs {
                let answer = run.await??;
                answers.push(format!(
                    "{name}: {} | {}",
                    answer.status(),
                    body_line(&answer)
                ));
            }
            Ok(answers)
        })
    }

    /// The first line of an answer's body.
    fn body_line(answer: &Answer) -> &str {
        answer.body.lines().next().unwrap_or_default()
    }

    /// `name: <what>` for each case of `pairs`, of one side.
    fn rows<F: Fn(&Answer) -> String>(
        pairs: &[Pair],
        side: fn(&Pair) -> &Answer,
        what: F,
    ) -> Vec<String> {
        pairs
            .iter()
            .map(|pair| format!("{}: {}", pair.case.name, what(side(pair))))
            .collect()
    }

    /// Go's side of a pair.
    const fn go(pair: &Pair) -> &Answer {
        &pair.case.go
    }

    /// The Rust server's side of a pair.
    const fn rust(pair: &Pair) -> &Answer {
        &pair.rust
    }

    /// A path no route matches: httprouter answers with net/http's `NotFound`, `404 page not
    /// found` as text/plain; charset=utf-8 with `X-Content-Type-Options: nosniff`; axum's
    /// fallback answers 404 with an empty body and neither header (D-ROUTER).
    // Go: httprouter v1.3.0 router.go:446-451 calls http.NotFound when Router.NotFound is nil,
    // and http.Error sets the type and nosniff (net/http/server.go:2301-2322)
    #[test]
    fn not_found_differs() -> TestResult {
        let pairs = run_groups(&["not_found"])?;
        let what = |answer: &Answer| {
            format!(
                "{} | type {} | nosniff {} | {:?}",
                answer.status(),
                answer.field("content-type"),
                answer.field("x-content-type-options"),
                answer.body
            )
        };
        let go_not_found = "HTTP/1.1 404 Not Found | type text/plain; charset=utf-8 | nosniff \
                            nosniff | \"404 page not found\\n\"";
        ensure_eq(
            &rows(&pairs, go, what),
            &[
                format!("unknown_path: {go_not_found}"),
                format!("route_prefix: {go_not_found}"),
                format!("extra_segment: {go_not_found}"),
                format!("unknown_path_post: {go_not_found}"),
            ],
            "httprouter",
        )?;
        let rust_not_found = "HTTP/1.1 404 Not Found | type - | nosniff - | \"\"";
        ensure_eq(
            &rows(&pairs, rust, what),
            &[
                format!("unknown_path: {rust_not_found}"),
                format!("route_prefix: {rust_not_found}"),
                format!("extra_segment: {rust_not_found}"),
                format!("unknown_path_post: {rust_not_found}"),
            ],
            "axum",
        )
    }

    /// httprouter redirects a path that a route matches but for a trailing slash, its case or
    /// its cleaning (`//`, `..`): 301 for GET and 307 for any other method, to the route's path
    /// with the query kept. axum redirects nothing: each of those paths gets its 404 (D-ROUTER).
    // Go: RedirectTrailingSlash and RedirectFixedPath, on by default (httprouter v1.3.0
    // router.go:389-418)
    #[test]
    fn no_redirects() -> TestResult {
        let pairs = run_groups(&["redirects"])?;
        let what = |answer: &Answer| format!("{} | {}", answer.status(), answer.field("location"));
        ensure_eq(
            &rows(&pairs, go, what),
            &[
                "trailing_slash: HTTP/1.1 301 Moved Permanently | /status",
                "trailing_slash_post: HTTP/1.1 307 Temporary Redirect | /openrtb2/auction",
                "trailing_slash_query: HTTP/1.1 301 Moved Permanently | /status?a=1",
                "missing_slash: HTTP/1.1 301 Moved Permanently | /static/",
                "case_fixed: HTTP/1.1 301 Moved Permanently | /status",
                "case_fixed_post: HTTP/1.1 307 Temporary Redirect | /openrtb2/auction",
                "case_fixed_param: HTTP/1.1 301 Moved Permanently | /info/bidders/AppNexus",
                "double_slash: HTTP/1.1 301 Moved Permanently | /status",
                "dot_segments: HTTP/1.1 301 Moved Permanently | /status",
            ],
            "httprouter",
        )?;
        ensure_eq(
            &rows(&pairs, rust, what),
            &[
                "trailing_slash: HTTP/1.1 404 Not Found | -",
                "trailing_slash_post: HTTP/1.1 404 Not Found | -",
                "trailing_slash_query: HTTP/1.1 404 Not Found | -",
                "missing_slash: HTTP/1.1 404 Not Found | -",
                "case_fixed: HTTP/1.1 404 Not Found | -",
                "case_fixed_post: HTTP/1.1 404 Not Found | -",
                "case_fixed_param: HTTP/1.1 404 Not Found | -",
                "double_slash: HTTP/1.1 404 Not Found | -",
                "dot_segments: HTTP/1.1 404 Not Found | -",
            ],
            "axum",
        )
    }

    /// A method no route of the path takes: httprouter answers 405 with `Method Not Allowed` as
    /// text and `Allow` listing the path's methods sorted, joined with `, `, OPTIONS added, and
    /// answers an OPTIONS on a routed path itself with that `Allow` (net/http answers `OPTIONS *`
    /// before any handler). axum answers 405 with an empty body and its own `Allow` (the
    /// methods in the order they were routed, joined with `,`, HEAD with every GET); every
    /// OPTIONS, `*` and unknown paths included, gets tower-http's CORS answer
    /// (`server.cors-actual-requests`), with that `Allow` where the path is routed (D-ROUTER).
    // Go: HandleMethodNotAllowed and HandleOPTIONS, on by default (httprouter v1.3.0
    // router.go:422-444), and serverHandler.ServeHTTP, which gives `OPTIONS *` to
    // globalOptionsHandler (net/http/server.go:3302-3312)
    #[test]
    fn method_not_allowed_differs() -> TestResult {
        let pairs = run_groups(&["method_not_allowed", "options"])?;
        let what = |answer: &Answer| {
            format!(
                "{} | allow {} | cors {} | {:?}",
                answer.status(),
                answer.field("allow"),
                answer.field("access-control-allow-methods"),
                answer.body
            )
        };
        ensure_eq(
            &rows(&pairs, go, what),
            &[
                "put_get_route: HTTP/1.1 405 Method Not Allowed | allow GET, OPTIONS | cors - | \
                 \"Method Not Allowed\\n\"",
                "delete_post_route: HTTP/1.1 405 Method Not Allowed | allow OPTIONS, POST | cors - \
                 | \"Method Not Allowed\\n\"",
                "get_post_route: HTTP/1.1 405 Method Not Allowed | allow OPTIONS, POST | cors - | \
                 \"Method Not Allowed\\n\"",
                "patch_get_post_route: HTTP/1.1 405 Method Not Allowed | allow GET, OPTIONS, POST \
                 | cors - | \"Method Not Allowed\\n\"",
                "post_param_route: HTTP/1.1 405 Method Not Allowed | allow GET, OPTIONS | cors - | \
                 \"Method Not Allowed\\n\"",
                "options_get_route: HTTP/1.1 200 OK | allow GET, OPTIONS | cors - | \"\"",
                "options_post_route: HTTP/1.1 200 OK | allow OPTIONS, POST | cors - | \"\"",
                "options_unknown: HTTP/1.1 404 Not Found | allow - | cors - | \"404 page not \
                 found\\n\"",
                "options_asterisk: HTTP/1.1 200 OK | allow - | cors - | \"\"",
            ],
            "httprouter",
        )?;
        ensure_eq(
            &rows(&pairs, rust, what),
            &[
                "put_get_route: HTTP/1.1 405 Method Not Allowed | allow GET,HEAD | cors - | \"\"",
                "delete_post_route: HTTP/1.1 405 Method Not Allowed | allow POST | cors - | \"\"",
                "get_post_route: HTTP/1.1 405 Method Not Allowed | allow POST | cors - | \"\"",
                "patch_get_post_route: HTTP/1.1 405 Method Not Allowed | allow POST,GET,HEAD | \
                 cors - | \"\"",
                "post_param_route: HTTP/1.1 405 Method Not Allowed | allow GET,HEAD | cors - | \
                 \"\"",
                "options_get_route: HTTP/1.1 200 OK | allow GET,HEAD | cors GET,POST,HEAD | \"\"",
                "options_post_route: HTTP/1.1 200 OK | allow POST | cors GET,POST,HEAD | \"\"",
                "options_unknown: HTTP/1.1 200 OK | allow - | cors GET,POST,HEAD | \"\"",
                "options_asterisk: HTTP/1.1 200 OK | allow - | cors GET,POST,HEAD | \"\"",
            ],
            "axum",
        )
    }

    /// router/router.go routes no HEAD, so httprouter answers HEAD on a GET route with 405 and
    /// `Allow: GET, OPTIONS`; axum's GET routes take HEAD, run the GET handler and send its head
    /// without the body (D-ROUTER).
    // Go: router/router.go:285-318 routes no HEAD, and Handle adds a route to its method's tree
    // only (httprouter v1.3.0 router.go:244)
    #[test]
    fn head_runs_get() -> TestResult {
        let pairs = run_groups(&["head"])?;
        let what = |answer: &Answer| {
            format!(
                "{} | allow {} | length {} | type {}",
                answer.status(),
                answer.field("allow"),
                answer.field("content-length"),
                answer.field("content-type")
            )
        };
        ensure_eq(
            &rows(&pairs, go, what),
            &[
                "head_get_route: HTTP/1.1 405 Method Not Allowed | allow GET, OPTIONS | length 19 \
                 | type text/plain; charset=utf-8",
                "head_post_route: HTTP/1.1 405 Method Not Allowed | allow OPTIONS, POST | length \
                 19 | type text/plain; charset=utf-8",
                "head_unknown: HTTP/1.1 404 Not Found | allow - | length 19 | type text/plain; \
                 charset=utf-8",
            ],
            "httprouter",
        )?;
        ensure_eq(
            &rows(&pairs, rust, what),
            &[
                "head_get_route: HTTP/1.1 200 OK | allow - | length 20 | type text/plain; \
                 charset=utf-8",
                "head_post_route: HTTP/1.1 405 Method Not Allowed | allow POST | length 0 | type \
                 -",
                "head_unknown: HTTP/1.1 404 Not Found | allow - | length - | type -",
            ],
            "axum",
        )
    }

    /// httprouter matches the decoded path net/http gives it, so `/st%61tus` is `/status` and
    /// `%2F` a slash, and net/http answers an invalid escape with 400 before any handler; axum
    /// matches the path as written, so an escape in a literal segment misses the route, `%2F`
    /// stays inside its segment and an invalid escape is routed (to its 404). The parameter values
    /// a handler reads with axum's `Path` are decoded either way.
    // Go: ServeHTTP routes req.URL.Path (httprouter v1.3.0 router.go:383), which url.Parse
    // decodes (setPath, net/url/url.go:659)
    #[test]
    fn raw_path_routing() -> TestResult {
        let pairs = run_groups(&["decoded_path"])?;
        let what = |answer: &Answer| format!("{} | {}", answer.status(), body_line(answer));
        ensure_eq(
            &rows(&pairs, go, what),
            &[
                "encoded_letter: HTTP/1.1 200 OK | route=status body=0",
                "encoded_param: HTTP/1.1 200 OK | route=bidder bidderName=appnexus body=0",
                "encoded_slash_param: HTTP/1.1 404 Not Found | 404 page not found",
                "encoded_trailing_slash: HTTP/1.1 301 Moved Permanently | <a \
                 href=\"/status\">Moved Permanently</a>.",
                "encoded_query_mark: HTTP/1.1 404 Not Found | 404 page not found",
                "invalid_escape: HTTP/1.1 400 Bad Request | 400 Bad Request",
                "static_root: HTTP/1.1 200 OK | route=static filepath=/ body=0",
                "static_file: HTTP/1.1 200 OK | route=static filepath=/a b/c.txt body=0",
            ],
            "httprouter",
        )?;
        ensure_eq(
            &rows(&pairs, rust, what),
            &[
                "encoded_letter: HTTP/1.1 404 Not Found | ",
                "encoded_param: HTTP/1.1 200 OK | route=bidder bidderName=appnexus body=0",
                "encoded_slash_param: HTTP/1.1 200 OK | route=bidder bidderName=a/b body=0",
                "encoded_trailing_slash: HTTP/1.1 404 Not Found | ",
                "encoded_query_mark: HTTP/1.1 404 Not Found | ",
                "invalid_escape: HTTP/1.1 404 Not Found | ",
                "static_root: HTTP/1.1 200 OK | route=static filepath=/ body=0",
                "static_file: HTTP/1.1 200 OK | route=static filepath=/a b/c.txt body=0",
            ],
            "axum",
        )
    }

    /// net/http sets no limit on request bodies, so a 3 MiB auction body reaches the handler;
    /// so it does on pbs-server, which turns axum's 2 MiB `DefaultBodyLimit` off, where
    /// `axum::serve` as it comes answers the same request with 413 before the handler runs.
    // Go: http.Server reads bodies without a limit (net/http/server.go); Prebid's endpoints read
    // at most max_request_size themselves (endpoints/openrtb2/auction.go)
    #[test]
    fn no_library_body_limit() -> TestResult {
        let pairs = run_groups(&["body_limit"])?;
        let what = |answer: &Answer| format!("{} | {}", answer.status(), body_line(answer));
        let want = [
            "auction_3mib: HTTP/1.1 200 OK | route=auction body=3145728",
            "auction_small: HTTP/1.1 200 OK | route=auction body=100",
        ];
        ensure_eq(&rows(&pairs, go, what), &want, "net/http")?;
        ensure_eq(&rows(&pairs, rust, what), &want, "pbs-server")?;
        ensure_eq(
            &run_axum_default(&["auction_3mib", "auction_small"])?,
            &[
                "auction_3mib: HTTP/1.1 413 Payload Too Large | Failed to buffer the request body: \
                 length limit exceeded",
                "auction_small: HTTP/1.1 200 OK | route=auction body=100",
            ],
            "axum::serve",
        )
    }

    /// The middleware's headers: `NoCache`'s three on every answer the router gives, 404, 405
    /// and the CORS answers included, as on Go's (only the answers net/http writes before any
    /// handler lack them: the 400 to an invalid escape and `OPTIONS *`, which axum routes); and
    /// the CORS headers of the middleware cases as `server.cors-preflight` and
    /// `server.cors-actual-requests` record them for tower-http's `CorsLayer` with Prebid's
    /// options.
    // Go: NoCache (router/router.go:105-114) around SupportCORS (router/router.go:377-385), as
    // main.go:82-83 nests them
    #[test]
    fn no_cache_and_cors() -> TestResult {
        let pairs = run_groups(&[
            "not_found",
            "redirects",
            "method_not_allowed",
            "options",
            "head",
            "decoded_path",
            "body_limit",
            "middleware",
        ])?;
        let no_cache = |answer: &Answer| {
            format!(
                "{} | {} | {}",
                answer.field("cache-control"),
                answer.field("pragma"),
                answer.field("expires")
            )
        };
        let headers = "no-cache, no-store, must-revalidate | no-cache | 0";
        for pair in &pairs {
            let go_want =
                if ["invalid_escape", "options_asterisk"].contains(&pair.case.name.as_str()) {
                    "- | - | -"
                } else {
                    headers
                };
            ensure_eq(no_cache(&pair.case.go).as_str(), go_want, &pair.case.name)?;
            ensure_eq(no_cache(&pair.rust).as_str(), headers, &pair.case.name)?;
        }
        let middleware: Vec<Pair> = pairs
            .into_iter()
            .filter(|pair| pair.case.group == "middleware")
            .collect();
        let cors = |answer: &Answer| {
            [
                "access-control-allow-origin",
                "access-control-allow-credentials",
                "access-control-allow-methods",
                "access-control-allow-headers",
                "vary",
            ]
            .map(|name| answer.field(name))
            .join(" | ")
        };
        ensure_eq(
            &rows(&middleware, go, cors),
            &[
                "get_with_origin: https://publisher.example | true | - | - | Origin",
                "post_without_origin: - | - | - | - | Origin",
                "preflight: https://publisher.example | true | POST | content-type | Origin, \
                 Access-Control-Request-Method, Access-Control-Request-Headers",
                "not_found_with_origin: https://publisher.example | true | - | - | Origin",
            ],
            "rs/cors",
        )?;
        let vary = "origin, access-control-request-method, access-control-request-headers";
        ensure_eq(
            &rows(&middleware, rust, cors),
            &[
                format!("get_with_origin: https://publisher.example | true | - | - | {vary}"),
                format!("post_without_origin: - | true | - | - | {vary}"),
                format!(
                    "preflight: https://publisher.example | true | GET,POST,HEAD | \
                     origin,x-requested-with,content-type,accept | {vary}"
                ),
                format!("not_found_with_origin: https://publisher.example | true | - | - | {vary}"),
            ],
            "tower-http",
        )
    }
}
