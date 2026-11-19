//! Pins of the clients' errors against Go's (`testdata/golden/errors.json`,
//! `testdata/go/errors.go`): which ones are deadlines, the test bidder.go applies before it
//! reports `errortypes.Timeout` (restored), and their texts (ledgered).

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

    /// How one case failed: where, the error's kind and text, and whether it is a deadline.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Failure {
        stage: String,
        kind: Option<ErrorKind>,
        error: String,
        timeout: bool,
    }

    /// The server each case of the golden runs against, as `errorsGolden` sets them up.
    async fn server_for(name: &str) -> Result<Option<RawServer>, BoxError> {
        Ok(Some(match name {
            "deadline_awaiting_headers" | "cgi_http_proxy" => {
                RawServer::start(|mut conn| async move { conn.drain().await }).await?
            }
            "deadline_reading_body" => {
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
            "connection_refused" => return Ok(None),
            other => return Err(format!("no server for {other}").into()),
        }))
    }

    /// A port nothing listens on.
    async fn closed_port() -> Result<u16, BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Ok(listener.local_addr()?.port())
    }

    /// Runs `case` with the general client as bidder.go runs a request: send under the deadline,
    /// then read the body.
    async fn rust_failure(case: &Value) -> Result<Failure, BoxError> {
        let name = text(case, "/name")?;
        let server = server_for(name).await?;
        let port = match &server {
            Some(server) => server.port(),
            None => closed_port().await?,
        };
        let proxy = if name == "cgi_http_proxy" {
            ProxyEnv {
                http_proxy: "http://127.0.0.1:9".to_owned(),
                cgi: true,
                ..ProxyEnv::default()
            }
        } else {
            ProxyEnv::default()
        };
        let client = Builder::new(HttpClientSettings::GENERAL, proxy)
            .loopback_only(0)
            .build()?;
        let ports = [(port, "PORT")];
        let method = text(case, "/method")?;
        let mut req = request(method, &ports_in(text(case, "/url")?, &ports))?;
        if req.method() == Method::POST {
            *req.body_mut() = Some(Body::from(r#"{"id":"r"}"#));
        }
        let deadline =
            Instant::now() + Duration::from_millis(u64::try_from(int(case, "/deadline_ms")?)?);
        let failed = |stage: &str, e: &RequestError| Failure {
            stage: stage.to_owned(),
            kind: Some(e.kind()),
            error: with_ports(&e.to_string(), &ports),
            timeout: e.is_timeout(),
        };
        let failure = match client.send(req, Some(deadline), None).await {
            Err(e) => failed("send", &e),
            Ok(response) => match response.bytes().await {
                Err(e) => failed("body", &e),
                Ok(_) => Failure {
                    stage: "body".to_owned(),
                    kind: None,
                    error: String::new(),
                    timeout: false,
                },
            },
        };
        drop(client);
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
                timeout: case
                    .get("timeout")
                    .and_then(Value::as_bool)
                    .ok_or("no timeout")?,
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
    // context.DeadlineExceeded) for errortypes.Timeout).
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
                    check_eq(
                        &(rust.kind, rust.error.as_str()),
                        &(
                            Some(ErrorKind::DeadlineExceeded),
                            "context deadline exceeded",
                        ),
                        &format!("{name}: a deadline reads as ctxhttp's"),
                    )?;
                }
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
                ],
                "the cases whose text differs from Go's",
            )
        })
    }
}
