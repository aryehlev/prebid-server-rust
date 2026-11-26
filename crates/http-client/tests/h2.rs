//! The pins of the bidder client's HTTP/2 (`http-client.http2`, `http-client.h2c-probe`,
//! `http-client.h2c-demotion`, `http-client.h2-connection-counts` and
//! `http-client.h2c-redirects`): loopback bidders that speak h2 over TLS, h2c, or HTTP/1.1 only,
//! answering HTTP/2's preface as Go's servers do, next to what Go's clients did with the same
//! servers (`testdata/golden/h2.json`, `testdata/go/h2.go`). The cases that move tokio's clock on
//! (a stalling server, the verdicts' lifetime) run with `-- --ignored h2c`.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        fmt::Write as _,
        future::{Future, poll_fn},
        sync::{Arc, Mutex, MutexGuard, PoisonError},
        time::Duration,
    };

    use bytes::Bytes;
    use h2::{
        Reason, RecvStream,
        server::{SendResponse, handshake},
    };
    use pbs_http_client::{
        Builder, Client, Event, General, H2C_PROBE_TIMEOUT, H2C_VERDICT_TTL, Http2Settings,
        HttpClientSettings, ProxyEnv, Trace,
        reqwest::{
            Request, StatusCode, Url,
            header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, LOCATION, USER_AGENT},
        },
    };
    use serde_json::Value;
    use tokio::{
        runtime,
        sync::watch,
        task::JoinSet,
        time::{self, Instant, sleep, timeout},
    };

    use crate::support::{
        BoxError, Counts, RawConn, RawServer, Stream, TestPki, TestResult, check_eq, fetch, golden,
        int, list, ports_in, request, run, strings, text, unhex,
    };

    /// HTTP/2's client connection preface.
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

    /// An empty SETTINGS frame: what a probe sends after the preface.
    const EMPTY_SETTINGS: [u8; 9] = [0, 0, 0, 4, 0, 0, 0, 0, 0];

    /// A bid request's body, as `h2.go` posts it.
    const BODY: &str = r#"{"id":"r"}"#;

    /// The Content-Type adapters set, as `h2.go` sets it.
    const JSON: &str = "application/json;charset=utf-8";

    /// The Date the answers of Go's servers carry in place of `h2.json`'s `DATE`.
    const DATE: &str = "Fri, 20 Nov 2026 00:00:00 GMT";

    /// How long a bidder takes to answer `/wait`.
    const WAIT: Duration = Duration::from_millis(100);

    /// The size of the header `/big` answers with: past hyper's 16 KiB, within Go's 10 MiB.
    const BIG: usize = 64 * 1024;

    /// A second.
    const SECOND: Duration = Duration::from_secs(1);

    /// What a cleartext bidder does with a connection that opens with HTTP/2's preface: a
    /// probe's (an empty SETTINGS frame next) or a client's own.
    #[derive(Debug, Clone)]
    enum Preface {
        /// Speaks HTTP/2: Go's h2c server's SETTINGS frame to a probe (`h2.json`), h2 to a client.
        Serve,
        /// As `Serve`, but drops a client's connection once it has read a request.
        Crash,
        /// Answers as one of Go's HTTP/1.1 servers does (`h2.json`), then closes; `handled`: its
        /// handler ran for `PRI * HTTP/2.0`. A connection with prior knowledge is answered once
        /// its request has come ([`serve_cleartext`]).
        Answer { answer: Vec<u8>, handled: bool },
        /// Closes the connection unanswered: a probe's at once, one with prior knowledge once its
        /// request has come.
        Close,
        /// Reads, and never answers.
        Stall,
    }

    /// What a bidder saw.
    #[derive(Debug, Default, Clone)]
    struct Seen {
        /// The requests its handlers read, as `h2.go` writes them.
        requests: Vec<String>,
        /// The HTTP/1.1 requests' heads, line by line as they came.
        heads: Vec<Vec<String>>,
        /// The h2 requests' pseudo-header fields, fields (sorted) and body, as `h2.json` writes
        /// them.
        h2: Vec<(Vec<String>, Vec<String>, String)>,
        /// The probes that began: connections whose preface an empty SETTINGS frame followed.
        probing: usize,
        /// The probes that ended, each as the bytes it sent, in hex.
        probes: Vec<String>,
        /// The other connections that opened with the preface: HTTP/2 with prior knowledge.
        prior: usize,
        /// The streams the client reset, as their path and the reason.
        resets: Vec<String>,
    }

    /// What a bidder's connections share.
    #[derive(Debug)]
    struct Shared {
        preface: Mutex<Preface>,
        seen: watch::Sender<Seen>,
        /// Go's h2c server's answer to a probe: its SETTINGS frame (`h2.json`).
        settings: Vec<u8>,
    }

    impl Shared {
        fn new(preface: Preface, doc: &Value) -> Result<Arc<Self>, BoxError> {
            let h2c = probe_server(doc, "h2c")?;
            let mut settings = unhex(text(h2c, "/first")?)?;
            settings.extend(unhex(text(h2c, "/settings")?)?);
            Ok(Arc::new(Self {
                preface: Mutex::new(preface),
                seen: watch::Sender::new(Seen::default()),
                settings,
            }))
        }

        fn record<F: FnOnce(&mut Seen)>(&self, change: F) {
            self.seen.send_modify(change);
        }

        fn mode(&self) -> Preface {
            lock(&self.preface).clone()
        }
    }

    /// A loopback bidder.
    #[derive(Debug)]
    struct Bidder {
        server: RawServer,
        shared: Arc<Shared>,
    }

    impl Bidder {
        /// A cleartext bidder: HTTP/1.1, and `preface` for the connections that open with
        /// HTTP/2's.
        async fn cleartext(preface: Preface, doc: &Value) -> Result<Self, BoxError> {
            let shared = Shared::new(preface, doc)?;
            let server = RawServer::start({
                let shared = Arc::clone(&shared);
                move |conn| serve_cleartext(conn, Arc::clone(&shared))
            })
            .await?;
            Ok(Self { server, shared })
        }

        /// A TLS bidder offering `alpn`: h2 when TLS picks it, HTTP/1.1 otherwise.
        async fn tls(pki: &TestPki, alpn: &[&[u8]], doc: &Value) -> Result<Self, BoxError> {
            let shared = Shared::new(Preface::Serve, doc)?;
            let server = RawServer::start_tls(pki.acceptor(alpn)?, {
                let shared = Arc::clone(&shared);
                move |conn| serve_tls(conn, Arc::clone(&shared))
            })
            .await?;
            Ok(Self { server, shared })
        }

        const fn port(&self) -> u16 {
            self.server.port()
        }

        fn accepted(&self) -> usize {
            self.server.counts().accepted
        }

        /// From now on, does `preface` with the connections that open with HTTP/2's.
        fn set(&self, preface: Preface) {
            *lock(&self.shared.preface) = preface;
        }

        fn seen(&self) -> Seen {
            self.shared.seen.borrow().clone()
        }

        /// What the bidder has seen once `done` holds, waiting up to ten seconds.
        async fn wait<F: FnMut(&Seen) -> bool>(&self, done: F) -> Result<Seen, BoxError> {
            let mut seen = self.shared.seen.subscribe();
            let found = timeout(Duration::from_secs(10), seen.wait_for(done))
                .await
                .map_err(|elapsed| format!("{elapsed}: the bidder saw only {:?}", self.seen()))??
                .clone();
            Ok(found)
        }

        async fn close(self) -> Counts {
            self.server.close().await
        }
    }

    /// Locks `mutex`, through a poisoning.
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `bytes` in hex, as `h2.json` writes them.
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut out, byte| {
            let _written = write!(out, "{byte:02x}");
            out
        })
    }

    /// The first of `values`.
    fn first(values: &[String]) -> Result<String, BoxError> {
        values
            .first()
            .cloned()
            .ok_or_else(|| "an empty list".into())
    }

    /// `h2.json`'s probe of `server`.
    fn probe_server<'d>(doc: &'d Value, server: &str) -> Result<&'d Value, BoxError> {
        list(doc, "/probe/servers")?
            .iter()
            .find(|entry| text(entry, "/server").ok() == Some(server))
            .ok_or_else(|| format!("h2.json has no probe of {server}").into())
    }

    /// What one of `h2.json`'s HTTP/1.1 servers does with HTTP/2's preface, as a cleartext
    /// bidder's [`Preface`].
    fn answer(doc: &Value, server: &str) -> Result<Preface, BoxError> {
        let probed = probe_server(doc, server)?;
        Ok(Preface::Answer {
            answer: text(probed, "/answer")?.replace("DATE", DATE).into_bytes(),
            handled: !list(probed, "/handled")?.is_empty(),
        })
    }

    /// Serves one connection of a cleartext bidder.
    ///
    /// With prior knowledge, an answer or a close waits until the request has come, as one that
    /// crosses a network comes after the request. One that came sooner could close the connection
    /// before hyper's client handed it the request, and the request's outcome would hang on
    /// timing (`http-client.h2c-demotion`): sent again over HTTP/1.1, failed unsent, or, rarely,
    /// queued until its deadline.
    async fn serve_cleartext(mut conn: RawConn, shared: Arc<Shared>) {
        if conn.peek(PREFACE.len()).await != PREFACE {
            return http1(conn, &shared).await;
        }
        let hello = conn.peek(PREFACE.len() + EMPTY_SETTINGS.len()).await;
        let probe = hello.get(PREFACE.len()..) == Some(EMPTY_SETTINGS.as_slice());
        shared.record(|seen| {
            if probe {
                seen.probing += 1;
            } else {
                seen.prior += 1;
            }
        });
        let ended = |sent: &[u8]| {
            let sent = hex(sent);
            shared.record(|seen| seen.probes.push(sent));
        };
        match (shared.mode(), probe) {
            (Preface::Serve | Preface::Crash, true) => {
                let mut sent = conn.read_bytes(hello.len()).await.unwrap_or_default();
                conn.write(&shared.settings).await;
                sent.extend(conn.read_rest().await);
                ended(&sent);
            }
            (Preface::Serve, false) => serve_h2(conn.into_stream(), &shared, false).await,
            (Preface::Crash, false) => serve_h2(conn.into_stream(), &shared, true).await,
            (Preface::Answer { answer, handled }, _) => {
                let mut sent = if probe {
                    conn.read_bytes(PREFACE.len()).await.unwrap_or_default()
                } else {
                    conn.read_through(BODY.as_bytes()).await
                };
                if handled {
                    let pri = "PRI * HTTP/2.0 ua= body=0".to_owned();
                    shared.record(|seen| seen.requests.push(pri));
                }
                conn.write(&answer).await;
                conn.shutdown().await;
                sent.extend(conn.read_rest().await);
                if probe {
                    ended(&sent);
                }
            }
            (Preface::Close, _) => {
                if probe {
                    ended(&hello);
                } else {
                    let _request = conn.read_through(BODY.as_bytes()).await;
                }
            }
            (Preface::Stall, _) => {
                let sent = conn.read_rest().await;
                if probe {
                    ended(&sent);
                }
            }
        }
    }

    /// Serves one connection of a TLS bidder.
    async fn serve_tls(conn: RawConn, shared: Arc<Shared>) {
        if conn.alpn() == Some(b"h2".as_slice()) {
            serve_h2(conn.into_stream(), &shared, false).await;
        } else {
            http1(conn, &shared).await;
        }
    }

    /// Serves HTTP/1.1: each request answered after its path.
    async fn http1(mut conn: RawConn, shared: &Shared) {
        while let Some(request) = conn.read_request().await {
            let request_line = request.head.first().cloned().unwrap_or_default();
            let mut words = request_line.split(' ');
            let (method, target, proto) = (
                words.next().unwrap_or_default(),
                words.next().unwrap_or_default(),
                words.next().unwrap_or_default(),
            );
            let field = |name: &str| {
                request
                    .head
                    .iter()
                    .skip(1)
                    .filter_map(|line| line.split_once(": "))
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map_or("", |(_, value)| value)
            };
            let handled = format!(
                "{method} {target} {proto} ua={} body={}",
                field("user-agent"),
                request.body.len()
            );
            shared.record(|seen| {
                seen.requests.push(handled);
                seen.heads.push(request.head.clone());
            });
            let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_owned();
            let answer = match target {
                "/slow" => {
                    conn.drain().await;
                    return;
                }
                "/wait" => {
                    sleep(WAIT).await;
                    ok
                }
                "/same" => redirect("/bid"),
                "/away" => {
                    let port = field("host").rsplit_once(':').map_or("", |(_, port)| port);
                    redirect(&format!("http://localhost:{port}/bid"))
                }
                _ => ok,
            };
            conn.write(answer.as_bytes()).await;
        }
    }

    /// An HTTP/1.1 307 to `location`.
    fn redirect(location: &str) -> String {
        format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
        )
    }

    /// An HTTP/2 request's head, as its handler reads it.
    #[derive(Debug)]
    struct Head {
        method: String,
        path: String,
        authority: String,
        scheme: String,
        port: u16,
        headers: HeaderMap,
    }

    /// Serves HTTP/2 on `stream`; with `crash`, only until it has read a request, and then drops
    /// the connection.
    async fn serve_h2<S: Stream>(stream: S, shared: &Arc<Shared>, crash: bool) {
        let Ok(mut connection) = handshake(stream).await else {
            return;
        };
        while let Some(Ok((request, respond))) = connection.accept().await {
            let (parts, body) = request.into_parts();
            let head = Head {
                method: parts.method.to_string(),
                path: parts
                    .uri
                    .path_and_query()
                    .map_or_else(String::new, ToString::to_string),
                authority: parts
                    .uri
                    .authority()
                    .map_or_else(String::new, ToString::to_string),
                scheme: parts.uri.scheme_str().unwrap_or_default().to_owned(),
                port: parts.uri.port_u16().unwrap_or_default(),
                headers: parts.headers,
            };
            let answered = tokio::spawn(answer_h2(head, body, respond, Arc::clone(shared), crash));
            if crash {
                // Read the request while the connection runs, then drop the connection, and the
                // stream after it, so that the client sees the connection go, not a reset.
                let kept = tokio::select! {
                    kept = answered => kept.ok().flatten(),
                    _closed = poll_fn(|cx| connection.poll_closed(cx)) => None,
                };
                drop(connection);
                drop(kept);
                return;
            }
        }
    }

    /// Reads an HTTP/2 request, records it and answers it after its path; with `crash`, gives its
    /// stream back unanswered.
    async fn answer_h2(
        head: Head,
        mut body: RecvStream,
        mut respond: SendResponse<Bytes>,
        shared: Arc<Shared>,
        crash: bool,
    ) -> Option<SendResponse<Bytes>> {
        let mut read = Vec::new();
        while let Some(Ok(chunk)) = body.data().await {
            let _released = body.flow_control().release_capacity(chunk.len());
            read.extend_from_slice(&chunk);
        }
        let agent = head
            .headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let handled = format!(
            "{} {} HTTP/2.0 ua={agent} body={}",
            head.method,
            head.path,
            read.len()
        );
        let pseudo = vec![
            format!(":authority: {}", head.authority),
            format!(":method: {}", head.method),
            format!(":path: {}", head.path),
            format!(":scheme: {}", head.scheme),
        ];
        let mut fields: Vec<String> = head
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {}", String::from_utf8_lossy(value.as_bytes())))
            .collect();
        fields.sort();
        let read = String::from_utf8_lossy(&read).into_owned();
        shared.record(|seen| {
            seen.requests.push(handled);
            seen.h2.push((pseudo, fields, read));
        });
        if crash {
            return Some(respond);
        }
        match head.path.as_str() {
            "/slow" => {
                if let Ok(reason) = poll_fn(|cx| respond.poll_reset(cx)).await {
                    let reset = format!("/slow {reason:?}");
                    shared.record(|seen| seen.resets.push(reset));
                }
            }
            "/wait" => {
                sleep(WAIT).await;
                send(&mut respond, StatusCode::OK, HeaderMap::new(), "ok");
            }
            "/refused" | "/refusing" => {
                // `/refused` refuses the first stream that asks for it, as a server short of
                // streams does, and answers the next; `/refusing` refuses every one.
                let first = shared
                    .seen
                    .borrow()
                    .requests
                    .iter()
                    .filter(|line| line.contains(" /refused "))
                    .count()
                    == 1;
                if head.path == "/refusing" || first {
                    respond.send_reset(Reason::REFUSED_STREAM);
                } else {
                    send(&mut respond, StatusCode::OK, HeaderMap::new(), "ok");
                }
            }
            "/same" => send(
                &mut respond,
                StatusCode::TEMPORARY_REDIRECT,
                location("/bid"),
                "",
            ),
            "/away" | "/held" => {
                if head.path == "/held" {
                    // Held until the bidder has read two, or for ten seconds at most.
                    let mut seen = shared.seen.subscribe();
                    let both = seen.wait_for(|seen| {
                        let requests = seen.requests.iter();
                        requests.filter(|line| line.contains(" /held ")).count() >= 2
                    });
                    drop(timeout(Duration::from_secs(10), both).await);
                }
                let away = format!("http://localhost:{}/bid", head.port);
                send(
                    &mut respond,
                    StatusCode::TEMPORARY_REDIRECT,
                    location(&away),
                    "",
                );
            }
            "/big" => {
                let mut headers = HeaderMap::new();
                if let Ok(value) = HeaderValue::from_str(&"a".repeat(BIG)) {
                    let _previous = headers.insert("x-big", value);
                }
                send(&mut respond, StatusCode::OK, headers, "ok");
            }
            _ => send(&mut respond, StatusCode::OK, HeaderMap::new(), "ok"),
        }
        None
    }

    /// A `Location` of `to`.
    fn location(to: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(to) {
            let _previous = headers.insert(LOCATION, value);
        }
        headers
    }

    /// Answers on `respond` with `status`, `headers` and `body`.
    fn send(
        respond: &mut SendResponse<Bytes>,
        status: StatusCode,
        headers: HeaderMap,
        body: &'static str,
    ) {
        let mut head = default_arg(SendResponse::<Bytes>::send_response);
        *head.status_mut() = status;
        *head.headers_mut() = headers;
        if let Ok(mut stream) = respond.send_response(head, body.is_empty())
            && !body.is_empty()
        {
            let _sent = stream.send_data(Bytes::from_static(body.as_bytes()), true);
        }
    }

    /// The default of the second argument of `method`: the response head h2 takes, a type of the
    /// `http` crate, which this crate does not name.
    fn default_arg<S, T: Default, R>(_method: fn(&mut S, T, bool) -> R) -> T {
        T::default()
    }

    /// router.go's general client settings with this port's HTTP/2 on
    /// (`http_client.http2.enabled`), under which HTTP-H2's pins run.
    const HTTP2_ON: HttpClientSettings = HttpClientSettings {
        http2: Http2Settings::ON,
        ..HttpClientSettings::GENERAL
    };

    /// The bidder client: router.go's general client negotiating HTTP/2, trusting `pki`'s CA,
    /// under the loopback guard.
    fn bidder_client(pki: &TestPki) -> Result<Client, BoxError> {
        Ok(Builder::new(HTTP2_ON, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .negotiate_http2()
            .build()?)
    }

    /// router.go's general client, its bidders' half negotiating HTTP/2, trusting `pki`'s CA,
    /// under the loopback guard.
    fn general_client(pki: &TestPki) -> Result<General<Client>, BoxError> {
        Ok(Builder::new(HTTP2_ON, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .negotiate_http2()
            .build_general()?)
    }

    /// gamma's bid request to `url`: [`bid`]'s, with `field` (`h2.json`'s gamma field), which
    /// gamma's adapter adds.
    fn gamma(url: &str, field: &str) -> Result<Request, BoxError> {
        let mut request = bid(url)?;
        let (name, value) = field.split_once(": ").ok_or("a field without a value")?;
        let _previous = request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
        Ok(request)
    }

    /// A bid request to `url`: a POST of `{"id":"r"}` with the Content-Type `h2.go` sets.
    fn bid(url: &str) -> Result<Request, BoxError> {
        let mut bid = request("POST", url)?;
        let _previous = bid
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(JSON));
        *bid.body_mut() = Some(BODY.into());
        Ok(bid)
    }

    /// The protocol of the answer to a bid request to `url`, or the error's kind.
    async fn post(client: &Client, url: &str) -> Result<String, BoxError> {
        sent(client, bid(url)?).await
    }

    /// The protocol of the answer to `request`, or the error's kind.
    async fn sent(client: &Client, request: Request) -> Result<String, BoxError> {
        Ok(match client.send(request, None, None).await {
            Ok(response) => {
                let fetched = fetch(response).await;
                drop(fetched.body?);
                format!("{:?}", fetched.version)
            }
            Err(error) => format!("error {:?}", error.kind()),
        })
    }

    /// The protocol, status and `Location` of the answer to a bid request to `url`.
    async fn answered(client: &Client, url: &str) -> Result<String, BoxError> {
        let fetched = fetch(client.send(bid(url)?, None, None).await?).await;
        let location = fetched
            .headers
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map_or_else(String::new, |location| format!(" {location}"));
        drop(fetched.body?);
        Ok(format!(
            "{:?} {}{location}",
            fetched.version,
            fetched.status.as_u16()
        ))
    }

    /// What bid requests that a bidder may redirect come to.
    #[derive(Debug, PartialEq, Eq)]
    struct Redirected {
        /// The answers, as [`answered`] writes them, sorted.
        answers: Vec<String>,
        /// The requests the bidder read for them.
        read: Vec<String>,
        /// Those requests, each as its method and the length of its body, and the answers'
        /// statuses, as [`go_chain`] writes what Go's client did.
        chain: (Vec<String>, Vec<i64>),
    }

    /// The bid requests to `urls`, sent at once to `bidder`, which may redirect them.
    async fn redirected(
        bidder: &Bidder,
        client: &Client,
        urls: &[String],
    ) -> Result<Redirected, BoxError> {
        let before = bidder.seen().requests.len();
        let mut tasks = JoinSet::new();
        for url in urls {
            let (client, url) = (client.clone(), url.clone());
            let _handle = tasks.spawn(async move { answered(&client, &url).await });
        }
        let mut answers = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            answers.push(joined??);
        }
        answers.sort();
        let read = bidder
            .seen()
            .requests
            .get(before..)
            .map_or_default(<[String]>::to_vec);
        let sent = read
            .iter()
            .map(|line| {
                let mut words = line.split(' ');
                let method = words.next().unwrap_or_default();
                format!("{method} {}", words.next_back().unwrap_or_default())
            })
            .collect();
        let mut statuses = Vec::new();
        for answer in &answers {
            let status = answer
                .split(' ')
                .nth(1)
                .ok_or("an answer without a status")?;
            statuses.push(status.parse()?);
        }
        Ok(Redirected {
            answers,
            read,
            chain: (sent, statuses),
        })
    }

    /// `redirects.json`'s case `name`: what Go's client did with a chain of redirects.
    fn redirect_case<'d>(redirects: &'d Value, name: &str) -> Result<&'d Value, BoxError> {
        list(redirects, "/cases")?
            .iter()
            .find(|case| text(case, "/name").ok() == Some(name))
            .ok_or_else(|| format!("redirects.json has no {name}").into())
    }

    /// The requests Go's client sent in `case`, one of `redirects.json`'s, each as its method and
    /// the length of its body, and the status of the answer it returned.
    fn go_chain(case: &Value) -> Result<(Vec<String>, i64), BoxError> {
        let mut sent = Vec::new();
        for hop in list(case, "/hops")? {
            let (method, body) = (text(hop, "/method")?, text(hop, "/body")?);
            sent.push(format!("{method} body={}", body.len()));
        }
        Ok((sent, int(case, "/status")?))
    }

    /// The protocol of the answer to a bid request to `url`, and its `GotConn` events, as
    /// `metrics.json` writes Go's.
    async fn traced_post(client: &Client, url: &str) -> Result<(String, Vec<String>), BoxError> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let trace = Trace::new({
            let events = Arc::clone(&events);
            move |event| {
                if let Event::GotConn { reused, .. } = event {
                    lock(&events).push(format!("GotConn reused={reused}"));
                }
            }
        });
        let fetched = fetch(client.send(bid(url)?, None, Some(trace)).await?).await;
        drop(fetched.body?);
        let recorded = lock(&events).clone();
        Ok((format!("{:?}", fetched.version), recorded))
    }

    /// A bidder speaking `transport` (`h2`: h2 over TLS; `h2c`) and the URL of its root.
    async fn start(
        transport: &str,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<(Bidder, String), BoxError> {
        Ok(if transport == "h2" {
            let bidder = Bidder::tls(pki, &[b"h2", b"http/1.1"], doc).await?;
            let base = format!("https://127.0.0.1:{}", bidder.port());
            (bidder, base)
        } else {
            let bidder = Bidder::cleartext(Preface::Serve, doc).await?;
            let base = format!("http://127.0.0.1:{}", bidder.port());
            (bidder, base)
        })
    }

    /// A bidder speaking `transport`, the bidder client, already on HTTP/2 with it (over h2c once
    /// a first request has started the probe and the probe has ended), and the bidder's root.
    async fn on_h2(
        transport: &str,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<(Bidder, Client, String), BoxError> {
        let (bidder, base) = start(transport, pki, doc).await?;
        let client = bidder_client(pki)?;
        if transport == "h2c" {
            check_eq(
                &post(&client, &format!("{base}/bid")).await?,
                &"HTTP/1.1".to_owned(),
                "the request that starts the probe",
            )?;
            let _probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
        }
        Ok((bidder, client, base))
    }

    /// `h2.json`'s selection of `client` with `server`: the protocols, and the requests the
    /// server read, sorted.
    fn selection(
        doc: &Value,
        client: &str,
        server: &str,
    ) -> Result<(Vec<String>, Vec<String>), BoxError> {
        let case = list(doc, "/selection")?
            .iter()
            .find(|case| {
                text(case, "/client").ok() == Some(client)
                    && text(case, "/server").ok() == Some(server)
            })
            .ok_or_else(|| format!("no selection of {client} with {server}"))?;
        let mut handled = strings(case, "/handled")?;
        handled.sort();
        Ok((strings(case, "/protos")?, handled))
    }

    /// `h2.json`'s selection server `server`, and the URL of a bid request to it.
    async fn selection_server(
        server: &str,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<(Bidder, String), BoxError> {
        let bidder = match server {
            "https_h2" => Bidder::tls(pki, &[b"h2", b"http/1.1"], doc).await?,
            "https_http1" => Bidder::tls(pki, &[b"http/1.1"], doc).await?,
            "h2c" => Bidder::cleartext(Preface::Serve, doc).await?,
            _ => Bidder::cleartext(answer(doc, "http1_handler")?, doc).await?,
        };
        let scheme = if server.starts_with("https") {
            "https"
        } else {
            "http"
        };
        let url = format!("{scheme}://127.0.0.1:{}/bid", bidder.port());
        Ok((bidder, url))
    }

    /// What the bidder client does with two bid requests to `server`, one of `h2.json`'s
    /// selection servers: the protocols of the answers, and the bid requests the server read,
    /// sorted (the probe's `PRI * HTTP/2.0` is `probe_sends_no_request`'s).
    async fn select(
        server: &str,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<(Vec<String>, Vec<String>), BoxError> {
        let (bidder, url) = selection_server(server, pki, doc).await?;
        let client = bidder_client(pki)?;
        let before = post(&client, &url).await?;
        if url.starts_with("http:") {
            let _probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
        }
        let after = post(&client, &url).await?;
        drop(client);
        let mut handled: Vec<String> = bidder
            .seen()
            .requests
            .into_iter()
            .filter(|handled| !handled.starts_with("PRI "))
            .collect();
        handled.sort();
        let _counts = bidder.close().await;
        Ok((vec![before, after], handled))
    }

    // Go: router/router.go:143-158 (the bidder client's Transport sets neither ForceAttemptHTTP2
    // nor Protocols), net/http/transport.go:477 Transport.protocols, and h2_bundle.go (ALPN h2,
    // and h2c with prior knowledge, which h2.json's http2 client speaks).
    #[test]
    fn alpn_and_h2c_selection() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut differ = Vec::new();
            for server in ["https_h2", "https_http1", "h2c", "http1"] {
                let go = selection(&doc, "general", server)?;
                let rust = select(server, &pki, &doc).await?;
                if rust != go {
                    differ.push((server.to_owned(), go, rust));
                }
            }
            // Where Go's library speaks HTTP/2 (h2.json's http2 client), the bidder client does
            // too, with Go-http-client/1.1 (D-UA); over h2c from the request after the probe.
            let d_ua = |(protos, handled): (Vec<String>, Vec<String>)| {
                let handled: Vec<String> = handled
                    .iter()
                    .map(|line| line.replace("Go-http-client/2.0", "Go-http-client/1.1"))
                    .collect();
                (protos, handled)
            };
            let h2 = d_ua(selection(&doc, "http2", "https_h2")?);
            let (h2c_protos, h2c_handled) = d_ua(selection(&doc, "http2", "h2c")?);
            let (go_protos, go_handled) = selection(&doc, "general", "h2c")?;
            let mut handled = vec![first(&go_handled)?, first(&h2c_handled)?];
            handled.sort();
            let protos = vec![first(&go_protos)?, first(&h2c_protos)?];
            check_eq(
                &differ,
                &vec![
                    (
                        "https_h2".to_owned(),
                        selection(&doc, "general", "https_h2")?,
                        h2,
                    ),
                    ("h2c".to_owned(), (go_protos, go_handled), (protos, handled)),
                ],
                "the servers the bidder client speaks to otherwise than Go's (Go, Rust)",
            )
        })
    }

    /// What the general client's own half does with two bid requests to `server`, one of
    /// `h2.json`'s selection servers, as [`select`] gives the bidder client's (the probe's
    /// `PRI * HTTP/2.0` left in), and the connections that had opened with HTTP/2's preface a
    /// moment after.
    async fn select_general(
        server: &str,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<((Vec<String>, Vec<String>), usize), BoxError> {
        let (bidder, url) = selection_server(server, pki, doc).await?;
        let clients = general_client(pki)?;
        let protos = vec![
            post(&clients.general, &url).await?,
            post(&clients.general, &url).await?,
        ];
        drop(clients);
        sleep(WAIT).await;
        let seen = bidder.seen();
        let mut handled = seen.requests;
        handled.sort();
        let _counts = bidder.close().await;
        Ok(((protos, handled), seen.probing + seen.prior))
    }

    // Go: router/router.go:143-158 (generalHttpClient: one Transport, HTTP/1.1 to every server)
    // and :211-247 (the modules, the stored-request fetchers, the vendor list and the bidders,
    // which all send through it).
    #[test]
    fn only_the_bidders_negotiate_http2() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut differ = Vec::new();
            for server in ["https_h2", "https_http1", "h2c", "http1"] {
                let go = (selection(&doc, "general", server)?, 0);
                let rust = select_general(server, &pki, &doc).await?;
                if rust != go {
                    differ.push((server.to_owned(), go, rust));
                }
            }
            check_eq(
                &differ,
                &Vec::new(),
                "the servers the general client's own half speaks to otherwise than Go's general \
                 client, or probes (Go, Rust)",
            )?;
            // Beside the bidders' half on HTTP/2, each half keeps its protocols: over h2c from
            // the request after the probe.
            let mut halves = Vec::new();
            for transport in ["h2", "h2c"] {
                let (bidder, base) = start(transport, &pki, &doc).await?;
                let clients = general_client(&pki)?;
                let url = format!("{base}/bid");
                let mut protos = Vec::new();
                if transport == "h2c" {
                    protos.push(post(&clients.bidders, &url).await?);
                    let _probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
                }
                for client in [
                    &clients.bidders,
                    &clients.general,
                    &clients.bidders,
                    &clients.general,
                ] {
                    protos.push(post(client, &url).await?);
                }
                drop(clients);
                let seen = bidder.seen();
                let _counts = bidder.close().await;
                halves.push((transport, protos, seen.probing, seen.prior));
            }
            let (h1, h2) = ("HTTP/1.1".to_owned(), "HTTP/2.0".to_owned());
            let in_turn = vec![h2.clone(), h1.clone(), h2, h1.clone()];
            let mut probed = vec![h1];
            probed.extend(in_turn.iter().cloned());
            check_eq(
                &halves,
                &vec![("h2", in_turn, 0, 0), ("h2c", probed, 1, 1)],
                "each transport: the protocols of the bidders' and the general requests in turn, \
                 the probes and the connections with prior knowledge",
            )
        })
    }

    /// The connections `h2.json`'s server accepted from Go's `client` in the selection with
    /// `server`.
    fn selection_connections(doc: &Value, client: &str, server: &str) -> Result<i64, BoxError> {
        let case = list(doc, "/selection")?
            .iter()
            .find(|case| {
                text(case, "/client").ok() == Some(client)
                    && text(case, "/server").ok() == Some(server)
            })
            .ok_or_else(|| format!("no selection of {client} with {server}"))?;
        int(case, "/connections")
    }

    /// A client of each kind that must speak as Go's general client does whatever its HTTP/2
    /// setting: the bidder client and the general client's bidders' half with HTTP/2 off (the
    /// default), and a client that is not the bidder client with HTTP/2 on.
    fn http1_clients(pki: &TestPki) -> Result<Vec<(&'static str, Client)>, BoxError> {
        let off = || {
            Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default())
                .roots(pki.roots())
                .loopback_only(0)
                .negotiate_http2()
        };
        Ok(vec![
            ("bidder client, HTTP/2 off", off().build()?),
            ("bidders' half, HTTP/2 off", off().build_general()?.bidders),
            (
                "plain client, HTTP/2 on",
                Builder::new(HTTP2_ON, ProxyEnv::default())
                    .roots(pki.roots())
                    .loopback_only(0)
                    .build()?,
            ),
        ])
    }

    // Go: router/router.go:143-158 (the bidder client's Transport: HTTP/1.1 to every server, no
    // h2 in ALPN) and h2.json's general client, whose selection records the protocols, the
    // request lines and the connections of two bid requests to each server.
    #[test]
    fn http2_off_is_go() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut differ = Vec::new();
            for server in ["https_h2", "https_http1", "h2c", "http1"] {
                let go = (
                    selection(&doc, "general", server)?,
                    selection_connections(&doc, "general", server)?,
                    0,
                );
                for (name, client) in http1_clients(&pki)? {
                    let (bidder, url) = selection_server(server, &pki, &doc).await?;
                    let first = post(&client, &url).await?;
                    // hyper hands an HTTP/1.1 connection back to its pool from the connection's
                    // task once the body has ended, where Go's body read waits at EOF until the
                    // connection is idle again: a moment lets it get there, so that the second
                    // request finds it as Go's does.
                    sleep(WAIT).await;
                    let protos = vec![first, post(&client, &url).await?];
                    drop(client);
                    sleep(WAIT).await;
                    let seen = bidder.seen();
                    let mut handled = seen.requests;
                    handled.sort();
                    let accepted = i64::try_from(bidder.accepted())?;
                    let _counts = bidder.close().await;
                    let rust = ((protos, handled), accepted, seen.probing + seen.prior);
                    if rust != go {
                        differ.push((server, name, go.clone(), rust));
                    }
                }
            }
            check_eq(
                &differ,
                &Vec::new(),
                "the servers a client with HTTP/2 off, or one that is not the bidder client, \
                 speaks to otherwise than Go's general client: the protocols and request lines \
                 (ALPN offering no h2 makes https_h2 pick http/1.1), the connections, the probes \
                 (Go, Rust)",
            )
        })
    }

    // Go: router/router.go:143-158 (the bidders' HTTP/1.1, which a redirect keeps) and
    // net/http/client.go:597-755 (Client.do follows a 307 with the method and the body).
    #[test]
    fn cleartext_redirect_to_tls_stays_on_http1() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, &[b"h2", b"http/1.1"], &doc).await?;
            let target = format!("https://127.0.0.1:{}/bid", bidder.port());
            let moved = redirect(&target);
            let redirecting = RawServer::start(move |mut conn: RawConn| {
                let moved = moved.clone();
                async move {
                    while conn.read_request().await.is_some() {
                        conn.write(moved.as_bytes()).await;
                    }
                }
            })
            .await?;
            let clients = general_client(&pki)?;
            let url = format!("http://127.0.0.1:{}/bid", redirecting.port());
            let protocols = vec![
                post(&clients.bidders, &url).await?,
                post(&clients.bidders, &target).await?,
            ];
            drop(clients);
            let _redirected = redirecting.close().await;
            let mut read = bidder.seen().requests;
            read.sort();
            let _counts = bidder.close().await;
            let line =
                |protocol: &str| format!("POST /bid {protocol} ua=Go-http-client/1.1 body=10");
            check_eq(
                &(protocols, read),
                &(
                    vec!["HTTP/1.1".to_owned(), "HTTP/2.0".to_owned()],
                    vec![line("HTTP/1.1"), line("HTTP/2.0")],
                ),
                "a bid request redirected from cleartext to the TLS bidder, then one sent to it \
                 at once: the protocols, and the requests the bidder read",
            )
        })
    }

    /// What two bid requests to a cleartext bidder doing `preface` come to: the protocols of
    /// their answers, the bytes of the probe the first started, the requests the bidder read
    /// (sorted), the probes it saw begin and the connections that opened with prior knowledge.
    ///
    /// Not every connection it accepted: hyper-util's client hands an HTTP/1.1 connection back to
    /// its pool from a task of its own once the connection is ready again, so the second request,
    /// which can follow the first at once, can open another under load, where Go's Transport has
    /// the connection back in its pool before the body's reader sees EOF
    /// (net/http/transport.go:2316-2320).
    type Probed = (Vec<String>, Vec<String>, Vec<String>, usize, usize);

    /// [`Probed`] for `preface`.
    async fn probe_of(preface: Preface, pki: &TestPki, doc: &Value) -> Result<Probed, BoxError> {
        let bidder = Bidder::cleartext(preface, doc).await?;
        let url = format!("http://127.0.0.1:{}/bid", bidder.port());
        let client = bidder_client(pki)?;
        let before = post(&client, &url).await?;
        let probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
        let after = post(&client, &url).await?;
        let mut seen = bidder.seen();
        seen.requests.sort();
        drop(client);
        let _counts = bidder.close().await;
        Ok((
            vec![before, after],
            probed.probes,
            seen.requests,
            seen.probing,
            seen.prior,
        ))
    }

    // Go: golang.org/x/net/http2 frame.go (the preface, SETTINGS and GOAWAY bytes h2.json
    // records) and net/http server.go readRequest (`PRI * HTTP/2.0`, which Go's HTTP/1.1 servers
    // read as a request: a ServeMux answers 400, a bare handler runs).
    #[test]
    fn probe_sends_no_request() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let hello = text(&doc, "/probe/hello")?.to_owned();
            let goodbye = text(&doc, "/probe/goodbye")?;
            let line =
                |protocol: &str| format!("POST /bid {protocol} ua=Go-http-client/1.1 body=10");
            let http1 = || "HTTP/1.1".to_owned();
            let mut got = Vec::new();
            let mut want = Vec::new();
            for server in list(&doc, "/probe/servers")? {
                let name = text(server, "/server")?;
                let h2c = name == "h2c";
                let preface = if h2c {
                    Preface::Serve
                } else {
                    answer(&doc, name)?
                };
                got.push((name.to_owned(), probe_of(preface, &pki, &doc).await?));
                let after = if h2c { "HTTP/2.0".to_owned() } else { http1() };
                let mut requests = vec![line("HTTP/1.1"), line(&after)];
                requests.extend(strings(server, "/handled")?);
                requests.sort();
                let sent = if h2c {
                    format!("{hello}{goodbye}")
                } else {
                    hello.clone()
                };
                let prior = usize::from(h2c);
                want.push((
                    name.to_owned(),
                    (vec![http1(), after], vec![sent], requests, 1, prior),
                ));
            }
            got.push((
                "closing".to_owned(),
                probe_of(Preface::Close, &pki, &doc).await?,
            ));
            want.push((
                "closing".to_owned(),
                (
                    vec![http1(), http1()],
                    vec![hello.clone()],
                    vec![line("HTTP/1.1"), line("HTTP/1.1")],
                    1,
                    0,
                ),
            ));
            check_eq(
                &got,
                &want,
                "each server's probe: the protocols before and after it, what it sent, the \
                 requests read, the probes and the connections with prior knowledge",
            )
        })
    }

    /// What becomes of a bid request to an origin marked h2c that has turned to `preface`, and of
    /// the next: their outcomes, the requests the bidder read and the connections it saw with
    /// prior knowledge.
    async fn demotion(
        preface: Preface,
        pki: &TestPki,
        doc: &Value,
    ) -> Result<(Vec<String>, Vec<String>, usize), BoxError> {
        let (bidder, client, base) = on_h2("h2c", pki, doc).await?;
        let url = format!("{base}/bid");
        bidder.set(preface);
        let failed = post(&client, &url).await?;
        let next = post(&client, &url).await?;
        drop(client);
        let seen = bidder.seen();
        let _counts = bidder.close().await;
        Ok((vec![failed, next], seen.requests, seen.prior))
    }

    // Go: net/http/transport.go Transport.roundTrip and persistConn.roundTrip (a request goes
    // again only when none of it was written, nothingWrittenError), on the HTTP/1.1 every Go
    // bidder request takes.
    #[test]
    fn demotion_resends_only_unread() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let line =
                |protocol: &str| format!("POST /bid {protocol} ua=Go-http-client/1.1 body=10");
            let h1 = || line("HTTP/1.1");
            let http1 = || "HTTP/1.1".to_owned();
            let cases = [
                ("answers_http1", answer(&doc, "http1_mux")?),
                ("fails_after_reading", Preface::Crash),
                ("closes_unanswered", Preface::Close),
            ];
            let mut got = Vec::new();
            for (name, preface) in cases {
                got.push((name, demotion(preface, &pki, &doc).await?));
            }
            let failed = || "error Request".to_owned();
            check_eq(
                &got,
                &vec![
                    (
                        "answers_http1",
                        (vec![http1(), http1()], vec![h1(), h1(), h1()], 1),
                    ),
                    (
                        "fails_after_reading",
                        (
                            vec![failed(), http1()],
                            vec![h1(), line("HTTP/2.0"), h1()],
                            1,
                        ),
                    ),
                    (
                        "closes_unanswered",
                        (vec![failed(), http1()], vec![h1(), h1()], 1),
                    ),
                ],
                "each case: the request that went over h2c and the next, the requests the bidder \
                 read, its connections with prior knowledge",
            )
        })
    }

    /// The `GotConn` events of two bid requests sent at once to `url`, with their protocols,
    /// sorted.
    async fn concurrent(client: &Client, url: &str) -> Result<Vec<String>, BoxError> {
        let mut tasks = JoinSet::new();
        for _ in 0..2 {
            let client = client.clone();
            let url = url.to_owned();
            let _handle = tasks.spawn(async move { traced_post(&client, &url).await });
        }
        let mut events = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            let (protocol, got) = joined??;
            events.extend(got.into_iter().map(|event| format!("{protocol} {event}")));
        }
        events.sort();
        Ok(events)
    }

    // Go: exchange/bidder.go:732-798 (GotConn's Reused, behind adapter_connection_created and
    // adapter_connection_reused) and h2_bundle.go http2ClientConn (one connection carries every
    // stream).
    #[test]
    fn connection_counts() -> TestResult {
        run(async {
            let metrics = golden("metrics.json")?;
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let case = list(&metrics, "/cases")?
                .iter()
                .find(|case| text(case, "/name").ok() == Some("concurrent_new_connections"))
                .ok_or("metrics.json has no concurrent_new_connections")?;
            let mut go = Vec::new();
            for request in list(case, "/events")? {
                let events = request.as_array().ok_or("events is not an array")?;
                go.extend(
                    events
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|event| event.starts_with("GotConn"))
                        .map(|event| format!("HTTP/1.1 {event}")),
                );
            }
            let mut got = Vec::new();
            for transport in ["h2", "h2c"] {
                let (bidder, base) = start(transport, &pki, &doc).await?;
                let client = bidder_client(&pki)?;
                // The first request: over h2 it opens the connection; over h2c it goes over
                // HTTP/1.1 and starts the probe.
                let (protocol, opening) = traced_post(&client, &format!("{base}/bid")).await?;
                let mut events: Vec<String> = opening
                    .into_iter()
                    .map(|event| format!("{protocol} {event}"))
                    .collect();
                if transport == "h2c" {
                    let _probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
                }
                events.extend(concurrent(&client, &format!("{base}/wait")).await?);
                let (_, later) = traced_post(&client, &format!("{base}/bid")).await?;
                drop(client);
                let probes = bidder.seen().probes.len();
                let _counts = bidder.close().await;
                got.push((transport, events, later, probes));
            }
            let event =
                |protocol: &str, reused: bool| format!("{protocol} GotConn reused={reused}");
            check_eq(
                &go,
                &vec![event("HTTP/1.1", false), event("HTTP/1.1", false)],
                "Go's two requests at once: a connection each",
            )?;
            let h2 = vec![
                event("HTTP/2.0", false),
                event("HTTP/2.0", true),
                event("HTTP/2.0", true),
            ];
            let h2c = vec![
                event("HTTP/1.1", false),
                event("HTTP/2.0", false),
                event("HTTP/2.0", true),
            ];
            let reused = vec!["GotConn reused=true".to_owned()];
            check_eq(
                &got,
                &vec![("h2", h2, reused.clone(), 0), ("h2c", h2c, reused, 1)],
                "the GotConn events over h2 and h2c: the first request, the two at once, the one \
                 after; the probes",
            )
        })
    }

    // Go: net/http/client.go Client.do and redirectBehavior (redirects.json's post_307 and
    // post_307_cross_host: Go's bidder client follows a 307 on the origin and to another origin
    // over HTTP/1.1, the body sent again).
    #[test]
    fn h2c_redirects_stay_on_the_origin() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let redirects = golden("redirects.json")?;
            let pki = TestPki::new()?;
            let (bidder, client, base) = on_h2("h2c", &pki, &doc).await?;
            let port = bidder.port();
            let mut got = Vec::new();
            // A 307 on the origin; two 307s to another origin, which the bidder holds until it
            // has read both requests, so that both are on their way over h2c when the first
            // comes back; and a 307 to another origin once the origin is back on HTTP/1.1.
            for urls in [vec!["/same"], vec!["/held"; 2], vec!["/away"]] {
                let urls: Vec<String> = urls.iter().map(|path| format!("{base}{path}")).collect();
                got.push(redirected(&bidder, &client, &urls).await?);
            }
            drop(client);
            let _counts = bidder.close().await;
            let line = |path: &str, protocol: &str| {
                format!("POST {path} {protocol} ua=Go-http-client/1.1 body=10")
            };
            let on_origin = go_chain(redirect_case(&redirects, "post_307")?)?;
            let cross = redirect_case(&redirects, "post_307_cross_host")?;
            let followed = go_chain(cross)?;
            // Over h2c each stops at the 307, where Go's client sends its second request.
            let first = followed.0.first().cloned().unwrap_or_default();
            let stopped = int(cross, "/steps/0/status")?;
            let away = format!("HTTP/2.0 307 http://localhost:{port}/bid");
            check_eq(
                &got,
                &vec![
                    Redirected {
                        answers: vec!["HTTP/2.0 200".to_owned()],
                        read: vec![line("/same", "HTTP/2.0"), line("/bid", "HTTP/2.0")],
                        chain: (on_origin.0, vec![on_origin.1]),
                    },
                    Redirected {
                        answers: vec![away.clone(), away],
                        read: vec![line("/held", "HTTP/2.0"); 2],
                        chain: (vec![first.clone(), first], vec![stopped; 2]),
                    },
                    Redirected {
                        answers: vec!["HTTP/1.1 200".to_owned()],
                        read: vec![line("/away", "HTTP/1.1"), line("/bid", "HTTP/1.1")],
                        chain: (followed.0, vec![followed.1]),
                    },
                ],
                "a 307 on the origin, two to another origin sent at once, and one to another \
                 origin once the origin is back on HTTP/1.1: the answers, the requests the bidder \
                 read, and those requests and the statuses next to Go's",
            )
        })
    }

    // Go: router/router.go:143-158 (Go's bidder client connects to a bidder only to send it a
    // request, and through the proxy the environment names).
    #[test]
    fn no_probe_before_the_first_request() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let bidder = Bidder::cleartext(Preface::Serve, &doc).await?;
            let url = format!("http://127.0.0.1:{}/bid", bidder.port());
            let proxy = RawServer::start(|mut conn: RawConn| async move {
                while conn.read_request().await.is_some() {
                    conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await;
                }
            })
            .await?;
            let client = bidder_client(&pki)?;
            let proxied = Builder::new(HTTP2_ON, ProxyEnv::default())
                .proxy_url(Url::parse(&format!("http://127.0.0.1:{}", proxy.port()))?)
                .loopback_only(0)
                .negotiate_http2()
                .build()?;
            let plain = Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default())
                .loopback_only(0)
                .build()?;
            sleep(WAIT).await;
            let idle = bidder.accepted();
            let others = vec![
                post(&proxied, &url).await?,
                post(&plain, &url).await?,
                post(&proxied, &url).await?,
                post(&plain, &url).await?,
            ];
            sleep(WAIT).await;
            let before = (bidder.accepted(), bidder.seen().probing);
            let first = post(&client, &url).await?;
            let probed = bidder.wait(|seen| !seen.probes.is_empty()).await?;
            let after = (bidder.accepted(), probed.probing);
            drop((client, proxied, plain));
            let _proxied = proxy.close().await;
            let _counts = bidder.close().await;
            check_eq(
                &(idle, others, before, first, after),
                &(
                    0,
                    vec!["HTTP/1.1".to_owned(); 4],
                    (1, 0),
                    "HTTP/1.1".to_owned(),
                    (3, 1),
                ),
                "the connections before any request, the proxied and plain clients' requests, \
                 the connections and probes before and after the bidder client's first",
            )
        })
    }

    // Go: net/http/h2_bundle.go http2ClientConn.encodeHeaders (the fields of h2.json's requests,
    // with User-Agent Go-http-client/2.0), and D-UA (Go-http-client/1.1 on every protocol).
    #[test]
    fn user_agent_on_h2() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut got = Vec::new();
            let mut want = Vec::new();
            for request in list(&doc, "/requests")? {
                let transport = text(request, "/transport")?;
                let (bidder, client, base) = on_h2(transport, &pki, &doc).await?;
                let protocol = post(&client, &format!("{base}/bid")).await?;
                drop(client);
                let read = bidder
                    .seen()
                    .h2
                    .last()
                    .cloned()
                    .ok_or("no HTTP/2 request")?;
                let port = bidder.port();
                let _counts = bidder.close().await;
                got.push((transport.to_owned(), protocol, read));
                want.push((
                    transport.to_owned(),
                    "HTTP/2.0".to_owned(),
                    as_sent(request, port)?,
                ));
            }
            check_eq(
                &got,
                &want,
                "each transport's bid request as the bidder read it: pseudo-header fields, \
                 fields, body",
            )
        })
    }

    /// One of `h2.json`'s requests of Go's HTTP/2 client as the bidder client sends it to a bidder
    /// on `port`: the pseudo-header fields, the fields with D-UA's User-Agent and reqwest's Accept
    /// (http-client.default-accept), sorted, and the body.
    fn as_sent(request: &Value, port: u16) -> Result<(Vec<String>, Vec<String>, String), BoxError> {
        let pseudo = strings(request, "/pseudo")?
            .iter()
            .map(|field| ports_in(field, &[(port, "PORT")]))
            .collect();
        let mut fields: Vec<String> = strings(request, "/fields")?
            .iter()
            .map(|field| field.replace("Go-http-client/2.0", "Go-http-client/1.1"))
            .collect();
        fields.push("accept: */*".to_owned());
        fields.sort();
        Ok((pseudo, fields, text(request, "/body")?.to_owned()))
    }

    /// The protocol of the answer to gamma's bid request, carrying `field`, from a TLS bidder that
    /// offers HTTP/1.1 only, and the head the bidder read.
    async fn gamma_over_http1(
        pki: &TestPki,
        doc: &Value,
        field: &str,
    ) -> Result<(String, Vec<String>), BoxError> {
        let bidder = Bidder::tls(pki, &[b"http/1.1"], doc).await?;
        let client = bidder_client(pki)?;
        let url = format!("https://127.0.0.1:{}/bid", bidder.port());
        let protocol = sent(&client, gamma(&url, field)?).await?;
        drop(client);
        let head = bidder
            .seen()
            .heads
            .last()
            .cloned()
            .ok_or("no HTTP/1.1 request")?;
        let _counts = bidder.close().await;
        Ok((protocol, head))
    }

    // Go: adapters/gamma/gamma.go:134 (Connection: keep-alive); net/http/request.go Request.write,
    // which writes it over HTTP/1.1 (h2.json's gamma http1), and net/http/h2_bundle.go
    // encodeHeaders, which leaves the connection-specific fields out of an HTTP/2 request (gamma
    // requests), as hyper's HTTP/2 client does (RFC 9113 section 8.2.2).
    #[test]
    fn connection_fields_dropped_on_h2() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let field = text(&doc, "/gamma/field")?;
            let carries =
                |lines: &[String]| lines.iter().any(|line| line.eq_ignore_ascii_case(field));
            // Over HTTP/1.1 the field goes as Go's bidder client sends it.
            let (http1, head) = gamma_over_http1(&pki, &doc, field).await?;
            check_eq(
                &(http1, carries(&head)),
                &(
                    "HTTP/1.1".to_owned(),
                    carries(&strings(&doc, "/gamma/http1")?),
                ),
                "the protocol of gamma's bid request to an HTTP/1.1 bidder, and whether it carries \
                 the field, as Go's does",
            )?;
            // Over h2 and h2c it goes as Go's HTTP/2 client sends it: without the field.
            let mut got = Vec::new();
            let mut want = Vec::new();
            for request in list(&doc, "/gamma/requests")? {
                let transport = text(request, "/transport")?;
                let (bidder, client, base) = on_h2(transport, &pki, &doc).await?;
                let protocol = sent(&client, gamma(&format!("{base}/bid"), field)?).await?;
                drop(client);
                let read = bidder
                    .seen()
                    .h2
                    .last()
                    .cloned()
                    .ok_or("no HTTP/2 request")?;
                let port = bidder.port();
                let _counts = bidder.close().await;
                got.push((transport.to_owned(), protocol, read));
                want.push((
                    transport.to_owned(),
                    "HTTP/2.0".to_owned(),
                    as_sent(request, port)?,
                ));
            }
            check_eq(
                &got,
                &want,
                "each transport's gamma bid request as the bidder read it: pseudo-header fields, \
                 fields, body",
            )
        })
    }

    // Go: net/http/transfer.go transferWriter.shouldSendContentLength (Content-Length: 0 for a
    // POST, PUT or PATCH without a body, none for a DELETE: headers.json's cases post_empty to
    // delete_empty). hyper's HTTP/2 client writes the same lengths, where its HTTP/1.1 writes none
    // (http-client.empty-body-length).
    #[test]
    fn empty_bodies_framed_on_h2() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let heads = golden("headers.json")?;
            let pki = TestPki::new()?;
            let lengths = |lines: &[String], prefix: &str| -> Vec<String> {
                lines
                    .iter()
                    .filter_map(|line| line.strip_prefix(prefix))
                    .map(ToOwned::to_owned)
                    .collect()
            };
            let mut got = Vec::new();
            let mut want = Vec::new();
            for transport in ["h2", "h2c"] {
                let (bidder, client, base) = on_h2(transport, &pki, &doc).await?;
                for case in list(&heads, "/cases")? {
                    let name = text(case, "/name")?;
                    if !name.ends_with("_empty") {
                        continue;
                    }
                    let method = text(case, "/method")?;
                    let protocol =
                        sent(&client, request(method, &format!("{base}/p?q=1"))?).await?;
                    let read = bidder
                        .seen()
                        .h2
                        .last()
                        .cloned()
                        .ok_or("no HTTP/2 request")?;
                    got.push((
                        transport,
                        name.to_owned(),
                        protocol,
                        lengths(&read.1, "content-length: "),
                    ));
                    want.push((
                        transport,
                        name.to_owned(),
                        "HTTP/2.0".to_owned(),
                        lengths(&strings(case, "/head")?, "Content-Length: "),
                    ));
                }
                drop(client);
                let _counts = bidder.close().await;
            }
            check_eq(
                &got,
                &want,
                "each transport and bodiless case of headers.json: the protocol, and the \
                 content-length the bidder read (Go's)",
            )
        })
    }

    // Go: none: Go's bidder client speaks HTTP/1.1, which has no REFUSED_STREAM. reqwest's default
    // retry policy (its retry module: protocol NACKs) sends a request whose stream the server
    // refused unprocessed (RFC 9113 section 8.7) again at once, at most twice; Go's HTTP/2 client
    // sends it again up to seven times, backing off a second and longer after the first
    // (net/http/h2_bundle.go:7893-7923 and http2canRetryError).
    #[test]
    fn refused_streams_go_again() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut got = Vec::new();
            for transport in ["h2", "h2c"] {
                let (bidder, client, base) = on_h2(transport, &pki, &doc).await?;
                let refused = post(&client, &format!("{base}/refused")).await?;
                let refusing = post(&client, &format!("{base}/refusing")).await?;
                let next = post(&client, &format!("{base}/bid")).await?;
                drop(client);
                let seen = bidder.seen();
                let read = |path: &str| {
                    let path = format!(" {path} ");
                    seen.requests
                        .iter()
                        .filter(|line| line.contains(&path))
                        .count()
                };
                let reads = (read("/refused"), read("/refusing"));
                let _counts = bidder.close().await;
                got.push((transport, (refused, reads.0), (refusing, reads.1), next));
            }
            // Over h2c the failure moves the origin back to HTTP/1.1 (http-client.h2c-demotion).
            let want = |transport, next: &str| {
                (
                    transport,
                    ("HTTP/2.0".to_owned(), 2),
                    ("error Request".to_owned(), 3),
                    next.to_owned(),
                )
            };
            check_eq(
                &got,
                &vec![want("h2", "HTTP/2.0"), want("h2c", "HTTP/1.1")],
                "each transport: a request refused once and one refused every time, each with the \
                 streams the bidder got for it, then the protocol of the next request",
            )
        })
    }

    /// What a bid request to `/slow` with a deadline, and one dropped while it waits, come to on
    /// the connection a first bid request opened: the deadline's error, the streams the bidder
    /// saw reset, the `GotConn` events of the first request (the first response on its
    /// connection) and of a request after each (reused ones), and the connections the bidder
    /// accepted and saw with prior knowledge.
    type Stopped = (String, Vec<String>, [Vec<String>; 3], (usize, usize));

    /// [`Stopped`] over `transport`.
    async fn stopped(transport: &str, pki: &TestPki, doc: &Value) -> Result<Stopped, BoxError> {
        let (bidder, client, base) = on_h2(transport, pki, doc).await?;
        // The connection is up before the deadline starts, so the deadline passes on an open
        // stream: on a loaded machine a handshake can outlast it, and the request then fails
        // with no stream to reset.
        let (_, opened) = traced_post(&client, &format!("{base}/bid")).await?;
        let slow = format!("{base}/slow");
        let deadline = Instant::now().checked_add(WAIT).ok_or("no deadline")?;
        let timed_out = match client.send(bid(&slow)?, Some(deadline), None).await {
            Ok(response) => format!("answered {:?}", response.status()),
            Err(error) => format!("{:?} {error}", error.kind()),
        };
        let _reset = bidder.wait(|seen| seen.resets.len() == 1).await?;
        let (_, after_deadline) = traced_post(&client, &format!("{base}/bid")).await?;
        let dropped = tokio::spawn({
            let client = client.clone();
            let slow = slow.clone();
            async move {
                drop(client.send(bid(&slow)?, None, None).await?);
                Ok::<(), BoxError>(())
            }
        });
        let _read = bidder
            .wait(|seen| {
                seen.requests
                    .iter()
                    .filter(|line| line.contains("/slow"))
                    .count()
                    == 2
            })
            .await?;
        dropped.abort();
        let _aborted = dropped.await;
        let seen = bidder.wait(|seen| seen.resets.len() == 2).await?;
        let (_, after_drop) = traced_post(&client, &format!("{base}/bid")).await?;
        let connections = (bidder.accepted(), bidder.seen().prior);
        drop(client);
        let _counts = bidder.close().await;
        Ok((
            timed_out,
            seen.resets,
            [opened, after_deadline, after_drop],
            connections,
        ))
    }

    // Go: x/net/context/ctxhttp Do (the request's deadline) and net/http/h2_bundle.go
    // http2clientStream.abortStream (RST_STREAM CANCEL once a request's context is done).
    #[test]
    fn timeouts_and_cancellation_on_h2() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let mut got = Vec::new();
            for transport in ["h2", "h2c"] {
                got.push((transport, stopped(transport, &pki, &doc).await?));
            }
            // One HTTP/2 connection carries every request: over TLS the only one the bidder
            // accepted; over h2c the one with prior knowledge, beside the probe's request's and
            // the probe's.
            let want = |transport, connections| {
                let reused = vec!["GotConn reused=true".to_owned()];
                (
                    transport,
                    (
                        "DeadlineExceeded context deadline exceeded".to_owned(),
                        vec!["/slow CANCEL".to_owned(); 2],
                        [
                            vec!["GotConn reused=false".to_owned()],
                            reused.clone(),
                            reused,
                        ],
                        connections,
                    ),
                )
            };
            check_eq(
                &got,
                &vec![want("h2", (1, 0)), want("h2c", (3, 1))],
                "each transport: the deadline's error, the resets, the connection of the first \
                 request and of one after each, the connections",
            )
        })
    }

    // Go: net/http/transport.go:329-334 (maxHeaderResponseSize: 10 MiB by default) and
    // net/http/h2_bundle.go:7489-7501 (maxHeaderListSize, which bounds an HTTP/2 response's
    // fields by it).
    #[test]
    fn h2_takes_gos_response_header_limit() -> TestResult {
        run(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let (bidder, client, base) = on_h2("h2", &pki, &doc).await?;
            let fetched = fetch(
                client
                    .send(bid(&format!("{base}/big"))?, None, None)
                    .await?,
            )
            .await;
            drop(client);
            let _counts = bidder.close().await;
            let big = fetched.headers.get("x-big").map(HeaderValue::len);
            check_eq(
                &(format!("{:?}", fetched.version), big),
                &("HTTP/2.0".to_owned(), Some(BIG)),
                "a response with a header past hyper's default list size",
            )
        })
    }

    /// Runs a pin on a current-thread runtime, whose clock [`skip`] can move on.
    fn run_local<F: Future<Output = TestResult>>(pin: F) -> TestResult {
        runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(pin)
    }

    /// Moves tokio's clock on by `by` at once: the timers it passes fire.
    async fn skip(by: Duration) {
        time::pause();
        time::advance(by).await;
        time::resume();
    }

    // Go: none: Go's bidder client never probes (router/router.go:143-158); the probe's timeout
    // and the verdict's lifetime are http-client.h2c-probe's.
    #[test]
    #[ignore = "moves tokio's clock on; run with -- --ignored h2c"]
    fn h2c_probe_gives_up_on_a_stalling_server() -> TestResult {
        run_local(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let bidder = Bidder::cleartext(Preface::Stall, &doc).await?;
            let url = format!("http://127.0.0.1:{}/bid", bidder.port());
            let client = bidder_client(&pki)?;
            let first = post(&client, &url).await?;
            let _hello = bidder.wait(|seen| seen.probing == 1).await?;
            skip(H2C_PROBE_TIMEOUT.saturating_sub(SECOND)).await;
            sleep(WAIT).await;
            let waiting = bidder.seen().probes.len();
            skip(SECOND).await;
            let ended = bidder.wait(|seen| !seen.probes.is_empty()).await?;
            let second = post(&client, &url).await?;
            sleep(WAIT).await;
            let holding = bidder.seen().probing;
            skip(H2C_VERDICT_TTL).await;
            let third = post(&client, &url).await?;
            let again = bidder.wait(|seen| seen.probing == 2).await?.probing;
            skip(H2C_PROBE_TIMEOUT).await;
            let _gave_up = bidder.wait(|seen| seen.probes.len() == 2).await?;
            drop(client);
            let _counts = bidder.close().await;
            check_eq(
                &(
                    vec![first, second, third],
                    waiting,
                    ended.probes,
                    holding,
                    again,
                ),
                &(
                    vec!["HTTP/1.1".to_owned(); 3],
                    0,
                    vec![text(&doc, "/probe/hello")?.to_owned()],
                    1,
                    2,
                ),
                "the protocols, the probe a second before its timeout, what it sent by then, the \
                 probes within the verdict's lifetime and after it",
            )
        })
    }

    // Go: none: Go's bidder client never probes (router/router.go:143-158); the verdict's
    // lifetime is http-client.h2c-probe's.
    #[test]
    #[ignore = "moves tokio's clock on; run with -- --ignored h2c"]
    fn h2c_verdict_expires() -> TestResult {
        run_local(async {
            let doc = golden("h2.json")?;
            let pki = TestPki::new()?;
            let (bidder, client, base) = on_h2("h2c", &pki, &doc).await?;
            let url = format!("{base}/bid");
            let marked = post(&client, &url).await?;
            skip(H2C_VERDICT_TTL.saturating_sub(SECOND)).await;
            let holding = post(&client, &url).await?;
            let probing = bidder.seen().probing;
            skip(SECOND).await;
            let expired = post(&client, &url).await?;
            let again = bidder.wait(|seen| seen.probes.len() == 2).await?.probing;
            let renewed = post(&client, &url).await?;
            drop(client);
            let _counts = bidder.close().await;
            let (h1, h2) = ("HTTP/1.1".to_owned(), "HTTP/2.0".to_owned());
            check_eq(
                &(vec![marked, holding, expired, renewed], probing, again),
                &(vec![h2.clone(), h2.clone(), h1, h2], 1, 2),
                "the protocols through the verdict's lifetime and past it, and the probes",
            )
        })
    }
}
