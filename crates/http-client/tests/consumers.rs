//! The crate's API against the Go call sites its consumers port: bidder.go's request with its
//! client trace, its timeout notification and its seat non-bid reasons, the fetcher's and the
//! cache client's errors and bodies, the floors fetch's `client.Do` texts and drain, scope3's
//! `Client.Timeout`, the recaptcha check's own Transport, and the parity transport that wraps
//! router.go's clients. Each pin compiles the shapes the port needs (trait objects, futures that
//! cross tasks, errors that copy, responses and errors a transport makes) and checks what Go's
//! call site sees; each names the Go it models.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt, future, io, mem,
        sync::{Arc, Mutex, PoisonError},
        time::Duration,
    };

    use bytes::Bytes;
    use futures_util::future::BoxFuture;
    use pbs_http_client::{
        Builder, Client, DnsError, ErrorKind, Event, HttpClientSettings, ProxyEnv, RequestError,
        Response, Trace, Transport,
        reqwest::{
            Body, Method, Request, StatusCode, Url, Version,
            header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
        },
    };
    use serde_json::Value;
    use tokio::{
        net::TcpListener,
        sync::mpsc,
        time::{Instant, sleep, sleep_until},
    };
    use url::form_urlencoded::Serializer;

    use crate::support::{
        BoxError, RawServer, TestPki, TestResult, check, check_eq, loopback_client, request, run,
        with_ports,
    };

    tokio::task_local! {
        /// parity/trace's `WithBidder`: the bidder the exchange names in a request's context,
        /// which the parity transport labels the call with (parity/trace/context.go:10-21).
        static BIDDER: String;
    }

    /// The metrics engine of these pins: the events the client traces recorded.
    #[derive(Debug, Default)]
    struct Recorded(Mutex<Vec<Event>>);

    impl Recorded {
        fn push(&self, event: Event) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event);
        }

        fn events(&self) -> Vec<Event> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// bidder.go's `addClientTrace`: hooks that record into the metrics engine they hold
    /// (exchange/bidder.go:732-798).
    fn client_trace(metrics: &Arc<Recorded>) -> Trace {
        let metrics = Arc::clone(metrics);
        Trace::new(move |event| metrics.push(event))
    }

    /// A loopback server answering every request with `status`, a `max-age` header and `body`.
    async fn answering(status: u16, body: &'static str) -> Result<RawServer, BoxError> {
        RawServer::start(move |mut conn| async move {
            let answer = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nMax-Age: 600\r\n\r\n{body}",
                body.len()
            );
            while conn.read_request().await.is_some() {
                conn.write(answer.as_bytes()).await;
            }
        })
        .await
    }

    /// A loopback server that reads and never answers.
    async fn silent() -> Result<RawServer, BoxError> {
        RawServer::start(|mut conn| async move { conn.drain().await }).await
    }

    /// A port nothing listens on.
    async fn closed_port() -> Result<u16, BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Ok(listener.local_addr()?.port())
    }

    /// The deadline `ms` milliseconds from now.
    fn after(ms: u64) -> Option<Instant> {
        Instant::now().checked_add(Duration::from_millis(ms))
    }

    /// Go's `roundTripperFunc` (fetcher/source/http_test.go:166-170) and `mockRoundTripper`
    /// (`exchange/bidder_test.go:3231-3247`): a transport answering each request with what its
    /// function gives.
    struct Scripted<F>(F);

    impl<F> fmt::Debug for Scripted<F> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Scripted")
        }
    }

    impl<F> Transport for Scripted<F>
    where
        F: Fn(&Request, Option<Trace>) -> Result<Response, RequestError> + Send + Sync,
    {
        fn round_trip(
            &self,
            request: Request,
            _deadline: Option<Instant>,
            trace: Option<Trace>,
        ) -> BoxFuture<'_, Result<Response, RequestError>> {
            Box::pin(future::ready((self.0)(&request, trace)))
        }
    }

    /// A client whose transport answers with `answer`: Go's `&http.Client{Transport: rt}`.
    fn scripted<F>(answer: F) -> Client
    where
        F: Fn(&Request, Option<Trace>) -> Result<Response, RequestError> + Send + Sync + 'static,
    {
        Client::from_transport(Arc::new(Scripted(answer)))
    }

    /// A response to `request` with `status` and `body`, as a Go test's `RoundTripper` makes one
    /// (`&http.Response{StatusCode: ..., Body: io.NopCloser(...)}`).
    fn made(request: &Request, status: StatusCode, body: &'static [u8]) -> Response {
        Response::new(
            request.url().clone(),
            status,
            HeaderMap::new(),
            Bytes::from_static(body),
        )
    }

    /// Go's `*net.OpError` around a system call's error, as a transport that does not dial makes
    /// one (`parity/transport/inline.go:217-220`, `exchange/bidder_test.go:3153`): the
    /// operation's words, then the error, which stays its cause.
    #[derive(Debug)]
    struct OpError {
        op: &'static str,
        cause: io::Error,
    }

    impl OpError {
        /// `dial tcp: connect: connection refused`.
        fn refused() -> Self {
            Self {
                op: "dial tcp: connect",
                cause: io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused"),
            }
        }

        /// `read tcp: read: connection reset by peer`.
        fn reset() -> Self {
            Self {
                op: "read tcp: read",
                cause: io::Error::new(io::ErrorKind::ConnectionReset, "connection reset by peer"),
            }
        }
    }

    impl fmt::Display for OpError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}: {}", self.op, self.cause)
        }
    }

    impl Error for OpError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.cause)
        }
    }

    /// bidder.go's `httpCallInfo` (exchange/bidder.go:581-670): what the adapter reads, or the
    /// error of the send or of the body.
    #[derive(Debug)]
    enum Call {
        /// `ResponseData`: the status, the headers handed on and the body.
        Answered(StatusCode, HeaderMap, Bytes),
        /// The send failed; `timeout` when bidder.go makes the error an `errortypes.Timeout`.
        Unsent { timeout: bool, error: RequestError },
        /// Reading the body failed, which bidder.go returns as it is.
        Unread(RequestError),
    }

    impl Call {
        /// The status, or the text of the error bidder.go reports.
        fn text(&self) -> String {
            match self {
                Self::Answered(status, ..) => status.to_string(),
                Self::Unsent { error, .. } | Self::Unread(error) => error.to_string(),
            }
        }
    }

    /// bidder.go's `doRequestImpl` from the send on (exchange/bidder.go:615-670): `ctxhttp.Do`
    /// under `deadline` with the client trace, a send error that is `context.DeadlineExceeded`
    /// a timeout, then `io.ReadAll`, and the status and the headers handed to the adapter.
    async fn do_request(
        client: &Client,
        request: Request,
        deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> Call {
        let mut response = match client.send(request, deadline, trace).await {
            Ok(response) => response,
            Err(error) => {
                return Call::Unsent {
                    timeout: error.is_timeout(),
                    error,
                };
            }
        };
        let status = response.status();
        let headers = mem::take(response.headers_mut());
        match response.bytes().await {
            Ok(body) => Call::Answered(status, headers, body),
            Err(error) => Call::Unread(error),
        }
    }

    /// `httpInfoToNonBidReason` (exchange/non_bid_reason.go:41-57): an `errortypes.Timeout` is
    /// 101, a refused connection or a missing host 103, another error 100.
    fn non_bid_reason(call: &Call) -> Option<u16> {
        let error = match call {
            Call::Answered(..) => return None,
            Call::Unsent { timeout: true, .. } => return Some(101),
            Call::Unsent { error, .. } | Call::Unread(error) => error,
        };
        Some(
            if error.is_connection_refused() || error.is_dns_not_found() {
                103
            } else {
                100
            },
        )
    }

    /// Compiles only for a type whose values cross tasks and threads.
    const fn shared<T: Send + Sync + 'static>() {}

    /// Compiles only for a type whose values move to another task.
    const fn sendable<T: Send + 'static>() {}

    /// Compiles only for a value that moves to another task.
    const fn movable<T: Send>(_: &T) {}

    /// Compiles only for an error that crosses tasks and threads and copies, as Go's errors do.
    const fn error_value<E: Error + Clone + Send + Sync + 'static>() {}

    // Go: exchange/bidder.go:581-670 (doRequestImpl: the adapter's headers, ctxhttp.Do under the
    // bid's deadline with the client trace, io.ReadAll, the status and httpResp.Header handed to
    // the adapter) and :627-633 and :672-689 (doTimeoutNotification, in a goroutine of its own on
    // the same client under 200 ms, with the bid request's headers and its body drained).
    #[test]
    fn bidder_request_and_timeout_notification() -> TestResult {
        run(async {
            let server = answering(200, r#"{"seatbid":[]}"#).await?;
            let client = loopback_client(HttpClientSettings::GENERAL, server.port())?;
            let metrics = Arc::new(Recorded::default());
            // RequestData keeps its headers for ext.debug.httpcalls and the notification sends
            // them again: each request takes a copy, which Go's Request.write leaves as it is.
            let mut headers = HeaderMap::new();
            drop(headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json")));
            let mut bid = request("POST", "http://bidder.test/openrtb2")?;
            *bid.headers_mut() = headers.clone();
            *bid.body_mut() = Some(Body::from(r#"{"id":"r"}"#));
            let sending = do_request(&client, bid, after(5000), Some(client_trace(&metrics)));
            movable(&sending);
            let (status, handed, body) = match sending.await {
                Call::Answered(status, handed, body) => (status, handed, body),
                unanswered @ (Call::Unsent { .. } | Call::Unread(_)) => {
                    return Err(format!("the bid request: {unanswered:?}").into());
                }
            };
            check_eq(
                &(status, handed.get("max-age"), body.as_ref()),
                &(
                    StatusCode::OK,
                    Some(&HeaderValue::from_static("600")),
                    br#"{"seatbid":[]}"#.as_slice(),
                ),
                "the adapter's ResponseData",
            )?;
            check(
                metrics
                    .events()
                    .iter()
                    .any(|event| matches!(event, Event::GotConn { reused: false, .. })),
                "GotConn on a new connection, recorded",
            )?;
            let notification = tokio::spawn({
                let client = client.clone();
                async move {
                    let mut notice = request("GET", "http://bidder.test/notify")?;
                    *notice.headers_mut() = headers;
                    let (notified, (_, drained)) = {
                        let response = client.send(notice, after(200), None).await?;
                        (response.status(), response.read_all().await)
                    };
                    Ok::<_, BoxError>((
                        notified.is_success(),
                        drained.as_ref().map(ToString::to_string),
                    ))
                }
            });
            check_eq(
                &notification.await??,
                &(true, None),
                "the notification's success",
            )?;
            drop(client);
            let _counts = server.close().await;
            Ok(())
        })
    }

    // Go: exchange/non_bid_reason_test.go:13-65 and exchange/bidder_test.go:3107-3229
    // (TestSeatNonBid: a RoundTripper failing with context.DeadlineExceeded, a refused dial, a
    // missing host or another error, read into seat non-bid codes 101, 103 and 100), with a reset
    // read (parity/transport/inline.go:219-220) and a body that fails once the deadline has
    // passed, which bidder.go returns unclassified (exchange/bidder.go:642-648).
    #[test]
    fn seat_non_bid_reasons() -> TestResult {
        run(async {
            let missing = DnsError::new("bidder.test".to_owned(), "no such host".to_owned(), true);
            let cases = [
                (
                    "answered",
                    scripted(|request, _| Ok(made(request, StatusCode::NO_CONTENT, b""))),
                    None,
                    "204 No Content",
                ),
                (
                    "timeout",
                    scripted(|_, _| Err(RequestError::deadline())),
                    Some(101),
                    "context deadline exceeded",
                ),
                (
                    "refused",
                    scripted(|_, _| Err(RequestError::new(ErrorKind::Connect, OpError::refused()))),
                    Some(103),
                    r#"Get "http://bidder.test/": dial tcp: connect: connection refused"#,
                ),
                (
                    "no such host",
                    scripted(move |_, _| {
                        Err(RequestError::new(ErrorKind::Connect, missing.clone()))
                    }),
                    Some(103),
                    r#"Get "http://bidder.test/": no such host"#,
                ),
                (
                    "reset",
                    scripted(|_, _| Err(RequestError::new(ErrorKind::Connect, OpError::reset()))),
                    Some(100),
                    r#"Get "http://bidder.test/": read tcp: read: connection reset by peer"#,
                ),
                (
                    "other",
                    scripted(|_, _| Err(RequestError::new(ErrorKind::Request, "some_error"))),
                    Some(100),
                    r#"Get "http://bidder.test/": some_error"#,
                ),
                (
                    "body past the deadline",
                    scripted(|request, _| {
                        Ok(made(request, StatusCode::OK, b"{")
                            .with_body_error(RequestError::deadline()))
                    }),
                    Some(100),
                    "context deadline exceeded",
                ),
            ];
            for (name, client, reason, text) in cases {
                let get = request("GET", "http://bidder.test/")?;
                let call = do_request(&client, get, after(1000), None).await;
                check_eq(
                    &(non_bid_reason(&call), call.text()),
                    &(reason, text.to_owned()),
                    name,
                )?;
            }
            Ok(())
        })
    }

    /// `Fetch` (fetcher/source/http.go:54-82) of `key` over `client` up to the decoder, with the
    /// request of `testRequestBuilder` (fetcher/source/http_test.go:155-160): the body, nothing
    /// for a 404, or the error.
    async fn fetch(client: &Client, key: &str) -> Result<Option<Bytes>, String> {
        let mut endpoint = Url::parse("http://example.com").map_err(|e| e.to_string())?;
        endpoint.set_query(Some(
            &Serializer::new(String::new())
                .append_pair("id", key)
                .finish(),
        ));
        let get = Request::new(Method::GET, endpoint);
        let (status, read) = {
            let response = client
                .send(get, None, None)
                .await
                .map_err(|e| format!("fetch key {key} via HTTP: {e}"))?;
            (response.status(), response.bytes().await)
        };
        let body = read.map_err(|e| format!("read HTTP response for key {key}: {e}"))?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::OK => Ok(Some(body)),
            other => Err(format!(
                "fetch key {key} via HTTP: unexpected response status {}",
                other.as_u16()
            )),
        }
    }

    // Go: fetcher/source/http_test.go:65-136 (TestHTTPSourceReturnsContextualErrors: the
    // transport's error inside client.Do's url.Error, an errorReader body, a 500) and
    // fetcher/source/http.go:70-72 and http_test.go:34-47 (a 404 is no data).
    #[test]
    fn fetcher_contextual_errors() -> TestResult {
        run(async {
            let failing = scripted(|_, _| Err(RequestError::new(ErrorKind::Request, "expected")));
            let unreadable = scripted(|request, _| {
                Ok(made(request, StatusCode::OK, b"")
                    .with_body_error(RequestError::new(ErrorKind::Body, "expected")))
            });
            let failed =
                scripted(|request, _| Ok(made(request, StatusCode::INTERNAL_SERVER_ERROR, b"")));
            let missing = scripted(|request, _| Ok(made(request, StatusCode::NOT_FOUND, b"")));
            let answered = scripted(|request, _| Ok(made(request, StatusCode::OK, b"{}")));
            check_eq(
                &[
                    fetch(&failing, "one").await,
                    fetch(&unreadable, "one").await,
                    fetch(&failed, "one").await,
                    fetch(&missing, "one").await,
                    fetch(&answered, "one").await,
                ],
                &[
                    // Go: `fetch key one via HTTP: Get "http://example.com?id=one": expected`;
                    // the url crate writes the empty path as `/` (http-client.error-texts).
                    Err(
                        r#"fetch key one via HTTP: Get "http://example.com/?id=one": expected"#
                            .to_owned(),
                    ),
                    Err("read HTTP response for key one: expected".to_owned()),
                    Err("fetch key one via HTTP: unexpected response status 500".to_owned()),
                    Ok(None),
                    Ok(Some(Bytes::from_static(b"{}"))),
                ],
                "the fetcher's results",
            )
        })
    }

    // Go: prebid_cache_client/client.go:101-125 (the JSON post under the auction's context, and
    // io.ReadAll's partial body in the logged error when the read failed).
    #[test]
    fn cache_client_reports_the_part_it_read() -> TestResult {
        run(async {
            let server = RawServer::start(|mut conn| async move {
                if conn.read_request().await.is_some() {
                    conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"responses\":[")
                        .await;
                }
            })
            .await?;
            let client = loopback_client(HttpClientSettings::CACHE, server.port())?;
            let put_url = "http://cache.test/cache";
            let mut put = request("POST", put_url)?;
            drop(put.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/json;charset=utf-8"),
            ));
            drop(
                put.headers_mut()
                    .insert(ACCEPT, HeaderValue::from_static("application/json")),
            );
            *put.body_mut() = Some(Body::from(r#"{"puts":[]}"#));
            let (status, (body, error)) = {
                let response = client.send(put, after(5000), None).await?;
                (response.status(), response.read_all().await)
            };
            check_eq(
                &(
                    error.as_ref().map(RequestError::kind),
                    format!(
                        "Prebid Cache call to {put_url} returned {}: {}",
                        status.as_u16(),
                        String::from_utf8_lossy(&body)
                    ),
                ),
                &(
                    Some(ErrorKind::Body),
                    r#"Prebid Cache call to http://cache.test/cache returned 200: {"responses":["#
                        .to_owned(),
                ),
                "the logged error, with the part read",
            )?;
            drop(client);
            let _counts = server.close().await;
            Ok(())
        })
    }

    // Go: floors/fetcher.go:261-273 (client.Do under the fetch's own timeout, its error in the
    // fetcher's words) and currency/rate_converter.go:49-61 (the same error, returned as it is).
    #[test]
    fn floors_fetch_error_reads_as_client_do() -> TestResult {
        run(async {
            let stalled = silent().await?;
            let port = stalled.port();
            let fetcher = loopback_client(HttpClientSettings::FLOORS, port)?;
            let url = format!("http://127.0.0.1:{port}/floors.json");
            let Err(error) = fetcher.send(request("GET", &url)?, after(100), None).await else {
                return Err("the silent server answered".into());
            };
            let fetch_error = format!(
                "error while getting response from url : {}",
                error.do_text()
            );
            check_eq(
                &(
                    error.to_string(),
                    with_ports(&fetch_error, &[(port, "PORT")]),
                ),
                &(
                    "context deadline exceeded".to_owned(),
                    concat!(
                        "error while getting response from url : ",
                        r#"Get "http://127.0.0.1:PORT/floors.json": context deadline exceeded"#
                    )
                    .to_owned(),
                ),
                "ctxhttp's text, and the floors fetcher's",
            )?;
            drop(fetcher);
            let _counts = stalled.close().await;
            Ok(())
        })
    }

    // Go: floors/fetcher.go:274-296 (max-age read from the headers, the body drained in a defer
    // so the connection goes back to the pool) and exchange/bidder.go:741-755 (the Reused flag
    // GotConn reports).
    #[test]
    fn floors_fetch_drains_for_reuse() -> TestResult {
        run(async {
            let server = answering(200, "{}").await?;
            let fetcher = loopback_client(HttpClientSettings::FLOORS, server.port())?;
            let metrics = Arc::new(Recorded::default());
            let mut reads = Vec::new();
            for _ in 0..2 {
                let get = request("GET", "http://floors.test/floors.json")?;
                let trace = Some(client_trace(&metrics));
                reads.push({
                    let response = fetcher.send(get, after(5000), trace).await?;
                    let max_age = response
                        .headers()
                        .get("max-age")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<i64>().ok());
                    (max_age, response.read_all().await.1.is_none())
                });
            }
            let reused: Vec<bool> = metrics
                .events()
                .iter()
                .filter_map(|event| {
                    if let Event::GotConn { reused, .. } = event {
                        Some(*reused)
                    } else {
                        None
                    }
                })
                .collect();
            check_eq(
                &(reads, reused),
                &(
                    vec![(Some(600), true), (Some(600), true)],
                    vec![false, true],
                ),
                "max-age and the drained body, and the second fetch on the first one's connection",
            )?;
            drop(fetcher);
            let _counts = server.close().await;
            Ok(())
        })
    }

    // Go: modules/scope3/rtd/module.go:75-80 (Timeout over the general client's Transport),
    // async_request.go:31-53 (the fetch in a goroutine of its own, under the incoming request's
    // context, which has no deadline) and module.go:430-433 (client.Do's error, which the hook
    // writes into its analytics tags at :301-310).
    #[test]
    fn scope3_client_timeout() -> TestResult {
        run(async {
            let stalled = silent().await?;
            let port = stalled.port();
            let general = loopback_client(HttpClientSettings::GENERAL, port)?;
            let scope3 = general.with_timeout(Duration::from_millis(100));
            let endpoint = format!("http://127.0.0.1:{port}/prebid/prebid");
            let fetch = tokio::spawn({
                let endpoint = endpoint.clone();
                async move {
                    let mut segments = request("POST", &endpoint)?;
                    *segments.body_mut() = Some(Body::from("{}"));
                    let sent = scope3.send(segments, None, None).await;
                    sent.err()
                        .ok_or_else(|| BoxError::from("the silent server answered"))
                }
            });
            let failed = fetch.await??;
            let ports = [(port, "PORT")];
            check_eq(
                &(
                    failed.kind(),
                    failed.is_timeout(),
                    with_ports(&failed.to_string(), &ports),
                ),
                &(
                    ErrorKind::ClientTimeout,
                    true,
                    concat!(
                        r#"Post "http://127.0.0.1:PORT/prebid/prebid": context deadline "#,
                        "exceeded (Client.Timeout exceeded while awaiting headers)"
                    )
                    .to_owned(),
                ),
                "the analytics tag's error",
            )?;
            let Err(bare) = general
                .send(request("GET", &endpoint)?, after(50), None)
                .await
            else {
                return Err("the silent server answered".into());
            };
            check_eq(
                &bare.to_string(),
                &"context deadline exceeded".to_owned(),
                "the general client has no timeout",
            )?;
            drop(general);
            let _counts = stalled.close().await;
            Ok(())
        })
    }

    // Go: pbs/usersync.go:36-66 (VerifyRecaptcha: a Transport of its own, Go's zero Transport with
    // the proxy environment and the CertPool's roots, so HTTP/1.1 only; PostForm without a
    // deadline, whose url.Values encode sorted by key; then the JSON verdict).
    #[test]
    fn recaptcha_check_on_a_zero_transport() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let (sender, mut bodies) = mpsc::unbounded_channel();
            let acceptor = pki.acceptor(&[b"h2", b"http/1.1"])?;
            let server = RawServer::start_tls(acceptor, move |mut conn| {
                let sender = sender.clone();
                async move {
                    if let Some(request) = conn.read_request().await {
                        let _sent = sender.send(request.body);
                        conn.write(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n{\"success\":true}",
                        )
                        .await;
                    }
                }
            })
            .await?;
            let client = Builder::new(HttpClientSettings::ZERO, ProxyEnv::default())
                .roots(pki.roots())
                .loopback_only(server.port())
                .build()?;
            let form = Serializer::new(String::new())
                .append_pair("response", "r")
                .append_pair("secret", "s")
                .finish();
            let mut post = request("POST", "https://localhost/recaptcha/api/siteverify")?;
            drop(post.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            ));
            *post.body_mut() = Some(Body::from(form));
            let (version, answer) = {
                let response = client.send(post, None, None).await?;
                (response.version(), response.bytes().await?)
            };
            let verdict: Value = serde_json::from_slice(&answer)?;
            check_eq(
                &(
                    version,
                    verdict.get("success").and_then(Value::as_bool),
                    bodies.recv().await,
                ),
                &(
                    Version::HTTP_11,
                    Some(true),
                    Some(b"response=r&secret=s".to_vec()),
                ),
                "HTTP/1.1, the verdict, and the form the server read",
            )?;
            drop(client);
            let _counts = server.close().await;
            Ok(())
        })
    }

    /// The parity transport (parity/transport/transport.go:80-88): the oracle, which every
    /// request reaches as its proxy, or the envelope's inline answers; and the calls it saw.
    #[derive(Debug)]
    struct Parity {
        mode: Mode,
        calls: Mutex<Vec<Seen>>,
    }

    /// How the parity transport answers.
    #[derive(Debug)]
    enum Mode {
        /// Through a clone of the production transport with the oracle as its proxy.
        Oracle(Arc<dyn Transport>),
        /// With an inline mock.
        Inline(Inline),
    }

    /// An inline mock (parity/transport/inline.go:137-224).
    #[derive(Debug, Clone, Copy)]
    enum Inline {
        Status(StatusCode),
        Delayed(Duration),
        Refused,
        Timeout,
        Disabled,
    }

    /// What the parity transport records of a call (parity/transport/inline.go:35-65): the
    /// bidder its context names, the URL and the body.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Seen {
        bidder: Option<String>,
        url: String,
        body: Option<Vec<u8>>,
    }

    impl Parity {
        fn new(mode: Mode) -> Self {
            Self {
                mode,
                calls: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<Seen> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Transport for Parity {
        fn round_trip(
            &self,
            request: Request,
            deadline: Option<Instant>,
            trace: Option<Trace>,
        ) -> BoxFuture<'_, Result<Response, RequestError>> {
            Box::pin(async move {
                let seen = Seen {
                    bidder: BIDDER.try_with(String::clone).ok(),
                    url: request.url().to_string(),
                    body: request.body().and_then(Body::as_bytes).map(<[u8]>::to_vec),
                };
                self.calls
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(seen);
                match &self.mode {
                    Mode::Oracle(oracle) => {
                        via_oracle(oracle.as_ref(), request, deadline, trace).await
                    }
                    Mode::Inline(answer) => inline(*answer, request.url().clone(), deadline).await,
                }
            })
        }
    }

    /// `viaOracle` (parity/transport/transport.go:119-143): the request rewritten from https to
    /// http and labelled with `X-Parity-Meta`, then sent through the oracle transport.
    async fn via_oracle(
        oracle: &dyn Transport,
        mut request: Request,
        deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> Result<Response, RequestError> {
        if request.url().scheme() == "https" {
            request
                .url_mut()
                .set_scheme("http")
                .map_err(|()| RequestError::new(ErrorKind::Request, "parity: scheme kept"))?;
        }
        drop(request.headers_mut().insert(
            HeaderName::from_static("x-parity-meta"),
            HeaderValue::from_static("eyJ2IjoxfQ"),
        ));
        oracle.round_trip(request, deadline, trace).await
    }

    /// `respond` and `failure` (parity/transport/inline.go:137-224): the mock's answer, its
    /// delay, and its failure against the request's deadline.
    async fn inline(
        answer: Inline,
        url: Url,
        deadline: Option<Instant>,
    ) -> Result<Response, RequestError> {
        match answer {
            Inline::Status(status) => {
                Ok(Response::new(url, status, HeaderMap::new(), Bytes::new()))
            }
            Inline::Delayed(delay) => {
                sleep(delay).await;
                Ok(Response::new(
                    url,
                    StatusCode::OK,
                    HeaderMap::new(),
                    Bytes::new(),
                ))
            }
            Inline::Refused => Err(RequestError::new(ErrorKind::Connect, OpError::refused())),
            Inline::Timeout => match deadline {
                Some(deadline) => {
                    sleep_until(deadline).await;
                    Err(RequestError::deadline())
                }
                None => Err(RequestError::new(
                    ErrorKind::Request,
                    "parity: inline mock timeout",
                )),
            },
            Inline::Disabled => Err(RequestError::new(
                ErrorKind::Request,
                "parity: network disabled",
            )),
        }
    }

    // Go: parity/transport/transport.go:119-173 (viaOracle: the request rewritten from https to
    // http, X-Parity-Meta added, sent through a clone of the production Transport whose Proxy is
    // http.ProxyURL(oracle), loopback included, a failure reading as the url.Error of the request
    // the client was handed) and parity/trace/context.go:10-21 (the bidder in the request's
    // context).
    #[test]
    fn parity_oracle_wraps_a_production_transport() -> TestResult {
        run(async {
            let (sender, mut heads) = mpsc::unbounded_channel();
            let oracle = RawServer::start(move |mut conn| {
                let sender = sender.clone();
                async move {
                    if let Some(request) = conn.read_request().await {
                        let _sent = sender.send(request.head);
                        conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await;
                    }
                }
            })
            .await?;
            let production =
                Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default()).loopback_only(0);
            let proxy = |port: u16| Url::parse(&format!("http://127.0.0.1:{port}"));
            let parity = Arc::new(Parity::new(Mode::Oracle(
                production
                    .clone()
                    .proxy_url(proxy(oracle.port())?)
                    .build_transport()?,
            )));
            let transport: Arc<dyn Transport> = Arc::<Parity>::clone(&parity);
            let wrapped = Client::from_transport(transport);
            let mut bid = request("POST", "https://bidder.test/openrtb2?x=1")?;
            *bid.body_mut() = Some(Body::from(r#"{"id":"r"}"#));
            let answer = BIDDER
                .scope("appnexus".to_owned(), async {
                    let response = wrapped.send(bid, after(5000), None).await?;
                    response.bytes().await
                })
                .await?;
            let head = heads.recv().await.ok_or("the oracle read no request")?;
            check_eq(
                &(
                    answer,
                    head.first().cloned(),
                    head.contains(&"x-parity-meta: eyJ2IjoxfQ".to_owned()),
                    parity.calls(),
                ),
                &(
                    Bytes::from_static(b"ok"),
                    Some("POST http://bidder.test/openrtb2?x=1 HTTP/1.1".to_owned()),
                    true,
                    vec![Seen {
                        bidder: Some("appnexus".to_owned()),
                        url: "https://bidder.test/openrtb2?x=1".to_owned(),
                        body: Some(br#"{"id":"r"}"#.to_vec()),
                    }],
                ),
                "the oracle's answer, what it read as the proxy, and the call the transport saw",
            )?;
            let down = Client::from_transport(Arc::new(Parity::new(Mode::Oracle(
                production
                    .proxy_url(proxy(closed_port().await?)?)
                    .build_transport()?,
            ))));
            let Err(refused) = down
                .send(request("GET", "https://bidder.test/p")?, None, None)
                .await
            else {
                return Err("the closed oracle answered".into());
            };
            check(
                refused.is_connection_refused()
                    && refused
                        .to_string()
                        .starts_with(r#"Get "https://bidder.test/p": "#),
                "a refused oracle, in the url.Error of the request the client was handed",
            )?;
            drop((wrapped, down));
            let _counts = oracle.close().await;
            Ok(())
        })
    }

    // Go: parity/transport/inline.go:137-224 (inline mode: a status, a refused dial, a timeout
    // that waits for the deadline, a delay that the deadline cuts, and no mock, "parity: network
    // disabled").
    #[test]
    fn parity_inline_answers() -> TestResult {
        run(async {
            let url = "https://bidder.test/openrtb2";
            let mut seen = Vec::new();
            for answer in [
                Inline::Status(StatusCode::NO_CONTENT),
                Inline::Refused,
                Inline::Timeout,
                Inline::Delayed(Duration::from_secs(5)),
                Inline::Disabled,
            ] {
                let client = Client::from_transport(Arc::new(Parity::new(Mode::Inline(answer))));
                let sent = client.send(request("POST", url)?, after(50), None).await;
                seen.push((
                    sent.as_ref().map_or_else(ToString::to_string, |response| {
                        response.status().to_string()
                    }),
                    sent.as_ref().err().is_some_and(RequestError::is_timeout),
                    sent.as_ref()
                        .err()
                        .is_some_and(RequestError::is_connection_refused),
                ));
            }
            check_eq(
                &seen,
                &vec![
                    ("204 No Content".to_owned(), false, false),
                    (
                        concat!(
                            r#"Post "https://bidder.test/openrtb2": dial tcp: connect: "#,
                            "connection refused"
                        )
                        .to_owned(),
                        false,
                        true,
                    ),
                    ("context deadline exceeded".to_owned(), true, false),
                    ("context deadline exceeded".to_owned(), true, false),
                    (
                        r#"Post "https://bidder.test/openrtb2": parity: network disabled"#
                            .to_owned(),
                        false,
                        false,
                    ),
                ],
                "what the client returns for each mock",
            )
        })
    }

    // Go: exchange/bidder_test.go:2113-2127 and :2145-2167 (DNSDoneTripper: a RoundTripper that
    // calls the trace's DNSDone, which the bidder records as DNS time), and net/http's
    // RoundTripper, which any type with RoundTrip satisfies: the trait object, futures that cross
    // tasks and errors that copy (plan principle 15).
    #[test]
    fn transports_are_trait_objects() -> TestResult {
        shared::<Client>();
        shared::<Trace>();
        shared::<Arc<dyn Transport>>();
        shared::<RequestError>();
        shared::<DnsError>();
        sendable::<Response>();
        error_value::<RequestError>();
        error_value::<DnsError>();
        let metrics = Arc::new(Recorded::default());
        let transport: Arc<dyn Transport> =
            Arc::new(Scripted(|request: &Request, trace: Option<Trace>| {
                if let Some(trace) = trace {
                    trace.emit(Event::Dns {
                        elapsed: Duration::ZERO,
                    });
                }
                Ok(made(request, StatusCode::OK, b"postBody"))
            }));
        let client = Client::from_transport(transport);
        let trace = client_trace(&metrics);
        let body = run(async move {
            let reading = client
                .send(
                    request("POST", "http://www.example.com/")?,
                    None,
                    Some(trace),
                )
                .await?
                .bytes();
            movable(&reading);
            Ok(reading.await?)
        })?;
        check_eq(
            &(body, metrics.events()),
            &(
                Bytes::from_static(b"postBody"),
                vec![Event::Dns {
                    elapsed: Duration::ZERO,
                }],
            ),
            "the body, and DNSDone recorded",
        )
    }
}
