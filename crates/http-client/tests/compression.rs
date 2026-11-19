//! Pins of the general client's compression against Go's Transport
//! (`testdata/golden/compression.json`, `testdata/go/compression.go`): when `Accept-Encoding:
//! gzip` goes out, and what the caller then gets of a compressed response.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_http_client::{
        HttpClientSettings,
        reqwest::header::{HeaderName, HeaderValue},
    };
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::support::{
        BoxError, RawServer, TestResult, check_eq, fetch, golden, int, list, loopback_client,
        request, run, strings, text, unhex,
    };

    /// What one exchange shows: the `Accept-Encoding` the server read, and what the caller got.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Outcome {
        accept_encoding: String,
        status: i64,
        content_encoding: Vec<String>,
        content_length: Vec<String>,
        body: Vec<u8>,
        read_error: String,
    }

    /// Go's outcome as the golden records it.
    fn go_outcome(case: &Value) -> Result<Outcome, BoxError> {
        let line = text(case, "/accept_encoding")?;
        Ok(Outcome {
            accept_encoding: line
                .split_once(": ")
                .map_or_else(String::new, |(_, value)| value.to_owned()),
            status: int(case, "/status")?,
            content_encoding: strings(case, "/content_encoding")?,
            content_length: strings(case, "/content_length")?,
            body: unhex(text(case, "/body_hex")?)?,
            read_error: text(case, "/read_error")?.to_owned(),
        })
    }

    /// Serves `case`'s response to the general client and returns what it got.
    async fn rust_outcome(case: &Value) -> Result<Outcome, BoxError> {
        let head = strings(case, "/response_head")?.join("\r\n");
        let mut response = format!("{head}\r\nConnection: close\r\n\r\n").into_bytes();
        response.extend(unhex(text(case, "/response_body_hex")?)?);
        let (sender, mut seen) = mpsc::unbounded_channel();
        let server = RawServer::start(move |mut conn| {
            let sender = sender.clone();
            let answer = response.clone();
            async move {
                if let Some(request) = conn.read_request().await {
                    let accept_encoding = request
                        .head
                        .iter()
                        .filter_map(|line| line.split_once(": "))
                        .filter(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"))
                        .map(|(_, value)| value.to_owned())
                        .next_back()
                        .unwrap_or_default();
                    let _sent = sender.send(accept_encoding);
                    conn.write(&answer).await;
                }
            }
        })
        .await?;
        let client = loopback_client(HttpClientSettings::GENERAL, 0)?;
        let mut req = request("GET", &format!("http://127.0.0.1:{}/c", server.port()))?;
        for op in list(case, "/headers")? {
            let name = HeaderName::from_bytes(text(op, "/name")?.as_bytes())?;
            let value = HeaderValue::from_str(text(op, "/value")?)?;
            let _appended: bool = req.headers_mut().append(name, value);
        }
        let fetched = fetch(client.send(req, None, None).await?).await;
        let header = |name: &str| -> Vec<String> {
            fetched
                .headers
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect()
        };
        let content_encoding = header("content-encoding");
        let content_length = header("content-length");
        let status = i64::from(fetched.status.as_u16());
        let (body, read_error) = match fetched.body {
            Ok(body) => (body, String::new()),
            Err(e) => (Vec::new(), e.to_string()),
        };
        let accept_encoding = seen.recv().await.ok_or("the server read no request")?;
        drop(client);
        let _counts = server.close().await;
        Ok(Outcome {
            accept_encoding,
            status,
            content_encoding,
            content_length,
            body,
            read_error,
        })
    }

    /// Every case's name, Go's outcome and the general client's.
    async fn all_outcomes() -> Result<Vec<(String, Outcome, Outcome)>, BoxError> {
        let doc = golden("compression.json")?;
        let mut out = Vec::new();
        for case in list(&doc, "/cases")? {
            out.push((
                text(case, "/name")?.to_owned(),
                go_outcome(case)?,
                rust_outcome(case).await?,
            ));
        }
        Ok(out)
    }

    /// The cases whose outcome differs, each with the general client's outcome.
    async fn differing() -> Result<Vec<(String, Outcome)>, BoxError> {
        Ok(all_outcomes()
            .await?
            .into_iter()
            .filter(|(_, go, rust)| go != rust)
            .map(|(name, _, rust)| (name, rust))
            .collect())
    }

    /// The body every gzip case of the golden decompresses to.
    const PAYLOAD: &[u8] = br#"{"id":"r","seatbid":[]}"#;

    /// A 200 with `body` and neither Content-Encoding nor Content-Length, after `accept_encoding`.
    fn plain(accept_encoding: &str, body: &[u8]) -> Outcome {
        Outcome {
            accept_encoding: accept_encoding.to_owned(),
            status: 200,
            content_encoding: Vec::new(),
            content_length: Vec::new(),
            body: body.to_vec(),
            read_error: String::new(),
        }
    }

    /// The cases http-client.accept-encoding covers.
    const CALLER_CASES: [&str; 2] = ["caller_accept_encoding", "caller_range"];

    // Go: net/http/transport.go persistConn.roundTrip (requestedGzip: no Accept-Encoding and no
    // Range on the request) and persistConn.readLoop (the gzipReader).
    #[test]
    fn caller_accept_encoding_body_decompressed() -> TestResult {
        run(async {
            let outcomes = all_outcomes().await?;
            let caller: Vec<(String, Outcome, Outcome)> = outcomes
                .into_iter()
                .filter(|(name, ..)| CALLER_CASES.contains(&name.as_str()))
                .collect();
            for (name, go, _) in &caller {
                check_eq(
                    &(&go.content_encoding, &go.content_length),
                    &(&vec!["gzip".to_owned()], &vec!["45".to_owned()]),
                    &format!("{name}: Go leaves the body compressed"),
                )?;
            }
            let rust: Vec<(String, Outcome)> = caller
                .into_iter()
                .map(|(name, _, rust)| (name, rust))
                .collect();
            check_eq(
                &rust,
                &vec![
                    // The caller's own Accept-Encoding goes out, and the body comes back
                    // decompressed.
                    (
                        "caller_accept_encoding".to_owned(),
                        plain("gzip, deflate", PAYLOAD),
                    ),
                    // reqwest asks for gzip despite the Range, where Go asks for nothing.
                    ("caller_range".to_owned(), plain("gzip", PAYLOAD)),
                ],
                "what a caller that set Accept-Encoding or Range gets",
            )
        })
    }

    // Go: net/http/transport.go persistConn.readLoop (EqualFold on Content-Encoding, no gzipReader
    // for a response without a body) and compress/gzip Reader (the header read).
    #[test]
    fn gzip_edges_differ() -> TestResult {
        run(async {
            let rest: Vec<(String, Outcome)> = differing()
                .await?
                .into_iter()
                .filter(|(name, _)| !CALLER_CASES.contains(&name.as_str()))
                .collect();
            let gzipped = unhex(
                "1f8b08000000000000ffaa56ca4c51b2522a52d2512a4e4d2c4902f1a2636b01010000ffff016e18\
                 0217000000",
            )?;
            check_eq(
                &rest,
                &vec![
                    // Go decompresses `GZIP`; reqwest matches `gzip` only.
                    (
                        "upper_case_gzip".to_owned(),
                        Outcome {
                            content_encoding: vec!["GZIP".to_owned()],
                            content_length: vec!["45".to_owned()],
                            ..plain("gzip", &gzipped)
                        },
                    ),
                    // Go keeps the headers of a gzip response without a body.
                    (
                        "no_content_gzip".to_owned(),
                        Outcome {
                            status: 204,
                            ..plain("gzip", b"")
                        },
                    ),
                    ("empty_gzip_body".to_owned(), plain("gzip", b"")),
                    // Go's gzip.Reader reads the short header as `unexpected EOF`.
                    (
                        "corrupt_gzip".to_owned(),
                        Outcome {
                            read_error: "error decoding response body: Invalid gzip header"
                                .to_owned(),
                            ..plain("gzip", b"")
                        },
                    ),
                ],
                "the other cases whose outcome differs from Go's",
            )
        })
    }
}
