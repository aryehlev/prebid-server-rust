//! The pin of http-client.connection-metrics: the connection events each request of a run
//! reports, next to the httptrace hooks Go's general Transport fired for the same run
//! (`testdata/golden/metrics.json`, `testdata/go/metrics.go`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex, PoisonError},
        time::Duration,
    };

    use pbs_http_client::{Builder, Client, Event, HttpClientSettings, ProxyEnv, Trace};
    use serde_json::Value;
    use tokio::{task::JoinSet, time::sleep};

    use crate::support::{
        BoxError, RawConn, RawServer, TestPki, TestResult, check_eq, fetch, golden, list, ports_in,
        request, run, text,
    };

    /// A server that answers every request on a connection with 200 "ok" after `delay`, or
    /// closes the connection once it read a request when `closes`, over TLS when `pki` is
    /// given.
    async fn server(
        delay: Duration,
        closes: bool,
        pki: Option<&TestPki>,
    ) -> Result<RawServer, BoxError> {
        let handle = move |mut conn: RawConn| async move {
            while conn.read_request().await.is_some() {
                if closes {
                    break;
                }
                sleep(delay).await;
                conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            }
        };
        match pki {
            None => RawServer::start(handle).await,
            Some(pki) => RawServer::start_tls(pki.acceptor(&[b"http/1.1"])?, handle).await,
        }
    }

    /// One request's events, written as the golden writes Go's hooks, and whether it failed.
    type Traced = (Vec<String>, bool);

    /// The events one request reports, and whether it failed.
    async fn traced(client: &Client, url: &str) -> Result<Traced, BoxError> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let trace = Trace::new({
            let events = Arc::clone(&events);
            move |event| {
                let mut events = events.lock().unwrap_or_else(PoisonError::into_inner);
                match event {
                    Event::Dns { .. } => {
                        events.push("DNSStart".to_owned());
                        events.push("DNSDone".to_owned());
                    }
                    Event::GotConn { reused, .. } => {
                        events.push(format!("GotConn reused={reused}"));
                    }
                    _ => events.push(format!("{event:?}")),
                }
            }
        });
        let failed = match client.send(request("GET", url)?, None, Some(trace)).await {
            Ok(response) => fetch(response).await.body.is_err(),
            Err(_) => true,
        };
        let recorded = events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok((recorded, failed))
    }

    /// Runs `case` with the general client and returns each request's events, and whether it
    /// failed.
    async fn rust_events(case: &Value, pki: &TestPki) -> Result<Vec<Traced>, BoxError> {
        let url = text(case, "/url")?;
        let concurrent = case
            .get("concurrent")
            .and_then(Value::as_bool)
            .ok_or("no concurrent")?;
        let delay = if concurrent {
            Duration::from_millis(100)
        } else {
            Duration::ZERO
        };
        let closes = case.get("errors").is_some();
        let server = server(delay, closes, url.starts_with("https:").then_some(pki)).await?;
        let full = ports_in(url, &[(server.port(), "PORT")]);
        let client = Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .build()?;
        let mut events = Vec::new();
        if concurrent {
            let mut tasks = JoinSet::new();
            for _ in 0..2 {
                let client = client.clone();
                let full = full.clone();
                let _handle = tasks.spawn(async move { traced(&client, &full).await });
            }
            while let Some(joined) = tasks.join_next().await {
                events.push(joined??);
            }
        } else {
            for _ in 0..2 {
                events.push(traced(&client, &full).await?);
            }
        }
        drop(client);
        let _counts = server.close().await;
        Ok(events)
    }

    /// The events of a request through router.go's general client's own half, then of one
    /// through its bidders' half, to `case`'s server; the bidders' half negotiates HTTP/2 when
    /// `negotiate`.
    async fn halves_events(
        case: &Value,
        pki: &TestPki,
        negotiate: bool,
    ) -> Result<Vec<Vec<String>>, BoxError> {
        let url = text(case, "/url")?;
        let server = server(
            Duration::ZERO,
            false,
            url.starts_with("https:").then_some(pki),
        )
        .await?;
        let full = ports_in(url, &[(server.port(), "PORT")]);
        let builder = Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0);
        let clients = if negotiate {
            builder.negotiate_http2()
        } else {
            builder
        }
        .build_general()?;
        let mut events = Vec::new();
        for client in [&clients.general, &clients.bidders] {
            let (recorded, failed) = traced(client, &full).await?;
            if failed {
                return Err(format!("{full}: a request failed").into());
            }
            events.push(recorded);
        }
        drop(clients);
        let _counts = server.close().await;
        Ok(events)
    }

    /// Go's hooks for each request, without the ones the clients have no event for.
    fn go_events(case: &Value) -> Result<(Vec<Vec<String>>, Vec<String>), BoxError> {
        let mut kept = Vec::new();
        let mut missing = Vec::new();
        for request in list(case, "/events")? {
            let mut events = Vec::new();
            for event in request.as_array().ok_or("not an array")? {
                let event = event.as_str().ok_or("not a string")?.to_owned();
                if event.starts_with("DNS") || event.starts_with("GotConn") {
                    events.push(event);
                } else if !missing.contains(&event) {
                    missing.push(event);
                }
            }
            kept.push(events);
        }
        Ok((kept, missing))
    }

    // Go: exchange/bidder.go:732-798 (the httptrace.ClientTrace hooks behind
    // adapter_connection_created, adapter_connection_reused, adapter_connection_wait, the DNS
    // and the TLS handshake metrics).
    #[test]
    fn events_recorded() -> TestResult {
        run(async {
            let doc = golden("metrics.json")?;
            let pki = TestPki::new()?;
            let mut differ = Vec::new();
            let mut missing = Vec::new();
            for case in list(&doc, "/cases")? {
                let name = text(case, "/name")?;
                let (go, without) = go_events(case)?;
                let go_failed: Vec<bool> = match case.get("errors") {
                    Some(errors) => errors
                        .as_array()
                        .ok_or("errors is not an array")?
                        .iter()
                        .map(|error| error.as_str().is_some_and(|error| !error.is_empty()))
                        .collect(),
                    None => vec![false; go.len()],
                };
                let (rust, rust_failed): (Vec<Vec<String>>, Vec<bool>) =
                    rust_events(case, &pki).await?.into_iter().unzip();
                check_eq(
                    &rust_failed,
                    &go_failed,
                    &format!("{name}: the requests that failed"),
                )?;
                if rust != go {
                    differ.push((name.to_owned(), go, rust));
                }
                missing.push((name.to_owned(), without));
            }
            // Go's GotConn fires once a request has its connection; the clients read the
            // connection off the response, so a request that fails before its response head
            // reports none.
            let got = || vec!["GotConn reused=false".to_owned()];
            check_eq(
                &differ,
                &vec![(
                    "server_closes".to_owned(),
                    vec![got(), got()],
                    vec![Vec::new(), Vec::new()],
                )],
                "the cases whose events differ (Go, Rust)",
            )?;
            // The hooks with no event: GetConn (the wait's start), the dial and the TLS
            // handshake.
            let hooks = |names: &[&str]| -> Vec<String> {
                names.iter().map(|name| (*name).to_owned()).collect()
            };
            check_eq(
                &missing,
                &vec![
                    (
                        "http_ip".to_owned(),
                        hooks(&["GetConn", "ConnectStart", "ConnectDone err=false"]),
                    ),
                    ("http_hostname".to_owned(), hooks(&["GetConn"])),
                    (
                        "https_ip".to_owned(),
                        hooks(&[
                            "GetConn",
                            "ConnectStart",
                            "ConnectDone err=false",
                            "TLSHandshakeStart",
                            "TLSHandshakeDone err=false",
                        ]),
                    ),
                    (
                        "concurrent_new_connections".to_owned(),
                        hooks(&["GetConn", "ConnectStart", "ConnectDone err=false"]),
                    ),
                    (
                        "server_closes".to_owned(),
                        hooks(&["GetConn", "ConnectStart", "ConnectDone err=false"]),
                    ),
                ],
                "Go's hooks the clients report nothing for",
            )
        })
    }

    // Go: router/router.go:143-158 and :211-247 (one Transport for the modules, the
    // stored-request fetchers, the vendor list and the bidders, so a bidder's request takes the
    // connection another of them left idle) and exchange/bidder.go:732-798 (GotConn's Reused).
    #[test]
    fn general_client_halves_share_connections() -> TestResult {
        run(async {
            let doc = golden("metrics.json")?;
            let pki = TestPki::new()?;
            let mut differ = Vec::new();
            for negotiate in [false, true] {
                for name in ["http_ip", "https_ip"] {
                    let case = list(&doc, "/cases")?
                        .iter()
                        .find(|case| text(case, "/name").ok() == Some(name))
                        .ok_or_else(|| format!("metrics.json has no {name}"))?;
                    let (go, _) = go_events(case)?;
                    let rust = halves_events(case, &pki, negotiate).await?;
                    if rust != go {
                        differ.push((name.to_owned(), negotiate, go, rust));
                    }
                }
            }
            // Where the bidders' half negotiates HTTP/2, its TLS connections are its own
            // (http-client.bidder-tls-pool): its request opens one where Go's takes the one the
            // first request left idle.
            let got = |reused: bool| vec![format!("GotConn reused={reused}")];
            check_eq(
                &differ,
                &vec![(
                    "https_ip".to_owned(),
                    true,
                    vec![got(false), got(true)],
                    vec![got(false), got(false)],
                )],
                "the cases whose events differ (case, negotiating HTTP/2, Go, Rust)",
            )
        })
    }
}
