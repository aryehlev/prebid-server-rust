//! The pins of the bidder client's pool of HTTP/2 connections (`http-client.http2-opt-in`,
//! ruling 10 (a)), with `http_client.http2.enabled` on: loopback bidders that speak h2 over TLS,
//! or h2c after a probe, advertise a small `SETTINGS_MAX_CONCURRENT_STREAMS`, hold requests until
//! told, and say GOAWAY on demand. Every answer names the connection that carried it (`x-conn`,
//! the bidder's count of its HTTP/2 connections, probes left out). Go's bidder client has no
//! HTTP/2, so nothing here has a Go golden; `tests/h2.rs::http2_off_is_go` pins Go's behaviour
//! with the setting off.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        future::poll_fn,
        sync::{Arc, Mutex, MutexGuard, PoisonError},
        time::Duration,
    };

    use bytes::Bytes;
    use h2::{
        Reason, RecvStream,
        server::{Builder as ServerBuilder, SendResponse},
    };
    use pbs_http_client::{
        Builder, Client, DialerSettings, Event, Http2Settings, HttpClientSettings, ProxyEnv, Trace,
        reqwest::{
            self, Method, Request, StatusCode, Url,
            header::{HeaderName, HeaderValue},
        },
    };
    use tokio::{
        net::TcpListener,
        sync::{Semaphore, mpsc, oneshot, watch},
        task::{JoinHandle, JoinSet},
        time::{Instant, sleep, timeout},
    };

    use crate::support::{
        BoxError, RawConn, RawServer, TestPki, TestResult, check_eq, fetch, golden, list, request,
        run, strings, text, unhex,
    };

    /// HTTP/2's client connection preface.
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

    /// The probe's SETTINGS frame: empty.
    const EMPTY_SETTINGS: [u8; 9] = [0, 0, 0, 4, 0, 0, 0, 0, 0];

    /// How long a check waits to see that nothing happens.
    const WAIT: Duration = Duration::from_millis(200);

    /// router.go's general client settings with this port's HTTP/2 on.
    const HTTP2_ON: HttpClientSettings = HttpClientSettings {
        http2: Http2Settings::ON,
        ..HttpClientSettings::GENERAL
    };

    /// What a bidder saw.
    #[derive(Debug, Default, Clone)]
    struct Book {
        /// The HTTP/2 requests it read, in turn: their connection, path and `x-id`.
        read: Vec<(usize, String, String)>,
        /// The `x-id`s of the HTTP/2 requests it answered 200, in turn.
        answered: Vec<String>,
        /// The `x-id`s of the streams it refused unread.
        refused: Vec<String>,
        /// The HTTP/1.1 requests it read, as their request lines.
        http1: Vec<String>,
        /// The HTTP/2 connections that opened with a request's preface (probes left out).
        opened: usize,
        /// Those that have ended, by number.
        ended: Vec<usize>,
        /// The probes that have ended.
        probes: usize,
        /// The streams the client reset: their path and the reason.
        resets: Vec<String>,
    }

    impl Book {
        /// The requests read for `path`.
        fn count(&self, path: &str) -> usize {
            self.read.iter().filter(|(_, read, _)| read == path).count()
        }

        /// The connection the request with `x-id` `tag` was read on.
        fn conn_of(&self, tag: &str) -> Option<usize> {
            self.read
                .iter()
                .find(|(_, _, read)| read == tag)
                .map(|(conn, ..)| *conn)
        }
    }

    /// What a bidder's connections share.
    #[derive(Debug)]
    struct Shared {
        book: watch::Sender<Book>,
        /// The `SETTINGS_MAX_CONCURRENT_STREAMS` it advertises.
        streams: u32,
        /// The permits `/hold` waits for.
        release: Semaphore,
    }

    impl Shared {
        fn record<F: FnOnce(&mut Book)>(&self, change: F) {
            self.book.send_modify(change);
        }
    }

    /// A loopback bidder.
    #[derive(Debug)]
    struct Bidder {
        server: RawServer,
        shared: Arc<Shared>,
    }

    impl Bidder {
        fn shared(streams: u32) -> Arc<Shared> {
            Arc::new(Shared {
                book: watch::Sender::new(Book::default()),
                streams,
                release: Semaphore::new(0),
            })
        }

        /// A TLS bidder offering only `h2` in ALPN, advertising `streams`.
        async fn tls(pki: &TestPki, streams: u32) -> Result<Self, BoxError> {
            Self::tls_offering(pki, &[b"h2"], streams).await
        }

        /// A TLS bidder offering `alpn`, HTTP/2 when the client takes `h2` (advertising
        /// `streams`) and HTTP/1.1 otherwise.
        async fn tls_offering(
            pki: &TestPki,
            alpn: &[&[u8]],
            streams: u32,
        ) -> Result<Self, BoxError> {
            let shared = Self::shared(streams);
            let server = RawServer::start_tls(pki.acceptor(alpn)?, {
                let shared = Arc::clone(&shared);
                move |conn| serve(conn, Arc::clone(&shared))
            })
            .await?;
            Ok(Self { server, shared })
        }

        /// A cleartext bidder: HTTP/1.1, and h2c to a probe or with prior knowledge, advertising
        /// `streams`.
        async fn cleartext(streams: u32) -> Result<Self, BoxError> {
            let shared = Self::shared(streams);
            let server = RawServer::start({
                let shared = Arc::clone(&shared);
                move |conn| serve(conn, Arc::clone(&shared))
            })
            .await?;
            Ok(Self { server, shared })
        }

        /// The URL of `path` on the bidder, at `host`.
        fn url(&self, scheme: &str, host: &str, path: &str) -> String {
            format!("{scheme}://{host}:{}{path}", self.server.port())
        }

        fn book(&self) -> Book {
            self.shared.book.borrow().clone()
        }

        /// Lets `n` held requests be answered.
        fn release(&self, n: usize) {
            self.shared.release.add_permits(n);
        }

        /// What the bidder has seen once `done` holds, waiting up to ten seconds.
        async fn wait<F: FnMut(&Book) -> bool>(&self, done: F) -> Result<Book, BoxError> {
            let mut book = self.shared.book.subscribe();
            let found = timeout(Duration::from_secs(10), book.wait_for(done))
                .await
                .map_err(|elapsed| format!("{elapsed}: the bidder saw only {:?}", self.book()))??
                .clone();
            Ok(found)
        }

        async fn close(self) {
            let _counts = self.server.close().await;
        }
    }

    /// Serves one connection: HTTP/2 over TLS when ALPN took `h2`, or after HTTP/2's preface in
    /// cleartext, HTTP/1.1 otherwise.
    async fn serve(mut conn: RawConn, shared: Arc<Shared>) {
        let probe = match conn.alpn() {
            None => {
                if conn.peek(PREFACE.len()).await != PREFACE {
                    return http1(conn, &shared).await;
                }
                let hello = conn.peek(PREFACE.len() + EMPTY_SETTINGS.len()).await;
                hello.get(PREFACE.len()..) == Some(EMPTY_SETTINGS.as_slice())
            }
            Some(b"h2") => false,
            Some(_) => return http1(conn, &shared).await,
        };
        let mut id = 0;
        if !probe {
            shared.record(|book| {
                id = book.opened;
                book.opened += 1;
            });
        }
        let handshake = ServerBuilder::new()
            .max_concurrent_streams(shared.streams)
            .handshake::<_, Bytes>(conn.into_stream())
            .await;
        if let Ok(mut connection) = handshake {
            // A `/goaway` request asks for GOAWAY here, and is answered once it has been said.
            let (asks, mut asked) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
            loop {
                tokio::select! {
                    accepted = connection.accept() => {
                        let Some(Ok((request, respond))) = accepted else {
                            break;
                        };
                        let shared = Arc::clone(&shared);
                        drop(tokio::spawn(answer(id, request, respond, shared, asks.clone())));
                    }
                    Some(said) = asked.recv() => {
                        connection.graceful_shutdown();
                        let _told = said.send(());
                    }
                }
            }
        }
        shared.record(|book| {
            if probe {
                book.probes += 1;
            } else {
                book.ended.push(id);
            }
        });
    }

    /// Serves HTTP/1.1: every request answered `ok`.
    async fn http1(mut conn: RawConn, shared: &Shared) {
        while let Some(request) = conn.read_request().await {
            let line = request.head.first().cloned().unwrap_or_default();
            shared.record(|book| book.http1.push(line));
            conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    }

    /// Reads a request on connection `id`, records it and answers it after its path: `/hold`
    /// once released, `/wait` after a moment, `/slow` never (its reset is recorded), `/refuse`
    /// refused unread the first time its `x-id` comes, `/reset` reset with `INTERNAL_ERROR`
    /// once read, `/goaway` once the connection has said GOAWAY (asked through `asks`), anything
    /// else at once.
    async fn answer(
        id: usize,
        request: http::Request<RecvStream>,
        mut respond: SendResponse<Bytes>,
        shared: Arc<Shared>,
        asks: mpsc::UnboundedSender<oneshot::Sender<()>>,
    ) {
        let (parts, mut body) = request.into_parts();
        while let Some(Ok(chunk)) = body.data().await {
            let _released = body.flow_control().release_capacity(chunk.len());
        }
        let path = parts.uri.path().to_owned();
        let tag = parts
            .headers
            .get("x-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        shared.record(|book| book.read.push((id, path.clone(), tag.clone())));
        match path.as_str() {
            "/hold" => {
                if let Ok(permit) = shared.release.acquire().await {
                    permit.forget();
                }
            }
            "/wait" => sleep(Duration::from_millis(20)).await,
            "/slow" => {
                if let Ok(reason) = poll_fn(|cx| respond.poll_reset(cx)).await {
                    shared.record(|book| book.resets.push(format!("/slow {reason:?}")));
                }
                return;
            }
            "/refuse" => {
                let mut first = false;
                shared.record(|book| {
                    first = !book.refused.contains(&tag);
                    if first {
                        book.refused.push(tag.clone());
                    }
                });
                if first {
                    respond.send_reset(Reason::REFUSED_STREAM);
                    return;
                }
            }
            "/reset" => {
                respond.send_reset(Reason::INTERNAL_ERROR);
                return;
            }
            "/goaway" => {
                let (said, saying) = oneshot::channel();
                if asks.send(said).is_ok() {
                    drop(saying.await);
                }
            }
            _ => {}
        }
        let mut head = http::Response::new(());
        *head.status_mut() = StatusCode::OK;
        drop(head.headers_mut().insert("x-conn", HeaderValue::from(id)));
        if let Ok(mut stream) = respond.send_response(head, false) {
            drop(stream.send_data(Bytes::from_static(b"ok"), true));
        }
        shared.record(|book| book.answered.push(tag));
    }

    /// Locks `mutex`, through a poisoning.
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The bidder client with `settings`, trusting `pki`'s CA, under the loopback guard.
    fn bidder_client(pki: &TestPki, settings: HttpClientSettings) -> Result<Client, BoxError> {
        Ok(Builder::new(settings, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .negotiate_http2()
            .build()?)
    }

    /// A bid request to `url` with `x-id: tag`.
    fn bid(url: &str, tag: &str) -> Result<Request, BoxError> {
        let mut request = Request::new(Method::POST, Url::parse(url)?);
        *request.body_mut() = Some(r#"{"id":"r"}"#.into());
        drop(
            request
                .headers_mut()
                .insert("x-id", HeaderValue::from_str(tag)?),
        );
        Ok(request)
    }

    /// What a request got: the connection that answered (`x-conn`), the protocol, its connection
    /// events and its wait for a stream or a slot.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Got {
        conn: String,
        version: String,
        events: Vec<String>,
        wait: Duration,
    }

    /// `event` as the pins compare it.
    fn describe(event: Event) -> String {
        if let Event::GotConn { reused, .. } = event {
            format!("GotConn reused={reused}")
        } else if matches!(event, Event::Dns { .. }) {
            "Dns".to_owned()
        } else {
            format!("{event:?}")
        }
    }

    /// Sends a bid request to `url` with `x-id: tag` and reads the answer.
    async fn send(client: &Client, url: &str, tag: &str) -> Result<Got, BoxError> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let waited = Arc::new(Mutex::new(Duration::ZERO));
        let trace = Trace::new({
            let events = Arc::clone(&events);
            let waited = Arc::clone(&waited);
            move |event| {
                if let Event::GotConn { wait, .. } = event {
                    *lock(&waited) = wait;
                }
                lock(&events).push(describe(event));
            }
        });
        let fetched = fetch(client.send(bid(url, tag)?, None, Some(trace)).await?).await;
        let conn = fetched
            .headers
            .get("x-conn")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("none")
            .to_owned();
        let version = format!("{:?}", fetched.version);
        drop(fetched.body?);
        let wait = *lock(&waited);
        let events = lock(&events).clone();
        Ok(Got {
            conn,
            version,
            events,
            wait,
        })
    }

    /// [`send`] in a task of its own.
    fn spawn_send(client: &Client, url: &str, tag: &str) -> JoinHandle<Result<Got, BoxError>> {
        let client = client.clone();
        let url = url.to_owned();
        let tag = tag.to_owned();
        tokio::spawn(async move { send(&client, &url, &tag).await })
    }

    /// The connections the answers came on.
    fn conns(got: &[&Got]) -> Vec<String> {
        got.iter().map(|got| got.conn.clone()).collect()
    }

    // Go: none (the bidder client has no HTTP/2); ruling 10 (a): a request takes the pooled
    // connection with the most free streams, the peer's SETTINGS_MAX_CONCURRENT_STREAMS less its
    // open ones.
    #[test]
    fn streams_go_to_the_least_loaded_connection() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 4).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            let (bid_url, hold_url) = (
                bidder.url("https", "127.0.0.1", "/bid"),
                bidder.url("https", "127.0.0.1", "/hold"),
            );
            // The first answer brings the bidder's SETTINGS.
            let warm = send(&client, &bid_url, "warm").await?;
            let mut first = JoinSet::new();
            for n in 0..4 {
                let _handle = first.spawn({
                    let client = client.clone();
                    let hold_url = hold_url.clone();
                    async move { send(&client, &hold_url, &format!("a{n}")).await }
                });
            }
            let _four = bidder.wait(|book| book.count("/hold") == 4).await?;
            let fifth = spawn_send(&client, &hold_url, "e");
            let _five = bidder.wait(|book| book.count("/hold") == 5).await?;
            // Two of the first connection's four end: it has two free streams, the second three.
            bidder.release(2);
            for _ in 0..2 {
                let _ended = first.join_next().await.ok_or("no held request")???;
            }
            let next = send(&client, &bid_url, "next").await?;
            let after = send(&client, &bid_url, "after").await?;
            bidder.release(3);
            while let Some(joined) = first.join_next().await {
                let _ended = joined??;
            }
            let fifth = fifth.await??;
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            let held: Vec<Option<usize>> = ["a0", "a1", "a2", "a3", "e"]
                .iter()
                .map(|tag| book.conn_of(tag))
                .collect();
            check_eq(
                &(conns(&[&warm, &fifth, &next, &after]), held, book.opened),
                &(
                    vec![
                        "0".to_owned(),
                        "1".to_owned(),
                        "1".to_owned(),
                        "1".to_owned(),
                    ],
                    vec![Some(0), Some(0), Some(0), Some(0), Some(1)],
                    2,
                ),
                "the first answer's, the fifth held one's and the two later ones' connections, \
                 the held requests' connections, and the connections opened",
            )
        })
    }

    // Go: none; ruling 10 (a): when every connection is full, another is opened.
    #[test]
    fn a_full_connection_opens_another() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 1).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            let bid_url = bidder.url("https", "127.0.0.1", "/bid");
            let warm = send(&client, &bid_url, "warm").await?;
            let held = spawn_send(&client, &bidder.url("https", "127.0.0.1", "/hold"), "held");
            let _read = bidder.wait(|book| book.count("/hold") == 1).await?;
            let second = send(&client, &bid_url, "second").await?;
            bidder.release(1);
            let held = held.await??;
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            check_eq(
                &(
                    conns(&[&warm, &held, &second]),
                    [warm.version, held.version, second.version],
                    book.opened,
                ),
                &(
                    vec!["0".to_owned(), "0".to_owned(), "1".to_owned()],
                    [
                        "HTTP/2.0".to_owned(),
                        "HTTP/2.0".to_owned(),
                        "HTTP/2.0".to_owned(),
                    ],
                    2,
                ),
                "with SETTINGS_MAX_CONCURRENT_STREAMS 1: the first answer's, the held one's and \
                 the one sent meanwhile's connections and protocols, the connections opened",
            )
        })
    }

    // Go: net/http/transport.go Transport.queueForDial (MaxConnsPerHost: a request that needs
    // one more connection waits for one); ruling 10 (a): at the cap, requests wait as under the
    // per-host semaphore.
    #[test]
    fn the_cap_makes_requests_wait() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 1).await?;
            let capped = HttpClientSettings {
                max_connections_per_host: 1,
                ..HTTP2_ON
            };
            let client = bidder_client(&pki, capped)?;
            let bid_url = bidder.url("https", "127.0.0.1", "/bid");
            let warm = send(&client, &bid_url, "warm").await?;
            let held = spawn_send(&client, &bidder.url("https", "127.0.0.1", "/hold"), "held");
            let _read = bidder.wait(|book| book.count("/hold") == 1).await?;
            // Polled first, the request is waiting in the pool before the moment starts.
            let (waited, meanwhile) = tokio::join!(send(&client, &bid_url, "waiting"), async {
                sleep(WAIT).await;
                let read = bidder.book().read.len();
                bidder.release(1);
                read
            });
            let waited = waited?;
            let held = held.await??;
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            check_eq(
                &(
                    meanwhile,
                    conns(&[&warm, &held, &waited]),
                    waited.events.clone(),
                    waited.wait >= WAIT,
                    book.opened,
                ),
                &(
                    2,
                    vec!["0".to_owned(), "0".to_owned(), "0".to_owned()],
                    vec!["GotConn reused=true".to_owned()],
                    true,
                    1,
                ),
                "with max_connections_per_host 1 and one stream a connection: the requests read \
                 a moment after one was sent while the stream was held; the connections; the \
                 waiting request's events and whether its wait covers the moment; the \
                 connections opened",
            )
        })
    }

    // Go: none; ruling 10 (a): a connection that gets GOAWAY leaves the pool, and its streams in
    // flight finish.
    #[test]
    fn goaway_drains_a_connection() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 10).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            let bid_url = bidder.url("https", "127.0.0.1", "/bid");
            let warm = send(&client, &bid_url, "warm").await?;
            let held = spawn_send(&client, &bidder.url("https", "127.0.0.1", "/hold"), "held");
            let _read = bidder.wait(|book| book.count("/hold") == 1).await?;
            // Answered once the connection has said GOAWAY, which so has come first.
            let goaway = send(&client, &bidder.url("https", "127.0.0.1", "/goaway"), "g").await?;
            let moved = send(&client, &bid_url, "moved").await?;
            bidder.release(1);
            let held = held.await??;
            let _ended = bidder.wait(|book| book.ended.contains(&0)).await?;
            let later = send(&client, &bid_url, "later").await?;
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            check_eq(
                &(conns(&[&warm, &goaway, &held, &moved, &later]), book.opened),
                &(
                    vec![
                        "0".to_owned(),
                        "0".to_owned(),
                        "0".to_owned(),
                        "1".to_owned(),
                        "1".to_owned(),
                    ],
                    2,
                ),
                "the connections of the first answer, the GOAWAY's request, the request held \
                 across it, and the two after; the connections opened",
            )
        })
    }

    // Go: net/http/transport.go IdleConnTimeout; ruling 10 (a): idle connections close after
    // idle_connection_timeout_seconds.
    #[test]
    fn idle_connections_close() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 10).await?;
            let idle = HttpClientSettings {
                idle_connection_timeout_seconds: 1,
                ..HTTP2_ON
            };
            let client = bidder_client(&pki, idle)?;
            let bid_url = bidder.url("https", "127.0.0.1", "/bid");
            let start = Instant::now();
            let warm = send(&client, &bid_url, "warm").await?;
            let ended = bidder.wait(|book| !book.ended.is_empty()).await?.ended;
            let idled = start.elapsed() >= Duration::from_secs(1);
            let fresh = send(&client, &bid_url, "fresh").await?;
            drop(client);
            bidder.close().await;
            check_eq(
                &(ended, idled, conns(&[&warm, &fresh]), fresh.events),
                &(
                    vec![0],
                    true,
                    vec!["0".to_owned(), "1".to_owned()],
                    vec!["GotConn reused=false".to_owned()],
                ),
                "with idle_connection_timeout_seconds 1: the connections the client closed, \
                 whether a second had passed, the connections of the request before and after, \
                 and the second one's events",
            )
        })
    }

    // Go: none (no h2c); ruling 10 (a): one probe per origin, the probe rules unchanged, and the
    // h2c origin's requests over a pool.
    #[test]
    fn h2c_origin_pooled_after_one_probe() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::cleartext(1).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            let bid_url = bidder.url("http", "127.0.0.1", "/bid");
            let probing = send(&client, &bid_url, "probing").await?;
            let _probed = bidder.wait(|book| book.probes == 1).await?;
            let held = spawn_send(&client, &bidder.url("http", "127.0.0.1", "/hold"), "held");
            let _read = bidder.wait(|book| book.count("/hold") == 1).await?;
            let second = send(&client, &bid_url, "second").await?;
            bidder.release(1);
            let held = held.await??;
            let third = send(&client, &bid_url, "third").await?;
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            let versions: Vec<String> = [&probing, &held, &second, &third]
                .iter()
                .map(|got| got.version.clone())
                .collect();
            check_eq(
                &(
                    versions,
                    conns(&[&held, &second, &third]),
                    book.http1.len(),
                    book.probes,
                    book.opened,
                ),
                &(
                    vec![
                        "HTTP/1.1".to_owned(),
                        "HTTP/2.0".to_owned(),
                        "HTTP/2.0".to_owned(),
                        "HTTP/2.0".to_owned(),
                    ],
                    vec!["0".to_owned(), "1".to_owned(), "0".to_owned()],
                    1,
                    1,
                    2,
                ),
                "an h2c origin with one stream a connection: the protocols of the request that \
                 starts the probe, the held one, the one sent meanwhile and the one after; their \
                 connections; the HTTP/1.1 requests, the probes and the h2c connections",
            )
        })
    }

    // Go: exchange/bidder.go:732-798 (GotConn's Reused, DNSStart and DNSDone, behind
    // adapter_connection_created, adapter_connection_reused and the DNS time); HTTP-H2's
    // definitions, each pooled connection counted.
    #[test]
    fn connection_metrics_count_each_pooled_connection() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 1).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            // A name, so that each new connection looks it up.
            let bid_url = bidder.url("https", "localhost", "/bid");
            let opened = send(&client, &bid_url, "opened").await?;
            let held = spawn_send(&client, &bidder.url("https", "localhost", "/hold"), "held");
            let _read = bidder.wait(|book| book.count("/hold") == 1).await?;
            let second = send(&client, &bid_url, "second").await?;
            bidder.release(1);
            let held = held.await??;
            let reused = send(&client, &bid_url, "reused").await?;
            drop(client);
            bidder.close().await;
            let created = || vec!["Dns".to_owned(), "GotConn reused=false".to_owned()];
            let reuse = || vec!["GotConn reused=true".to_owned()];
            check_eq(
                &vec![opened.events, held.events, second.events, reused.events],
                &vec![created(), reuse(), created(), reuse()],
                "with one stream a connection: the events of the request that opens the first \
                 connection, the one on it after, the one that opens the second, and a later one",
            )
        })
    }

    // Go: x/net/context/ctxhttp Do and net/http/h2_bundle.go http2clientStream.abortStream
    // (RST_STREAM CANCEL once a request's context is done); ruling 10 (a): a stream given up
    // frees its place on its pooled connection.
    #[test]
    fn cancellation_frees_pooled_streams() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 1).await?;
            let client = bidder_client(&pki, HTTP2_ON)?;
            let (bid_url, slow) = (
                bidder.url("https", "127.0.0.1", "/bid"),
                bidder.url("https", "127.0.0.1", "/slow"),
            );
            let warm = send(&client, &bid_url, "warm").await?;
            let deadline = Instant::now().checked_add(WAIT).ok_or("no deadline")?;
            let timed_out = match client
                .send(bid(&slow, "timed")?, Some(deadline), None)
                .await
            {
                Ok(response) => format!("answered {:?}", response.status()),
                Err(error) => format!("{:?} {error}", error.kind()),
            };
            let _reset = bidder.wait(|book| book.resets.len() == 1).await?;
            let after_deadline = send(&client, &bid_url, "after deadline").await?;
            let dropped = spawn_send(&client, &slow, "dropped");
            let _read = bidder.wait(|book| book.count("/slow") == 2).await?;
            dropped.abort();
            drop(dropped.await);
            let book = bidder.wait(|book| book.resets.len() == 2).await?;
            let after_drop = send(&client, &bid_url, "after drop").await?;
            let opened = bidder.book().opened;
            drop(client);
            bidder.close().await;
            check_eq(
                &(
                    timed_out,
                    book.resets,
                    conns(&[&warm, &after_deadline, &after_drop]),
                    opened,
                ),
                &(
                    "DeadlineExceeded context deadline exceeded".to_owned(),
                    vec!["/slow CANCEL".to_owned(); 2],
                    vec!["0".to_owned(), "0".to_owned(), "0".to_owned()],
                    1,
                ),
                "with one stream a connection: the deadline's error, the resets, the connections \
                 of the requests before and after, the connections opened",
            )
        })
    }

    // Go: net/http/transport.go Transport.roundTrip (a request goes again only when none of it
    // was written); ruling 10 (a): under load, a refusal and a GOAWAY, no request is read twice.
    #[test]
    fn no_request_is_read_twice() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::tls(&pki, 2).await?;
            let capped = HttpClientSettings {
                max_connections_per_host: 2,
                ..HTTP2_ON
            };
            let client = bidder_client(&pki, capped)?;
            let mut tags = Vec::new();
            let mut sent = JoinSet::new();
            for n in 0..30 {
                let path = match n % 3 {
                    0 => "/bid",
                    1 => "/wait",
                    _ => "/refuse",
                };
                let path = if n == 10 { "/goaway" } else { path };
                let tag = format!("r{n:02}");
                tags.push(tag.clone());
                let url = bidder.url("https", "127.0.0.1", path);
                let _handle = sent.spawn({
                    let client = client.clone();
                    async move { send(&client, &url, &tag).await.map(|got| got.version) }
                });
            }
            let mut versions = Vec::new();
            while let Some(joined) = sent.join_next().await {
                versions.push(joined??);
            }
            let book = bidder.book();
            drop(client);
            bidder.close().await;
            // Each request read once, but those refused unread, read again once.
            let reread: Vec<(String, usize)> = tags
                .iter()
                .filter_map(|tag| {
                    let reads = book.read.iter().filter(|(_, _, read)| read == tag).count();
                    let expected = if book.refused.contains(tag) { 2 } else { 1 };
                    (reads != expected).then(|| (tag.clone(), reads))
                })
                .collect();
            check_eq(
                &(reread, book.refused.len()),
                &(Vec::new(), 10),
                "the requests read otherwise than once, or once more after the refusal of their \
                 stream unread, and the streams refused",
            )?;
            let mut answered = book.answered;
            answered.sort();
            check_eq(
                &(versions.len(), versions.iter().all(|v| v == "HTTP/2.0")),
                &(30, true),
                "every request answered over HTTP/2",
            )?;
            check_eq(
                &answered,
                &tags,
                "the requests the bidder answered: each once (refused streams and those a \
                 GOAWAY left out went again)",
            )
        })
    }

    /// What a client got of a response: its status, `Content-Encoding`, `Content-Length`, body
    /// and read error.
    type Decoded = (u16, Vec<String>, Vec<String>, Vec<u8>, String);

    /// A case of [`pooled_bodies_read_as_reqwest_h2`]: its name, its script and the fields the
    /// caller sets.
    type Case = (String, Script, Vec<(String, String)>);

    /// One response a scripted server writes: its status, its fields, and what goes on its
    /// stream after the head.
    #[derive(Debug, Clone)]
    struct Script {
        status: StatusCode,
        fields: Vec<(String, String)>,
        frames: Vec<Frame>,
    }

    /// What a scripted server writes on a response's stream.
    #[derive(Debug, Clone)]
    enum Frame {
        /// A DATA frame, the last when `end`.
        Data(Vec<u8>, bool),
        /// `RST_STREAM` with the reason, a moment after the frames before it.
        Reset(Reason),
    }

    /// The fields of `doc`'s case `case` the caller sets.
    fn caller_fields(case: &serde_json::Value) -> Result<Vec<(String, String)>, BoxError> {
        let mut fields = Vec::new();
        for op in list(case, "/headers")? {
            fields.push((
                text(op, "/name")?.to_owned(),
                text(op, "/value")?.to_owned(),
            ));
        }
        Ok(fields)
    }

    /// The script of compression.json's `case`: its head and body.
    fn golden_script(case: &serde_json::Value) -> Result<Script, BoxError> {
        let head = strings(case, "/response_head")?;
        let body = unhex(text(case, "/response_body_hex")?)?;
        let status = head
            .first()
            .and_then(|line| line.split(' ').nth(1))
            .ok_or("no status line")?
            .parse::<u16>()?;
        let fields = head
            .iter()
            .skip(1)
            .filter_map(|line| line.split_once(": "))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let frames = if body.is_empty() {
            Vec::new()
        } else {
            vec![Frame::Data(body, true)]
        };
        Ok(Script {
            status: StatusCode::from_u16(status)?,
            fields,
            frames,
        })
    }

    /// Writes `script` as the answer on `respond`.
    async fn play(script: Script, mut respond: SendResponse<Bytes>) {
        let mut head = http::Response::new(());
        *head.status_mut() = script.status;
        for (name, value) in &script.fields {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                let _appended: bool = head.headers_mut().append(name, value);
            }
        }
        let Ok(mut stream) = respond.send_response(head, script.frames.is_empty()) else {
            return;
        };
        for frame in script.frames {
            match frame {
                Frame::Data(data, end) => {
                    let _sent = stream.send_data(Bytes::from(data), end);
                }
                Frame::Reset(reason) => {
                    sleep(Duration::from_millis(50)).await;
                    stream.send_reset(reason);
                }
            }
        }
    }

    /// A server that speaks h2, over TLS (ALPN `h2` only) with `pki` or with prior knowledge in
    /// cleartext, and answers every request with `script`.
    async fn scripted(pki: Option<&TestPki>, script: Script) -> Result<RawServer, BoxError> {
        let serve = move |conn: RawConn| {
            let script = script.clone();
            async move {
                let handshake = ServerBuilder::new()
                    .handshake::<_, Bytes>(conn.into_stream())
                    .await;
                let Ok(mut connection) = handshake else {
                    return;
                };
                while let Some(Ok((_request, respond))) = connection.accept().await {
                    drop(tokio::spawn(play(script.clone(), respond)));
                }
            }
        };
        match pki {
            Some(pki) => RawServer::start_tls(pki.acceptor(&[b"h2"])?, serve).await,
            None => RawServer::start(serve).await,
        }
    }

    /// `error`'s text and its sources', as the crate words a reqwest error.
    fn chain(error: &dyn Error) -> String {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(next) = source {
            text.push_str(": ");
            text.push_str(&next.to_string());
            source = next.source();
        }
        text
    }

    /// What the pooled client gets of a GET to `url` with `fields`.
    async fn over_pool(
        client: &Client,
        url: &str,
        fields: &[(String, String)],
    ) -> Result<Decoded, BoxError> {
        let mut get = request("GET", url)?;
        for (name, value) in fields {
            let _appended: bool = get.headers_mut().append(
                HeaderName::from_bytes(name.as_bytes())?,
                HeaderValue::from_str(value)?,
            );
        }
        let got = fetch(client.send(get, None, None).await?).await;
        let header = |name: &str| -> Vec<String> {
            got.headers
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect()
        };
        let (encoding, length) = (header("content-encoding"), header("content-length"));
        let (body, error) = match got.body {
            Ok(body) => (body, String::new()),
            Err(error) => (Vec::new(), error.to_string()),
        };
        Ok((got.status.as_u16(), encoding, length, body, error))
    }

    /// What reqwest's own HTTP/2 client, with prior knowledge and gzip as the crate builds it,
    /// gets of a GET to `url` with `fields`, read by parts as the crate reads reqwest's.
    async fn over_reqwest(url: &str, fields: &[(String, String)]) -> Result<Decoded, BoxError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .http2_prior_knowledge()
            .build()?;
        let mut get = client.get(url);
        for (name, value) in fields {
            get = get.header(name.as_str(), value.as_str());
        }
        let mut response = get.send().await?;
        let header = |name: &str| -> Vec<String> {
            response
                .headers()
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect()
        };
        let (encoding, length) = (header("content-encoding"), header("content-length"));
        let status = response.status().as_u16();
        let mut body = Vec::new();
        let error = loop {
            match response.chunk().await {
                Ok(Some(part)) => body.extend_from_slice(&part),
                Ok(None) => break String::new(),
                Err(error) => break chain(&error),
            }
        };
        if !error.is_empty() {
            body.clear();
        }
        Ok((status, encoding, length, body, error))
    }

    // Go: none (HTTP-H2's requirement: with HTTP/2 on, what a caller gets of a response is what
    // reqwest's HTTP/2 gave it, whose differences from Go's are `http-client.accept-encoding`,
    // `http-client.gzip-edges` and `http-client.error-texts`): the responses of
    // testdata/golden/compression.json, and streams that reset, end early or carry more after a
    // gzip member.
    #[test]
    fn pooled_bodies_read_as_reqwest_h2() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let doc = golden("compression.json")?;
            let mut cases = Vec::new();
            for case in list(&doc, "/cases")? {
                cases.push((
                    text(case, "/name")?.to_owned(),
                    golden_script(case)?,
                    caller_fields(case)?,
                ));
            }
            cases.extend(stream_cases()?);
            let mut differ = Vec::new();
            let mut seen: Vec<(String, (String, String))> = Vec::new();
            for (name, script, fields) in cases {
                let tls = scripted(Some(&pki), script.clone()).await?;
                let pooled = bidder_client(&pki, HTTP2_ON)?;
                let tls_url = format!("https://127.0.0.1:{}/c", tls.port());
                let on_pool = over_pool(&pooled, &tls_url, &fields).await?;
                drop(pooled);
                let _tls_counts = tls.close().await;
                let cleartext = scripted(None, script).await?;
                let cleartext_url = format!("http://127.0.0.1:{}/c", cleartext.port());
                let on_reqwest = over_reqwest(&cleartext_url, &fields).await?;
                let _cleartext_counts = cleartext.close().await;
                seen.push((
                    name.clone(),
                    (
                        on_pool.4.clone(),
                        String::from_utf8_lossy(&on_pool.3).into_owned(),
                    ),
                ));
                if on_pool != on_reqwest {
                    differ.push((name, on_reqwest, on_pool));
                }
            }
            check_eq(
                &differ,
                &Vec::new(),
                "the responses a pooled connection gives the caller otherwise than reqwest's \
                 HTTP/2 (case, reqwest, the pool)",
            )?;
            check_streams(&seen)
        })
    }

    /// The cases of streams that reset, end early or carry more after a gzip member: their
    /// names, scripts and the fields the caller sets (none).
    fn stream_cases() -> Result<Vec<Case>, BoxError> {
        let member = unhex(
            "1f8b08000000000000ffaa56ca4c51b2522a52d2512a4e4d2c4902f1a2636b01010000ffff016e18\
             0217000000",
        )?;
        let part = member.get(..20).ok_or("short")?.to_vec();
        let gzip = vec![("Content-Encoding".to_owned(), "gzip".to_owned())];
        let streams = [
            (
                "reset_mid_body",
                Vec::new(),
                vec![
                    Frame::Data(b"par".to_vec(), false),
                    Frame::Reset(Reason::INTERNAL_ERROR),
                ],
            ),
            (
                "no_error_reset",
                Vec::new(),
                vec![
                    Frame::Data(b"ok".to_vec(), false),
                    Frame::Reset(Reason::NO_ERROR),
                ],
            ),
            (
                "gzip_then_more_data",
                gzip.clone(),
                vec![
                    Frame::Data(member.clone(), false),
                    Frame::Data(b"x".to_vec(), true),
                ],
            ),
            (
                "gzip_members_in_one_frame",
                gzip.clone(),
                vec![Frame::Data([member.clone(), member].concat(), true)],
            ),
            (
                "gzip_reset",
                gzip.clone(),
                vec![
                    Frame::Data(part.clone(), false),
                    Frame::Reset(Reason::INTERNAL_ERROR),
                ],
            ),
            ("gzip_cut_short", gzip, vec![Frame::Data(part, true)]),
        ];
        Ok(streams
            .into_iter()
            .map(|(name, fields, frames)| {
                let script = Script {
                    status: StatusCode::OK,
                    fields,
                    frames,
                };
                (name.to_owned(), script, Vec::new())
            })
            .collect())
    }

    /// Checks what the pool gave of [`stream_cases`], the last of `seen`: each case's read error
    /// and body.
    fn check_streams(seen: &[(String, (String, String))]) -> TestResult {
        let reset = "error decoding response body: error reading a body from connection: stream \
                     error received: unexpected internal error encountered";
        let payload = r#"{"id":"r","seatbid":[]}"#;
        let got = |error: &str, body: &str| (error.to_owned(), body.to_owned());
        check_eq(
            &seen
                .get(seen.len().saturating_sub(6)..)
                .unwrap_or_default()
                .to_vec(),
            &vec![
                ("reset_mid_body".to_owned(), got(reset, "")),
                // hyper: a reset with NO_ERROR ends the body early.
                ("no_error_reset".to_owned(), got("", "ok")),
                // tower-http: data after the gzip member is an error.
                (
                    "gzip_then_more_data".to_owned(),
                    got(
                        "error decoding response body: there are extra bytes after body has been \
                         decompressed",
                        "",
                    ),
                ),
                // tower-http decodes one member: the rest of its frame is dropped.
                ("gzip_members_in_one_frame".to_owned(), got("", payload)),
                ("gzip_reset".to_owned(), got(reset, "")),
                (
                    "gzip_cut_short".to_owned(),
                    got("error decoding response body: unexpected end of file", ""),
                ),
            ],
            "the streams' read errors and bodies over the pool",
        )
    }

    /// What a request to `url`, sent with `client` under a ten-second deadline, failed with: its
    /// kind, whether it is a refused connection or a name that does not exist (bidder.go's
    /// unreachable bidder), and its text.
    async fn failure(client: &Client, url: &str) -> Result<String, BoxError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("no deadline")?;
        Ok(
            match client.send(bid(url, "dial")?, Some(deadline), None).await {
                Ok(response) => format!("answered {}", response.status()),
                Err(error) => format!(
                    "{:?} refused={} not_found={}: {error}",
                    error.kind(),
                    error.is_connection_refused(),
                    error.is_dns_not_found()
                ),
            },
        )
    }

    // Go: none (HTTP-H2's requirement: with HTTP/2 on, a failed connect reads as reqwest's, the
    // texts `http-client.error-texts` ledgers); ruling 10 (a): the pool's dial fails as reqwest's
    // connector does, for an untrusted certificate, a refused connection, a name that does not
    // exist and the connect timeout.
    #[test]
    fn dial_errors_read_as_reqwest() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            // The clients trust no test root, so the bidder's certificate is untrusted.
            let alpn: [&[u8]; 2] = [b"h2", b"http/1.1"];
            let untrusted = RawServer::start_tls(pki.acceptor(&alpn)?, |mut conn| async move {
                conn.drain().await;
            })
            .await?;
            // Takes the connection and never answers TLS's hello.
            let silent = RawServer::start(|mut conn: RawConn| async move {
                conn.drain().await;
            })
            .await?;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let closed = listener.local_addr()?.port();
            drop(listener);
            let quick = HttpClientSettings {
                dialer: DialerSettings {
                    timeout_seconds: 1,
                    ..HTTP2_ON.dialer
                },
                ..HTTP2_ON
            };
            let build = |http2| {
                Builder::new(HttpClientSettings { http2, ..quick }, ProxyEnv::default())
                    .loopback_only(0)
                    .negotiate_http2()
                    .build()
            };
            let (pooled, reqwest) = (build(Http2Settings::ON)?, build(Http2Settings::OFF)?);
            let mut differ = Vec::new();
            for url in [
                format!("https://127.0.0.1:{}/bid", untrusted.port()),
                format!("https://localhost:{}/bid", untrusted.port()),
                format!("https://127.0.0.1:{closed}/bid"),
                format!("https://bidder.invalid:{closed}/bid"),
                format!("https://127.0.0.1:{}/bid", silent.port()),
            ] {
                let (over_pool, over_reqwest) = (
                    failure(&pooled, &url).await?,
                    failure(&reqwest, &url).await?,
                );
                if over_pool != over_reqwest {
                    differ.push((url, over_pool, over_reqwest));
                }
            }
            drop((pooled, reqwest));
            let _untrusted = untrusted.close().await;
            let _silent = silent.close().await;
            check_eq(
                &differ,
                &Vec::new(),
                "the dials that fail otherwise over the pool than over reqwest: the URL, then the \
                 kind, the unreachable tests and the text of each (pool, reqwest)",
            )
        })
    }

    // Go: net/http/transport.go Transport.queueForDial (MaxConnsPerHost counts connections);
    // ruling 10 (a) with `http-client.h2c-demotion`: an h2c origin moved back to HTTP/1.1 takes
    // its pooled connections out of the pool, which close once their streams end and give their
    // max_connections_per_host slots to its HTTP/1.1 requests.
    #[test]
    fn demotion_frees_the_slots() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let bidder = Bidder::cleartext(1).await?;
            let capped = HttpClientSettings {
                max_connections_per_host: 1,
                ..HTTP2_ON
            };
            let client = bidder_client(&pki, capped)?;
            let bid_url = bidder.url("http", "127.0.0.1", "/bid");
            let probing = send(&client, &bid_url, "probing").await?;
            let _probed = bidder.wait(|book| book.probes == 1).await?;
            let pooled = send(&client, &bid_url, "pooled").await?;
            // The bidder resets a stream it read: the origin goes back to HTTP/1.1.
            let reset = client
                .send(
                    bid(&bidder.url("http", "127.0.0.1", "/reset"), "reset")?,
                    None,
                    None,
                )
                .await
                .map(|response| response.status());
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or("no deadline")?;
            let start = Instant::now();
            let after = client
                .send(bid(&bid_url, "after")?, Some(deadline), None)
                .await
                .map(|response| format!("{:?}", response.version()));
            let prompt = start.elapsed() < Duration::from_secs(1);
            let ended = bidder.wait(|book| book.ended == vec![0]).await?.ended;
            drop(client);
            bidder.close().await;
            check_eq(
                &(
                    probing.version,
                    pooled.version,
                    reset.is_err(),
                    after.map_err(|e| e.to_string()),
                    prompt,
                    ended,
                ),
                &(
                    "HTTP/1.1".to_owned(),
                    "HTTP/2.0".to_owned(),
                    true,
                    Ok("HTTP/1.1".to_owned()),
                    true,
                    vec![0],
                ),
                "with max_connections_per_host 1: the protocols before and after the probe, \
                 whether the reset stream failed, the protocol of the request after it, whether \
                 it went within a second (not waiting out the idle connection) and the h2c \
                 connections closed",
            )
        })
    }

    // Go: none (HTTP-H2's requirement: with HTTP/2 on, a response reads as reqwest's gave it);
    // ruling 10 (a): reqwest's response URL is the last request's target, without the URL's
    // fragment.
    #[test]
    fn pooled_response_url_drops_the_fragment() -> TestResult {
        run(async {
            let pki = TestPki::new()?;
            let alpn: [&[u8]; 2] = [b"h2", b"http/1.1"];
            let bidder = Bidder::tls_offering(&pki, &alpn, 4).await?;
            let url = bidder.url("https", "127.0.0.1", "/bid#part");
            let mut urls = Vec::new();
            for http2 in [Http2Settings::ON, Http2Settings::OFF] {
                let client = bidder_client(&pki, HttpClientSettings { http2, ..HTTP2_ON })?;
                let response = client.send(bid(&url, "fragment")?, None, None).await?;
                urls.push((
                    format!("{:?}", response.version()),
                    response.url().to_string(),
                ));
            }
            bidder.close().await;
            let target = bidder_target(&url)?;
            check_eq(
                &urls,
                &vec![
                    ("HTTP/2.0".to_owned(), target.clone()),
                    ("HTTP/1.1".to_owned(), target),
                ],
                "the protocol and the response URL over the pool and over reqwest",
            )
        })
    }

    /// `url` without its fragment.
    fn bidder_target(url: &str) -> Result<String, BoxError> {
        let mut target = Url::parse(url)?;
        target.set_fragment(None);
        Ok(target.to_string())
    }
}
