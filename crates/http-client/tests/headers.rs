//! Pins of the request head the general client writes against the one Go's general Transport
//! writes for the same request (`testdata/golden/headers.json`, `testdata/go/headers.go`): the
//! `User-Agent` (restored, D-UA), the header names' case (D-HDRCASE), their order, the `Host`
//! line, the white space around values, and the length of an empty body.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_http_client::{
        GO_USER_AGENT, HttpClientSettings,
        reqwest::{
            Body,
            header::{HeaderName, HeaderValue},
        },
    };
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::support::{
        BoxError, RawServer, TestResult, check, check_eq, fetch, golden, list, loopback_client,
        request, run, text, with_ports,
    };

    /// One case's request head as Go's Transport wrote it and as the general client writes it,
    /// the server's port written `PORT`.
    #[derive(Debug)]
    struct Heads {
        name: String,
        go: Vec<String>,
        rust: Vec<String>,
    }

    /// Sends `case`'s request with the general client to a loopback server and returns the
    /// head the server read.
    async fn rust_head(case: &Value) -> Result<Vec<String>, BoxError> {
        let (sender, mut heads) = mpsc::unbounded_channel();
        let server = RawServer::start(move |mut conn| {
            let sender = sender.clone();
            async move {
                if let Some(request) = conn.read_request().await {
                    conn.write(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                        .await;
                    let _sent = sender.send(request.head);
                }
            }
        })
        .await?;
        let port = server.port();
        let url = text(case, "/url")?.replace("PORT", &port.to_string());
        let client = loopback_client(HttpClientSettings::GENERAL, port)?;
        let mut req = request(text(case, "/method")?, &url)?;
        for op in list(case, "/headers")? {
            let name = HeaderName::from_bytes(text(op, "/name")?.as_bytes())?;
            let value = HeaderValue::from_str(text(op, "/value")?)?;
            match text(op, "/op")? {
                "set" => drop(req.headers_mut().insert(name, value)),
                "add" | "raw" => {
                    let _appended: bool = req.headers_mut().append(name, value);
                }
                other => return Err(format!("unknown header op {other}").into()),
            }
        }
        if let Some(body) = case.get("body").and_then(Value::as_str) {
            *req.body_mut() = Some(Body::from(body.to_owned()));
        }
        drop(fetch(client.send(req, None, None).await?).await.body?);
        let head = heads.recv().await.ok_or("the server read no request")?;
        drop(client);
        let _counts = server.close().await;
        Ok(head
            .iter()
            .map(|line| with_ports(line, &[(port, "PORT")]))
            .collect())
    }

    /// Every case of the golden with both heads.
    async fn all_heads() -> Result<Vec<Heads>, BoxError> {
        let doc = golden("headers.json")?;
        let mut out = Vec::new();
        for case in list(&doc, "/cases")? {
            out.push(Heads {
                name: text(case, "/name")?.to_owned(),
                go: list(case, "/head")?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                rust: rust_head(case).await?,
            });
        }
        Ok(out)
    }

    /// The header lines of `head` (after the request line) as name and value.
    fn fields(head: &[String]) -> Vec<(String, String)> {
        head.iter()
            .skip(1)
            .filter_map(|line| line.split_once(": ").or_else(|| line.split_once(':')))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    /// `value` without the spaces and tabs around it, as Go writes every header value.
    fn trimmed(value: &str) -> String {
        value.trim_matches([' ', '\t']).to_owned()
    }

    /// The values of the header `name` in `head`, in order, its name in any case.
    fn values(head: &[String], name: &str) -> Vec<String> {
        fields(head)
            .into_iter()
            .filter(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
            .collect()
    }

    // Go: net/http/request.go Request.write (defaultUserAgent; the first User-Agent value, none
    // for an empty one).
    #[test]
    fn default_user_agent_is_go() -> TestResult {
        run(async {
            let all = all_heads().await?;
            for heads in &all {
                check_eq(
                    &values(&heads.rust, "user-agent"),
                    &values(&heads.go, "user-agent"),
                    &format!("{}: User-Agent", heads.name),
                )?;
            }
            let bare = all
                .iter()
                .find(|heads| heads.name == "get_bare")
                .ok_or("no get_bare case")?;
            check_eq(
                &values(&bare.rust, "user-agent"),
                &vec![GO_USER_AGENT.to_owned()],
                "a request without a User-Agent",
            )
        })
    }

    // Go: net/http/header.go Header.Set (CanonicalMIMEHeaderKey) and Header.write (names put
    // into the map directly go out as written).
    #[test]
    fn lower_case_names() -> TestResult {
        run(async {
            let mut go_mixed_case = 0;
            for heads in all_heads().await? {
                let mut rust = Vec::new();
                // The white space around values is http-client.header-value-space's
                // (values_untrimmed).
                for (name, value) in fields(&heads.rust) {
                    check(
                        name == name.to_ascii_lowercase(),
                        &format!("{}: {name} is not lower case", heads.name),
                    )?;
                    rust.push((name, trimmed(&value)));
                }
                let mut go: Vec<(String, String)> = fields(&heads.go)
                    .into_iter()
                    .inspect(|(name, _)| {
                        if *name != name.to_ascii_lowercase() {
                            go_mixed_case += 1;
                        }
                    })
                    .map(|(name, value)| (name.to_ascii_lowercase(), value))
                    .collect();
                // http-client.default-accept: reqwest's `Accept: */*` where the caller set none.
                if !go.iter().any(|(name, _)| name == "accept") {
                    go.push(("accept".to_owned(), "*/*".to_owned()));
                }
                // The Host line is http-client.host-header's (host_normalised).
                go.retain(|(name, _)| name != "host");
                rust.retain(|(name, _)| name != "host");
                // An empty body's `Content-Length: 0` is http-client.empty-body-length's
                // (empty_body_unframed).
                if values(&heads.rust, "content-length").is_empty() {
                    go.retain(|(name, value)| name != "content-length" || value != "0");
                }
                go.sort();
                rust.sort();
                check_eq(&rust, &go, &format!("{}: the header fields", heads.name))?;
            }
            check(go_mixed_case > 0, "Go wrote canonical names")
        })
    }

    // Go: net/http/request.go Request.write (Host, User-Agent, Content-Length, then
    // Header.WriteSubset's sorted keys, then the Transport's Accept-Encoding).
    #[test]
    fn wire_order() -> TestResult {
        run(async {
            let heads = all_heads().await?;
            let order = heads
                .iter()
                .find(|heads| heads.name == "header_order")
                .ok_or("no header_order case")?;
            let names = |head: &[String]| -> Vec<String> {
                fields(head)
                    .into_iter()
                    .map(|(name, _)| name.to_ascii_lowercase())
                    .collect()
            };
            check_eq(
                &names(&order.go),
                &[
                    "host",
                    "user-agent",
                    "content-length",
                    "accept-language",
                    "alpha",
                    "content-type",
                    "x-forwarded-for",
                    "zeta",
                    "accept-encoding",
                ]
                .map(str::to_owned)
                .to_vec(),
                "Go's order",
            )?;
            check_eq(
                &names(&order.rust),
                &[
                    "zeta",
                    "x-forwarded-for",
                    "accept-language",
                    "content-type",
                    "alpha",
                    "user-agent",
                    "accept",
                    "accept-encoding",
                    "host",
                    "content-length",
                ]
                .map(str::to_owned)
                .to_vec(),
                "the general client's order: the caller's, then reqwest's and hyper's own",
            )
        })
    }

    // Go: net/http/header.go Header.writeSubset (textproto.TrimString on every value) and
    // net/http/request.go Request.write.
    #[test]
    fn values_untrimmed() -> TestResult {
        run(async {
            let heads = all_heads().await?;
            let padded = heads
                .iter()
                .find(|heads| heads.name == "value_whitespace")
                .ok_or("no value_whitespace case")?;
            let own = |head: &[String]| -> Vec<(String, String)> {
                let mut own: Vec<(String, String)> = fields(head)
                    .into_iter()
                    .map(|(name, value)| (name.to_ascii_lowercase(), value))
                    .filter(|(name, _)| name.starts_with("x-"))
                    .collect();
                own.sort();
                own
            };
            let pairs = |pairs: [(&str, &str); 3]| -> Vec<(String, String)> {
                pairs
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .to_vec()
            };
            check_eq(
                &own(&padded.go),
                &pairs([
                    ("x-blank", ""),
                    ("x-padded", "padded value"),
                    ("x-tabbed", "tabbed"),
                ]),
                "Go trims every value",
            )?;
            check_eq(
                &own(&padded.rust),
                &pairs([
                    ("x-blank", "  "),
                    ("x-padded", "  padded value  "),
                    ("x-tabbed", "\ttabbed\t"),
                ]),
                "hyper writes each value as it was set",
            )
        })
    }

    // Go: net/http/transfer.go newTransferWriter and transferWriter.shouldSendContentLength
    // (Content-Length: 0 for a POST, PUT or PATCH without a body, none for another method).
    #[test]
    fn empty_body_unframed() -> TestResult {
        run(async {
            let mut differ = Vec::new();
            for heads in all_heads().await? {
                let go = values(&heads.go, "content-length");
                let rust = values(&heads.rust, "content-length");
                if go != rust {
                    differ.push((heads.name, go, rust));
                }
            }
            check_eq(
                &differ,
                &["post_empty", "put_empty", "patch_empty"]
                    .map(|name| (name.to_owned(), vec!["0".to_owned()], Vec::new()))
                    .to_vec(),
                "the cases whose Content-Length differs (Go's, hyper's)",
            )
        })
    }

    // Go: net/http/request.go Request.write (the Host line is the URL's host as written; a Host
    // entry in Header is never written).
    #[test]
    fn host_normalised() -> TestResult {
        run(async {
            let mut differ = Vec::new();
            for heads in all_heads().await? {
                let go = values(&heads.go, "host");
                let rust = values(&heads.rust, "host");
                check_eq(&rust.len(), &1, &format!("{}: one Host line", heads.name))?;
                if go != rust {
                    differ.push((heads.name, go, rust));
                }
            }
            check_eq(
                &differ,
                &vec![(
                    "host_as_written".to_owned(),
                    vec!["Bidder.Example.COM:80".to_owned()],
                    vec!["bidder.example.com".to_owned()],
                )],
                "the cases whose Host differs",
            )
        })
    }
}
