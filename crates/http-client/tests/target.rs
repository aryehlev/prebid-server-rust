//! The pin of http-client.request-target: the request-target and `Host` Go's Transport writes
//! for every expected-request URI of the adapter fixtures, every bidder-info endpoint template
//! rendered with config.go's sample values and the synthetic URIs that hold what those lack
//! (`testdata/golden/targets.json`, `testdata/go/targets.go`), next to what the clients write for
//! the url crate's parse of the same URI.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pbs_http_client::{
        HttpClientSettings,
        reqwest::{Url, Version},
    };
    use serde_json::Value;
    use tokio::sync::mpsc;
    use url::{Host, Position};

    use crate::support::{
        BoxError, RawServer, TestResult, check_eq, fetch, golden, list, loopback_client, request,
        run, text,
    };

    /// What goes out for one URI: the request-target and the `Host` line, or why there is no
    /// request.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Written {
        Request { target: String, host: String },
        Error(String),
    }

    /// Go's record of one URI.
    fn go_written(entry: &Value) -> Result<Written, BoxError> {
        Ok(match entry.get("error").and_then(Value::as_str) {
            Some(error) => Written::Error(error.to_owned()),
            None => Written::Request {
                target: text(entry, "/target")?.to_owned(),
                host: text(entry, "/host")?.to_owned(),
            },
        })
    }

    /// What the clients write for `uri`: reqwest turns the url crate's serialisation into the
    /// request's `http::Uri`, and hyper writes its path and query as the request-target and its
    /// host and non-default port as `Host`.
    fn rust_written(uri: &str) -> Written {
        match Url::parse(uri) {
            Err(e) => Written::Error(e.to_string()),
            Ok(url) => Written::Request {
                target: url[Position::BeforePath..Position::AfterQuery].to_owned(),
                host: url[Position::BeforeHost..Position::AfterPort].to_owned(),
            },
        }
    }

    /// The URIs of the golden's `section`s with Go's record, once each.
    fn uris(sections: &[&str]) -> Result<BTreeMap<String, Written>, BoxError> {
        let doc = golden("targets.json")?;
        let mut out = BTreeMap::new();
        for section in sections {
            for entry in list(&doc, &format!("/{section}"))? {
                let _earlier = out.insert(text(entry, "/uri")?.to_owned(), go_written(entry)?);
            }
        }
        Ok(out)
    }

    /// Whether `url` can go to the loopback server: plain HTTP to a host name at the scheme's
    /// port, which the loopback guard resolves to the server. An IP address or an explicit port
    /// would be dialled as written.
    fn reaches_test_server(url: &Url) -> bool {
        url.scheme() == "http"
            && matches!(url.host(), Some(Host::Domain(_)))
            && url.port().is_none()
    }

    /// The URIs of `uris` whose request differs, with Go's record and what the clients write.
    fn differing(uris: &BTreeMap<String, Written>) -> Vec<(String, Written, Written)> {
        uris.iter()
            .map(|(uri, go)| (uri.clone(), go.clone(), rust_written(uri)))
            .filter(|(_, go, rust)| rust != go)
            .collect()
    }

    /// A request with `target` and `host`.
    fn sent(target: &str, host: &str) -> Written {
        Written::Request {
            target: target.to_owned(),
            host: host.to_owned(),
        }
    }

    // Go: net/http/request.go Request.write (URL.RequestURI as the request-target, URL.Host as
    // the Host line) and net/url/url.go parse, EscapedPath and RequestURI.
    #[test]
    fn fixture_targets_differ() -> TestResult {
        run(async {
            // The fixtures and endpoints: every target the url crate parses is Go's, and only
            // the Host's case differs (http-client.host-header).
            let corpus = uris(&["fixtures", "endpoints"])?;
            let mut unparsed = Vec::new();
            for (uri, go, rust) in differing(&corpus) {
                match (go, rust) {
                    (
                        Written::Request {
                            target: go_target,
                            host: go_host,
                        },
                        Written::Request { target, host },
                    ) => check_eq(
                        &(target, host),
                        &(go_target, go_host.to_ascii_lowercase()),
                        &format!("{uri}: only the Host's case differs"),
                    )?,
                    (Written::Request { .. }, Written::Error(_)) => unparsed.push(uri),
                    (Written::Error(_), _) => {
                        return Err(format!("{uri}: Go could not write it").into());
                    }
                }
            }
            // Request.write writes these, and Go's Transport refuses to send them: a relative
            // URI has no scheme ("unsupported protocol scheme"), and the `#{REGION}#` templates,
            // which their adapters fill in before they send, have no host before the fragment
            // ("http: no Host in request URL").
            check_eq(
                &unparsed,
                &[
                    "",
                    "//host",
                    "https://#{REGION}#.152media-ssp.com/pserver",
                    "https://#{REGION}#.al-ad.com/pserver",
                    "https://#{REGION}#.zxyvrtd.com/pserver",
                    "uri?tk_xint=pbs-test-tracker",
                ]
                .map(str::to_owned)
                .to_vec(),
                "the fixture and endpoint URIs the url crate does not parse",
            )?;

            // The synthetic URIs hold what the corpus lacks.
            let synthetic = uris(&["synthetic"])?;
            let differ: Vec<(String, Written)> = differing(&synthetic)
                .into_iter()
                .map(|(uri, _, rust)| (uri, rust))
                .collect();
            check_eq(
                &differ,
                &[
                    // The Host (http-client.host-header): lower case, no default port.
                    ("http://Bidder.TEST/P", sent("/P", "bidder.test")),
                    // Go refuses control characters; the url crate drops tabs and newlines.
                    ("http://bidder.test/p\tq", sent("/pq", "bidder.test")),
                    ("http://bidder.test/p\nq", sent("/pq", "bidder.test")),
                    // Go refuses a malformed escape in the path; the url crate keeps it.
                    ("http://bidder.test/p%", sent("/p%", "bidder.test")),
                    ("http://bidder.test/p%zz", sent("/p%zz", "bidder.test")),
                    // Go sends dot segments as written; the url crate resolves them.
                    ("http://bidder.test/p/../q", sent("/q", "bidder.test")),
                    ("http://bidder.test/p/./q", sent("/p/q", "bidder.test")),
                    // Go sends the query as written; the url crate escapes `"`, `'`, `<`, `>`,
                    // spaces and every byte past ASCII.
                    (
                        "http://bidder.test/p?q=\"x\"",
                        sent("/p?q=%22x%22", "bidder.test"),
                    ),
                    (
                        "http://bidder.test/p?q='x'",
                        sent("/p?q=%27x%27", "bidder.test"),
                    ),
                    (
                        "http://bidder.test/p?q=<x>",
                        sent("/p?q=%3Cx%3E", "bidder.test"),
                    ),
                    (
                        "http://bidder.test/p?q=a b",
                        sent("/p?q=a%20b", "bidder.test"),
                    ),
                    (
                        "http://bidder.test/p?q={\"a\":[1]}",
                        sent("/p?q={%22a%22:[1]}", "bidder.test"),
                    ),
                    (
                        "http://bidder.test/p?q=\u{e9}",
                        sent("/p?q=%C3%A9", "bidder.test"),
                    ),
                    // A backslash is a slash to the url crate, and Go escapes it.
                    ("http://bidder.test/p\\q", sent("/p/q", "bidder.test")),
                    // Go escapes `|` and `^` in the path; the url crate keeps `|` and, since
                    // 2.5.9, escapes `^` too.
                    ("http://bidder.test/p|x^y", sent("/p|x%5Ey", "bidder.test")),
                    ("http://bidder.test:80/p", sent("/p", "bidder.test")),
                    ("https://bidder.test:443/p", sent("/p", "bidder.test")),
                ]
                .map(|(uri, written)| (uri.to_owned(), written))
                .to_vec(),
                "the synthetic URIs whose request differs",
            )?;
            wire_matches(&corpus).await?;
            wire_matches(&synthetic).await
        })
    }

    /// Sends every URI that can reach the loopback server and checks that the request line and
    /// `Host` line the server reads are the ones [`rust_written`] gives.
    async fn wire_matches(uris: &BTreeMap<String, Written>) -> TestResult {
        let (sender, mut heads) = mpsc::unbounded_channel();
        let server = RawServer::start(move |mut conn| {
            let sender = sender.clone();
            async move {
                while let Some(request) = conn.read_request().await {
                    let _sent = sender.send(request.head);
                    conn.write(b"HTTP/1.1 204 No Content\r\n\r\n").await;
                }
            }
        })
        .await?;
        let client = loopback_client(HttpClientSettings::GENERAL, server.port())?;
        let mut count = 0_usize;
        for uri in uris.keys() {
            let Ok(url) = Url::parse(uri) else {
                continue;
            };
            if !reaches_test_server(&url) {
                continue;
            }
            let fetched = fetch(client.send(request("POST", uri)?, None, None).await?).await;
            check_eq(&fetched.version, &Version::HTTP_11, "HTTP/1.1")?;
            drop(fetched.body?);
            let head = heads.recv().await.ok_or("the server read no request")?;
            let host = head
                .iter()
                .filter_map(|line| line.split_once(": "))
                .find(|(name, _)| name.eq_ignore_ascii_case("host"))
                .map_or_else(String::new, |(_, value)| value.to_owned());
            let target = head
                .first()
                .and_then(|line| line.strip_prefix("POST "))
                .and_then(|line| line.strip_suffix(" HTTP/1.1"))
                .unwrap_or_default()
                .to_owned();
            check_eq(
                &Written::Request { target, host },
                &rust_written(uri),
                &format!("{uri} on the wire"),
            )?;
            count += 1;
        }
        drop(client);
        let _counts = server.close().await;
        check_eq(&(count > 0), &true, "some URIs went over the wire")
    }
}
