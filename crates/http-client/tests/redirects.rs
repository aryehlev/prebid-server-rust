//! The pin of http-client.redirects: every hop of the redirect matrix as Go's client made it
//! under the default `CheckRedirect` (`testdata/golden/redirects.json`,
//! `testdata/go/redirects.go`), next to the hops the general client makes.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Arc, OnceLock},
    };

    use pbs_http_client::{
        HttpClientSettings,
        reqwest::{
            Body, StatusCode,
            header::{HeaderName, HeaderValue},
        },
    };
    use serde_json::{Map, Value};
    use tokio::sync::mpsc;

    use crate::support::{
        BoxError, RawConn, RawRequest, RawServer, TestResult, canonical_key, check_eq, fetch, flag,
        golden, int, list, loopback_client, ports_in, request, run, text, with_ports,
    };

    /// One request of a chain as the server saw it, ports written `PA` and `PB`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Hop {
        method: String,
        host: String,
        target: String,
        /// Every header but `Host`, under Go's canonical names.
        headers: BTreeMap<String, Vec<String>>,
        body: String,
    }

    /// What one case did: its hops, and the final status or the error.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Chain {
        hops: Vec<Hop>,
        status: i64,
        error: String,
    }

    /// Go's hop as the golden records it.
    fn go_hop(hop: &Value) -> Result<Hop, BoxError> {
        let mut headers = BTreeMap::new();
        for (name, values) in at_object(hop, "/headers")? {
            let values = values
                .as_array()
                .ok_or("header values are not an array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("a non-string value")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let _earlier = headers.insert(name.clone(), values);
        }
        Ok(Hop {
            method: text(hop, "/method")?.to_owned(),
            host: text(hop, "/host")?.to_owned(),
            target: text(hop, "/target")?.to_owned(),
            headers,
            body: text(hop, "/body")?.to_owned(),
        })
    }

    /// The object at `pointer`.
    fn at_object<'v>(doc: &'v Value, pointer: &str) -> Result<&'v Map<String, Value>, BoxError> {
        doc.pointer(pointer)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{pointer} is not an object").into())
    }

    /// Go's chain as the golden records it.
    fn go_chain(case: &Value) -> Result<Chain, BoxError> {
        Ok(Chain {
            hops: list(case, "/hops")?
                .iter()
                .map(go_hop)
                .collect::<Result<_, _>>()?,
            status: int(case, "/status")?,
            error: text(case, "/error")?.to_owned(),
        })
    }

    /// A request head as Go's server hands it to its handler.
    fn hop_of(request: &RawRequest, ports: &[(u16, &str)]) -> Hop {
        let mut words = request.head.first().map_or("", String::as_str).split(' ');
        let method = words.next().unwrap_or_default().to_owned();
        let target = words.next().unwrap_or_default().to_owned();
        let mut host = String::new();
        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for line in request.head.iter().skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let value = with_ports(value.trim(), ports);
            if name.eq_ignore_ascii_case("host") {
                host = value;
            } else {
                headers.entry(canonical_key(name)).or_default().push(value);
            }
        }
        Hop {
            method,
            host,
            target,
            headers,
            body: String::from_utf8_lossy(&request.body).into_owned(),
        }
    }

    /// The answer to one hop, as `redirectServer.ServeHTTP` gives it.
    fn answer(case: &Value, path: &str, ports: &[(u16, &str)]) -> Result<Vec<u8>, BoxError> {
        let (status, location) = if flag(case, "/loop")?
            && let Some(n) = path.strip_prefix("/r/").and_then(|n| n.parse::<u32>().ok())
        {
            (302, format!("/r/{}", n + 1))
        } else {
            let mut found = (200, String::new());
            for step in list(case, "/steps")? {
                if text(step, "/path")? == path {
                    found = (
                        u16::try_from(int(step, "/status")?)?,
                        ports_in(text(step, "/location")?, ports),
                    );
                    break;
                }
            }
            found
        };
        let body = if status == 200 { "done" } else { "" };
        let reason = StatusCode::from_u16(status)?
            .canonical_reason()
            .unwrap_or_default();
        let location = if location.is_empty() {
            String::new()
        } else {
            format!("Location: {location}\r\n")
        };
        Ok(format!(
            "HTTP/1.1 {status} {reason}\r\n{location}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes())
    }

    /// Serves `case` on two loopback listeners, sends its request with the general client, and
    /// returns the chain.
    async fn rust_chain(case: &Value) -> Result<Chain, BoxError> {
        let ports: Arc<OnceLock<[(u16, &'static str); 2]>> = Arc::new(OnceLock::new());
        let (sender, mut hops) = mpsc::unbounded_channel();
        let serve = |served: Value| {
            let ports = Arc::clone(&ports);
            let sender = sender.clone();
            move |mut conn: RawConn| {
                let ports = Arc::clone(&ports);
                let sender = sender.clone();
                let served = served.clone();
                async move {
                    while let Some(request) = conn.read_request().await {
                        let ports = ports.get().copied().unwrap_or_default();
                        let hop = hop_of(&request, &ports);
                        let path = hop.target.split('?').next().unwrap_or_default().to_owned();
                        let _sent = sender.send(hop);
                        let Ok(response) = answer(&served, &path, &ports) else {
                            return;
                        };
                        conn.write(&response).await;
                    }
                }
            }
        };
        let server_a = RawServer::start(serve(case.clone())).await?;
        let server_b = RawServer::start(serve(case.clone())).await?;
        let pair = [(server_a.port(), "PA"), (server_b.port(), "PB")];
        ports
            .set(pair)
            .map_err(|_already| "the ports were set twice")?;
        drop(sender);

        let client = loopback_client(HttpClientSettings::GENERAL, 0)?;
        let mut req = request(
            text(case, "/method")?,
            &ports_in(text(case, "/url")?, &pair),
        )?;
        for op in list(case, "/headers")? {
            let name = HeaderName::from_bytes(text(op, "/name")?.as_bytes())?;
            let value = HeaderValue::from_str(text(op, "/value")?)?;
            drop(req.headers_mut().insert(name, value));
        }
        if let Some(body) = case.get("body").and_then(Value::as_str) {
            *req.body_mut() = Some(Body::from(body.to_owned()));
        }
        let (status, error) = match client.send(req, None, None).await {
            Ok(response) => {
                let fetched = fetch(response).await;
                drop(fetched.body?);
                (i64::from(fetched.status.as_u16()), String::new())
            }
            Err(e) => (0, with_ports(&e.to_string(), &pair)),
        };
        drop(client);
        let _a = server_a.close().await;
        let _b = server_b.close().await;
        let mut chain = Vec::new();
        while let Ok(hop) = hops.try_recv() {
            chain.push(hop);
        }
        Ok(Chain {
            hops: chain,
            status,
            error,
        })
    }

    /// `chain` without what other entries ledger: reqwest's `Accept: */*` where the caller set
    /// none (http-client.default-accept).
    fn without_default_accept(mut chain: Chain) -> Chain {
        for hop in &mut chain.hops {
            if hop.headers.get("Accept") == Some(&vec!["*/*".to_owned()]) {
                let _accept = hop.headers.remove("Accept");
            }
        }
        chain
    }

    /// How `rust` differs from `go`, one line a difference.
    fn diff(go: &Chain, rust: &Chain) -> Vec<String> {
        let mut out = Vec::new();
        if go.status != rust.status {
            out.push(format!("status {} -> {}", go.status, rust.status));
        }
        if go.error != rust.error {
            out.push(format!("error {:?} -> {:?}", go.error, rust.error));
        }
        if go.hops.len() != rust.hops.len() {
            out.push(format!("{} hops -> {}", go.hops.len(), rust.hops.len()));
        }
        for (n, (go_hop, rust_hop)) in go.hops.iter().zip(&rust.hops).enumerate() {
            let fields = [
                ("method", &go_hop.method, &rust_hop.method),
                ("host", &go_hop.host, &rust_hop.host),
                ("target", &go_hop.target, &rust_hop.target),
                ("body", &go_hop.body, &rust_hop.body),
            ];
            for (field, go_value, rust_value) in fields {
                if go_value != rust_value {
                    out.push(format!("hop {n} {field} {go_value:?} -> {rust_value:?}"));
                }
            }
            let names: BTreeSet<&String> = go_hop
                .headers
                .keys()
                .chain(rust_hop.headers.keys())
                .collect();
            for name in names {
                let go_values = go_hop.headers.get(name);
                let rust_values = rust_hop.headers.get(name);
                if go_values != rust_values {
                    out.push(format!("hop {n} {name} {go_values:?} -> {rust_values:?}"));
                }
            }
        }
        out
    }

    // Go: net/http/client.go Client.do, redirectBehavior, defaultCheckRedirect,
    // makeHeadersCopier, shouldCopyHeaderOnRedirect and refererForURL.
    #[test]
    fn matrix_like_go() -> TestResult {
        run(async {
            let doc = golden("redirects.json")?;
            let mut differ = BTreeMap::new();
            for case in list(&doc, "/cases")? {
                let go = go_chain(case)?;
                let rust = without_default_accept(rust_chain(case).await?);
                let lines = diff(&go, &rust);
                if !lines.is_empty() {
                    let _earlier = differ.insert(text(case, "/name")?.to_owned(), lines);
                }
            }
            let expected: BTreeMap<String, Vec<String>> = [
                // reqwest writes the previous URL over the caller's Referer.
                (
                    "explicit_referer",
                    vec![concat!(
                        r#"hop 1 Referer Some(["http://explicit.test/"]) -> "#,
                        r#"Some(["http://origin.test:PA/start"])"#,
                    )],
                ),
                // Go drops the caller's content headers where it drops the body; reqwest only
                // where it drops a body it had.
                (
                    "get_302_body_headers",
                    vec![
                        r#"hop 1 Content-Language None -> Some(["en"])"#,
                        r#"hop 1 Content-Type None -> Some(["application/json"])"#,
                    ],
                ),
                // Go names the Location as the server wrote it (http-client.error-texts).
                (
                    "hop_limit",
                    vec![concat!(
                        r#"error "Get \"/r/10\": stopped after 10 redirects" -> "#,
                        r#""Get \"http://origin.test:PA/r/10\": stopped after 10 redirects""#,
                    )],
                ),
                // Go compares host names only, against the first request's host and its
                // subdomains; reqwest strips the sensitive headers on any change of host, port
                // or scheme from the previous hop.
                (
                    "port_change",
                    vec![
                        r#"hop 1 Authorization Some(["Bearer secret"]) -> None"#,
                        r#"hop 1 Cookie Some(["c=1"]) -> None"#,
                    ],
                ),
                // Go turns every method but GET and HEAD into a GET on 301 and 302; reqwest only
                // a POST.
                (
                    "put_302",
                    vec![
                        r#"hop 1 method "GET" -> "PUT""#,
                        r#"hop 1 body "" -> "{\"id\":\"r\"}""#,
                        r#"hop 1 Content-Length None -> Some(["10"])"#,
                        r#"hop 1 Content-Type None -> Some(["application/json"])"#,
                    ],
                ),
                (
                    "subdomain",
                    vec![
                        r#"hop 1 Authorization Some(["Bearer secret"]) -> None"#,
                        r#"hop 1 Cookie Some(["c=1"]) -> None"#,
                    ],
                ),
            ]
            .into_iter()
            .map(|(name, lines)| {
                (
                    name.to_owned(),
                    lines.into_iter().map(str::to_owned).collect(),
                )
            })
            .collect();
            check_eq(&differ, &expected, "the cases whose chain differs")
        })
    }
}
