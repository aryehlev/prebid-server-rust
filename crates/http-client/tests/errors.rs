//! Pins of the clients' errors against Go's (`testdata/golden/errors.json`,
//! `testdata/go/errors.go`): which ones are deadlines, the test bidder.go applies before it
//! reports `errortypes.Timeout`, with the texts of deadlines and of Go's `Client.Timeout`
//! (restored); which ones are unreachable bidders, seat non-bid code 103 (restored); and the
//! other texts (ledgered).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pbs_http_client::{
        Builder, ErrorKind, HttpClientSettings, ProxyEnv, RequestError,
        reqwest::{Body, Method},
    };
    use serde_json::Value;
    use tokio::{net::TcpListener, time::Instant};

    use crate::support::{
        BoxError, RawServer, TestResult, check_eq, golden, int, list, ports_in, request, run, text,
        with_ports,
    };

    /// How one case failed: where, the error's kind, its texts as ctxhttp and `client.Do` give
    /// them, whether it is a deadline and whether the bidder is unreachable.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Failure {
        stage: String,
        kind: Option<ErrorKind>,
        error: String,
        do_error: String,
        timeout: bool,
        unreachable: bool,
    }

    /// The server each case of the golden runs against, as `errorsGolden` sets them up; `closed`
    /// is the port nothing listens on.
    async fn server_for(name: &str, closed: u16) -> Result<Option<RawServer>, BoxError> {
        Ok(Some(match name {
            "deadline_awaiting_headers"
            | "cgi_http_proxy"
            | "client_timeout_awaiting_headers"
            | "deadline_before_client_timeout" => {
                RawServer::start(|mut conn| async move { conn.drain().await }).await?
            }
            "deadline_reading_body" | "client_timeout_reading_body" => {
                RawServer::start(|mut conn| async move {
                    if conn.read_request().await.is_some() {
                        conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789")
                            .await;
                        conn.drain().await;
                    }
                })
                .await?
            }
            "server_closes" => {
                RawServer::start(|mut conn| async move {
                    let _request = conn.read_request().await;
                })
                .await?
            }
            "redirect_then_refused" => {
                let redirect = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\n\
                     Location: http://127.0.0.1:{closed}/next\r\nContent-Length: 0\r\n\r\n"
                );
                RawServer::start(move |mut conn| {
                    let redirect = redirect.clone();
                    async move {
                        while conn.read_request().await.is_some() {
                            conn.write(redirect.as_bytes()).await;
                        }
                    }
                })
                .await?
            }
            "connection_refused" | "dns_not_found" | "empty_path_refused" => return Ok(None),
            other => return Err(format!("no server for {other}").into()),
        }))
    }

    /// A port nothing listens on.
    async fn closed_port() -> Result<u16, BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Ok(listener.local_addr()?.port())
    }

    /// Runs `case` with the general client as bidder.go runs a request: send under the deadline,
    /// then read the body; with the case's `Client.Timeout` as scope3's client has one.
    async fn rust_failure(case: &Value) -> Result<Failure, BoxError> {
        let name = text(case, "/name")?;
        let closed = closed_port().await?;
        let server = server_for(name, closed).await?;
        let port = server.as_ref().map_or(closed, RawServer::port);
        let proxy = if name == "cgi_http_proxy" {
            ProxyEnv {
                http_proxy: "http://127.0.0.1:9".to_owned(),
                cgi: true,
                ..ProxyEnv::default()
            }
        } else {
            ProxyEnv::default()
        };
        let general = Builder::new(HttpClientSettings::GENERAL, proxy)
            .loopback_only(0)
            .build()?;
        let client = general.with_timeout(Duration::from_millis(u64::try_from(int(
            case,
            "/client_timeout_ms",
        )?)?));
        let ports = [(port, "PORT"), (closed, "CLOSED")];
        let method = text(case, "/method")?;
        let mut req = request(method, &ports_in(text(case, "/url")?, &ports))?;
        if req.method() == Method::POST {
            *req.body_mut() = Some(Body::from(r#"{"id":"r"}"#));
        }
        let deadline = match u64::try_from(int(case, "/deadline_ms")?)? {
            0 => None,
            ms => Some(Instant::now() + Duration::from_millis(ms)),
        };
        let failed = |stage: &str, e: &RequestError| Failure {
            stage: stage.to_owned(),
            kind: Some(e.kind()),
            error: with_ports(&e.to_string(), &ports),
            do_error: with_ports(&e.do_text(), &ports),
            timeout: e.is_timeout(),
            unreachable: e.is_connection_refused() || e.is_dns_not_found(),
        };
        let failure = match client.send(req, deadline, None).await {
            Err(e) => failed("send", &e),
            Ok(response) => match response.bytes().await {
                Err(e) => failed("body", &e),
                Ok(_) => Failure {
                    stage: "body".to_owned(),
                    kind: None,
                    error: String::new(),
                    do_error: String::new(),
                    timeout: false,
                    unreachable: false,
                },
            },
        };
        drop((client, general));
        if let Some(server) = server {
            let _counts = server.close().await;
        }
        Ok(failure)
    }

    /// Every case's name, Go's failure (its kind unknown) and the general client's.
    async fn all_failures() -> Result<Vec<(String, Failure, Failure)>, BoxError> {
        let doc = golden("errors.json")?;
        let mut out = Vec::new();
        for case in list(&doc, "/cases")? {
            let go = Failure {
                stage: text(case, "/stage")?.to_owned(),
                kind: None,
                error: text(case, "/error")?.to_owned(),
                do_error: text(case, "/do_error")?.to_owned(),
                timeout: case
                    .get("timeout")
                    .and_then(Value::as_bool)
                    .ok_or("no timeout")?,
                unreachable: case
                    .get("unreachable")
                    .and_then(Value::as_bool)
                    .ok_or("no unreachable")?,
            };
            out.push((
                text(case, "/name")?.to_owned(),
                go,
                rust_failure(case).await?,
            ));
        }
        Ok(out)
    }

    // Go: exchange/bidder.go:616-620 (ctxhttp.Do, then errors.Is(err,
    // context.DeadlineExceeded) for errortypes.Timeout), net/http/client.go:733-738 and
    // :990-996 (Client.Timeout's texts) and floors/fetcher.go:270 (client.Do's text).
    #[test]
    fn deadline_is_timeout() -> TestResult {
        run(async {
            for (name, go, rust) in all_failures().await? {
                check_eq(
                    &(&rust.stage, rust.timeout),
                    &(&go.stage, go.timeout),
                    &format!("{name}: the stage and whether it is a deadline"),
                )?;
                if go.timeout {
                    let kind = if go.do_error.contains("(Client.Timeout") {
                        ErrorKind::ClientTimeout
                    } else {
                        ErrorKind::DeadlineExceeded
                    };
                    check_eq(
                        &(rust.kind, rust.error.as_str(), rust.do_error.as_str()),
                        &(Some(kind), go.error.as_str(), go.do_error.as_str()),
                        &format!("{name}: a deadline reads as ctxhttp's and client.Do's"),
                    )?;
                }
            }
            Ok(())
        })
    }

    // Go: exchange/non_bid_reason.go:53-57 (isBidderUnreachableError: errors.Is(err,
    // syscall.ECONNREFUSED), or errors.As a *net.DNSError with IsNotFound).
    #[test]
    fn unreachable_like_go() -> TestResult {
        run(async {
            for (name, go, rust) in all_failures().await? {
                check_eq(
                    &rust.unreachable,
                    &go.unreachable,
                    &format!("{name}: whether the bidder is unreachable"),
                )?;
            }
            Ok(())
        })
    }

    // Go: net/http/client.go Client.do (url.Error's `Op "URL": Err`) and net/net.go OpError.
    #[test]
    fn texts_differ_kind_same() -> TestResult {
        run(async {
            let mut differ = Vec::new();
            for (name, go, rust) in all_failures().await? {
                check_eq(
                    &rust.timeout,
                    &go.timeout,
                    &format!("{name}: the classification"),
                )?;
                if rust.error != go.error {
                    // The os error number is the platform's; the text before it is not.
                    let text = rust
                        .error
                        .split(" (os error ")
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    differ.push((name, rust.kind, text));
                }
            }
            check_eq(
                &differ,
                &vec![
                    // Go: `Post "http://127.0.0.1:PORT/e": EOF`.
                    (
                        "server_closes".to_owned(),
                        Some(ErrorKind::Request),
                        concat!(
                            r#"Post "http://127.0.0.1:PORT/e": error sending request: "#,
                            "client error (SendRequest): connection closed before message ",
                            "completed",
                        )
                        .to_owned(),
                    ),
                    // Go: `Post "http://127.0.0.1:PORT/e": dial tcp 127.0.0.1:PORT: connect:
                    // connection refused`.
                    (
                        "connection_refused".to_owned(),
                        Some(ErrorKind::Connect),
                        concat!(
                            r#"Post "http://127.0.0.1:PORT/e": error sending request: "#,
                            "client error (Connect): tcp connect error: Connection refused",
                        )
                        .to_owned(),
                    ),
                    // Go: `Post "http://nosuch.invalid:PORT/e": dial tcp: lookup nosuch.invalid:
                    // no such host`.
                    (
                        "dns_not_found".to_owned(),
                        Some(ErrorKind::Connect),
                        concat!(
                            r#"Post "http://nosuch.invalid:PORT/e": error sending request: "#,
                            "client error (Connect): dns error: error resolving DNS: no such host",
                        )
                        .to_owned(),
                    ),
                    // Go: `Post "http://127.0.0.1:PORT?x=1": dial tcp 127.0.0.1:PORT: connect:
                    // connection refused`; the url crate writes the empty path as `/`.
                    (
                        "empty_path_refused".to_owned(),
                        Some(ErrorKind::Connect),
                        concat!(
                            r#"Post "http://127.0.0.1:PORT/?x=1": error sending request: "#,
                            "client error (Connect): tcp connect error: Connection refused",
                        )
                        .to_owned(),
                    ),
                    // Go: `Post "http://127.0.0.1:CLOSED/next": dial tcp 127.0.0.1:CLOSED:
                    // connect: connection refused`, naming the hop that failed; reqwest's error
                    // keeps the first request's URL.
                    (
                        "redirect_then_refused".to_owned(),
                        Some(ErrorKind::Connect),
                        concat!(
                            r#"Post "http://127.0.0.1:PORT/e": error sending request: "#,
                            "client error (Connect): tcp connect error: Connection refused",
                        )
                        .to_owned(),
                    ),
                ],
                "the cases whose text differs from Go's",
            )
        })
    }
}
