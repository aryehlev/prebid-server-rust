//! Pins of the clients' proxies against Go's (`testdata/golden/proxy.json`,
//! `testdata/go/proxy.go`): the proxy x/net/http/httpproxy picks for each probe environment and
//! URL (http-client.proxy, restored by Rule B (c)) and the picks that still differ
//! (http-client.proxy-exceptions), what a proxy receives, and the proxy schemes Go dials that the
//! clients refuse (http-client.socks-proxy).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use pbs_http_client::{
        Builder, ErrorKind, HttpClientSettings, ProxyEnv,
        proxy::Choice,
        reqwest::{Url, Version},
    };
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::support::{
        BoxError, RawServer, TestResult, at, check, check_eq, flag, golden, list, request, run,
        strings, text, with_ports,
    };

    /// A probe's environment as `httpproxy.FromEnvironment` reads it.
    fn probe_env(probe: &Value) -> Result<ProxyEnv, BoxError> {
        let env = at(probe, "/env")?
            .as_object()
            .ok_or("the env is not an object")?;
        Ok(ProxyEnv::from_lookup(|name| {
            env.get(name).and_then(Value::as_str).map(str::to_owned)
        }))
    }

    /// The probe's Config as the golden records it.
    fn go_config(probe: &Value) -> Result<ProxyEnv, BoxError> {
        Ok(ProxyEnv {
            http_proxy: text(probe, "/config/http_proxy")?.to_owned(),
            https_proxy: text(probe, "/config/https_proxy")?.to_owned(),
            no_proxy: text(probe, "/config/no_proxy")?.to_owned(),
            cgi: flag(probe, "/config/cgi")?,
        })
    }

    /// What the rules pick for `url`, as the golden writes Go's pick: the proxy's text (`""` for
    /// a direct connection) and the error.
    fn rust_pick(env: &ProxyEnv, url: &str) -> (String, String) {
        let Ok(url) = Url::parse(url) else {
            return (String::new(), "parse".to_owned());
        };
        match env.rules().choose(&url) {
            Choice::Direct => (String::new(), String::new()),
            Choice::Proxy(proxy) => (proxy.text().to_owned(), String::new()),
            Choice::Refused => (
                String::new(),
                "refusing to use HTTP_PROXY value in CGI environment; see \
                 golang.org/s/cgihttpproxy"
                    .to_owned(),
            ),
        }
    }

    /// Go's pick and the rules' for one probe URL: the proxy's text and the error.
    type Pick = (String, String);

    /// Every probe's picks, compared: the probe URLs the url crate does not parse, how many
    /// `LocalHost` picks Go proxies, how many picks of the bogus value Go writes escaped, and
    /// every other pick that differs, with the probe's `NO_PROXY`.
    #[derive(Debug, Default)]
    struct Compared {
        unparsed: BTreeSet<String>,
        localhost: usize,
        bogus: usize,
        differ: Vec<(String, String, Pick, Pick)>,
    }

    /// Compares the rules' picks with Go's over every probe of the golden.
    fn compare() -> Result<Compared, BoxError> {
        let doc = golden("proxy.json")?;
        let mut compared = Compared::default();
        for probe in list(&doc, "/probes")? {
            let env = probe_env(probe)?;
            check_eq(&env, &go_config(probe)?, "the Config FromEnvironment reads")?;
            for result in list(probe, "/results")? {
                let url = text(result, "/url")?;
                let go_error = text(result, "/err")?;
                let go = (
                    text(result, "/proxy")?.to_owned(),
                    if go_error.starts_with("parse: ") {
                        "parse".to_owned()
                    } else {
                        go_error.to_owned()
                    },
                );
                let rust = rust_pick(&env, url);
                if rust == go {
                    continue;
                }
                if rust.1 == "parse" {
                    let _new = compared.unparsed.insert(url.to_owned());
                } else if url.contains("LocalHost") {
                    check_eq(
                        &rust,
                        &(String::new(), String::new()),
                        "LocalHost goes direct",
                    )?;
                    compared.localhost += 1;
                } else if go.0 == "bogus%20url" {
                    check_eq(&rust.0.as_str(), &"bogus url", "the bogus value")?;
                    compared.bogus += 1;
                } else {
                    compared
                        .differ
                        .push((env.no_proxy.clone(), url.to_owned(), go, rust));
                }
            }
        }
        Ok(compared)
    }

    // Go: golang.org/x/net/http/httpproxy (v0.55.0) FromEnvironment, Config.ProxyFunc,
    // config.useProxy and parseProxy.
    #[test]
    fn proxy_env_like_go() -> TestResult {
        let compared = compare()?;
        check_eq(
            &compared.differ,
            &Vec::new(),
            "the picks that differ, beyond proxy_exceptions_differ's",
        )
    }

    // Go: golang.org/x/net/http/httpproxy (v0.55.0) config.useProxy (the `localhost` test before
    // the host is lower-cased) and net/url/url.go parseHost (zones).
    #[test]
    fn proxy_exceptions_differ() -> TestResult {
        let compared = compare()?;
        // The url crate parses no IPv6 zone, so these never reach a client.
        check_eq(
            &compared.unparsed,
            &[
                "http://[::1%25lo0]/",
                "http://[::1%25lo0]:8080/p",
                "http://[fe80::1%25en0]/",
                "http://[fe80::1%25en0]:8080/p",
                "https://[::1%25lo0]/",
                "https://[::1%25lo0]:443/",
                "https://[fe80::1%25en0]/",
                "https://[fe80::1%25en0]:443/",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            "the probe URLs the url crate does not parse",
        )?;
        // Go proxies `LocalHost` wherever a proxy is set and NO_PROXY does not name it; the
        // url crate has lower-cased it, and the clients never proxy localhost.
        check_eq(&compared.localhost, &130, "LocalHost picks Go proxies")?;
        // Go's String() escapes the path it read `bogus url` as; the clients refuse the value
        // when they send (socks_proxy_refused), and Go's dial fails.
        check_eq(&compared.bogus, &34, "picks of the bogus value")
    }

    /// A loopback proxy that reads one request and answers it with `status`, and the head it
    /// read, its port written `PROXY`.
    async fn through_proxy(url: &str, status: &str) -> Result<(Vec<String>, String), BoxError> {
        let (sender, mut heads) = mpsc::unbounded_channel();
        let answer =
            format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        let proxy = RawServer::start(move |mut conn| {
            let sender = sender.clone();
            let answer = answer.clone();
            async move {
                if let Some(request) = conn.read_request().await {
                    let _sent = sender.send(request.head);
                    conn.write(answer.as_bytes()).await;
                }
            }
        })
        .await?;
        let port = proxy.port();
        let address = format!("http://127.0.0.1:{port}");
        let env = ProxyEnv {
            http_proxy: address.clone(),
            https_proxy: address,
            ..ProxyEnv::default()
        };
        let client = Builder::new(HttpClientSettings::GENERAL, env)
            .loopback_only(0)
            .build()?;
        let error = match client.send(request("GET", url)?, None, None).await {
            Ok(response) => {
                check_eq(&response.version(), &Version::HTTP_11, "HTTP/1.1")?;
                drop(response.bytes().await?);
                String::new()
            }
            Err(e) => e.to_string(),
        };
        let head = heads.recv().await.ok_or("the proxy read no request")?;
        drop(client);
        let _counts = proxy.close().await;
        let ports = [(port, "PROXY")];
        Ok((
            head.iter().map(|line| with_ports(line, &ports)).collect(),
            with_ports(&error, &ports),
        ))
    }

    /// `head`'s request line, and its header fields with lower-case names, sorted, without
    /// reqwest's default `Accept` (http-client.default-accept).
    fn fields(head: &[String]) -> (String, Vec<String>) {
        let mut fields: Vec<String> = head
            .iter()
            .skip(1)
            .filter_map(|line| line.split_once(": "))
            .map(|(name, value)| format!("{}: {value}", name.to_ascii_lowercase()))
            .filter(|field| field != "accept: */*")
            .collect();
        fields.sort();
        (head.first().cloned().unwrap_or_default(), fields)
    }

    // Go: net/http/transport.go persistConn.roundTrip (an absolute-form request to an http
    // proxy) and Transport.dialConn (CONNECT for an https URL).
    #[test]
    fn requests_reach_the_proxy_like_go() -> TestResult {
        run(async {
            let doc = golden("proxy.json")?;
            let mut errors = Vec::new();
            for wire in list(&doc, "/wire")? {
                let url = text(wire, "/url")?;
                let (head, error) = through_proxy(url, text(wire, "/status")?).await?;
                check_eq(
                    &fields(&head),
                    &fields(&strings(wire, "/head")?),
                    &format!("{url}: what the proxy read"),
                )?;
                errors.push((text(wire, "/error")?.to_owned(), error));
            }
            check_eq(
                &errors,
                &vec![
                    (String::new(), String::new()),
                    // http-client.error-texts.
                    (
                        r#"Get "https://bidder.test/p?q=1": Forbidden"#.to_owned(),
                        concat!(
                            r#"Get "https://bidder.test/p?q=1": error sending request: "#,
                            "client error (Connect): tunnel error: unsuccessful"
                        )
                        .to_owned(),
                    ),
                ],
                "the errors (Go, Rust)",
            )
        })
    }

    // Go: net/http/transport.go Transport.dialConn (socks5 and socks5h proxies through
    // golang.org/x/net/internal/socks).
    #[test]
    fn socks_proxy_refused() -> TestResult {
        let doc = golden("proxy.json")?;
        let probe = list(&doc, "/probes")?
            .iter()
            .find(|probe| {
                probe
                    .pointer("/env/HTTP_PROXY")
                    .and_then(Value::as_str)
                    .is_some_and(|proxy| proxy.starts_with("socks5://"))
            })
            .ok_or("no socks probe")?;
        let picks: BTreeSet<String> = list(probe, "/results")?
            .iter()
            .filter_map(|result| result.get("proxy").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        check(
            picks.contains("socks5://user:pw@socks.test:1080")
                && picks.contains("socks5h://socks.test"),
            "Go picks the socks proxies, and its Transport dials them",
        )?;
        let env = probe_env(probe)?;
        run(async move {
            let client = Builder::new(HttpClientSettings::GENERAL, env)
                .loopback_only(0)
                .build()?;
            let mut refused = Vec::new();
            for url in ["http://bidder.test/p", "https://bidder.test/p"] {
                match client.send(request("GET", url)?, None, None).await {
                    Ok(_) => return Err(format!("{url} was sent").into()),
                    Err(e) => refused.push((e.kind(), e.to_string())),
                }
            }
            check_eq(
                &refused,
                &vec![
                    (
                        ErrorKind::Proxy,
                        concat!(
                            r#"Get "http://bidder.test/p": HTTP_PROXY names a "socks5" proxy, "#,
                            "and only http and https proxies are supported"
                        )
                        .to_owned(),
                    ),
                    (
                        ErrorKind::Proxy,
                        concat!(
                            r#"Get "https://bidder.test/p": HTTPS_PROXY names a "socks5h" proxy, "#,
                            "and only http and https proxies are supported"
                        )
                        .to_owned(),
                    ),
                ],
                "what the clients do with them",
            )
        })
    }
}
