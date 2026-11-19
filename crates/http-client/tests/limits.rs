//! Pins of the clients' connection limits and timeouts against Go's general Transport with the
//! same settings (`testdata/golden/limits.json`, `testdata/go/limits.go`):
//! `max_connections_per_host` (restored by a per-host semaphore), the idle caps and timeouts, and
//! the TLS handshake timeout.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pbs_http_client::{Builder, HttpClientSettings, ProxyEnv};
    use serde_json::Value;
    use tokio::{
        task::JoinSet,
        time::{Instant, sleep},
    };

    use crate::support::{
        BoxError, Counts, RawServer, TestPki, TestResult, at, check_eq, flag, golden, int, list,
        loopback_client, request, run, strings, with_ports,
    };

    /// What one case did, as the golden records it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Outcome {
        statuses: Vec<i64>,
        errors: Vec<String>,
        accepted: Vec<i64>,
        max_open: Vec<i64>,
        serialized: Option<bool>,
        timeout: Option<bool>,
        within_limit: Option<bool>,
    }

    /// Go's outcome of the case named `name`.
    fn go_outcome(doc: &Value, name: &str) -> Result<(Value, Outcome), BoxError> {
        let case = list(doc, "/cases")?
            .iter()
            .find(|case| case.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("no case {name}"))?;
        let ints = |pointer: &str| -> Result<Vec<i64>, BoxError> {
            list(case, pointer)?
                .iter()
                .map(|value| value.as_i64().ok_or_else(|| "not an integer".into()))
                .collect()
        };
        let optional = |pointer: &str| -> Result<Option<bool>, BoxError> {
            Ok(match case.pointer(pointer) {
                None => None,
                Some(_) => Some(flag(case, pointer)?),
            })
        };
        Ok((
            case.clone(),
            Outcome {
                statuses: ints("/statuses")?,
                errors: strings(case, "/errors")?,
                accepted: ints("/accepted")?,
                max_open: ints("/max_open")?,
                serialized: optional("/serialized")?,
                timeout: optional("/timeout")?,
                within_limit: optional("/within_limit")?,
            },
        ))
    }

    /// The general client's settings with the case's, as config.go's keys name them.
    fn settings(case: &Value) -> Result<HttpClientSettings, BoxError> {
        let mut settings = HttpClientSettings::GENERAL;
        let keys = at(case, "/settings")?
            .as_object()
            .ok_or("the settings are not an object")?;
        for key in keys.keys() {
            let value = int(case, &format!("/settings/{key}"))?;
            match key.as_str() {
                "max_connections_per_host" => settings.max_connections_per_host = value,
                "max_idle_connections" => settings.max_idle_connections = value,
                "max_idle_connections_per_host" => settings.max_idle_connections_per_host = value,
                "idle_connection_timeout_seconds" => {
                    settings.idle_connection_timeout_seconds = value;
                }
                "tls_handshake_timeout_seconds" => settings.tls_handshake_timeout_seconds = value,
                other => return Err(format!("unknown setting {other}").into()),
            }
        }
        Ok(settings)
    }

    /// A server that answers every request on a connection with 200 "ok" after `delay`
    /// (`keepAliveServer`).
    async fn keep_alive_server(delay: Duration) -> Result<RawServer, BoxError> {
        RawServer::start(move |mut conn| async move {
            while conn.read_request().await.is_some() {
                sleep(delay).await;
                conn.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            }
        })
        .await
    }

    /// The final counts as the golden writes them.
    fn counted(counts: &[Counts]) -> Result<(Vec<i64>, Vec<i64>), BoxError> {
        let accepted = counts
            .iter()
            .map(|c| i64::try_from(c.accepted))
            .collect::<Result<_, _>>()?;
        let max_open = counts
            .iter()
            .map(|c| i64::try_from(c.max_open))
            .collect::<Result<_, _>>()?;
        Ok((accepted, max_open))
    }

    /// `concurrentCase`: `n` GETs at once to a server that answers each after 100 ms.
    async fn concurrent(case: &Value, n: usize) -> Result<Outcome, BoxError> {
        let delay = Duration::from_millis(100);
        let server = keep_alive_server(delay).await?;
        let client = loopback_client(settings(case)?, 0)?;
        let url = format!("http://127.0.0.1:{}/l", server.port());
        let start = Instant::now();
        let mut tasks = JoinSet::new();
        for i in 0..n {
            let client = client.clone();
            let req = request("GET", &url)?;
            let _handle = tasks.spawn(async move {
                let result = match client.send(req, None, None).await {
                    Ok(response) => {
                        let status = i64::from(response.status().as_u16());
                        response
                            .bytes()
                            .await
                            .map_or_else(|e| (0, e.to_string()), |_| (status, String::new()))
                    }
                    Err(e) => (0, e.to_string()),
                };
                (i, result)
            });
        }
        let mut results = vec![(0, String::new()); n];
        while let Some(joined) = tasks.join_next().await {
            let (i, result) = joined?;
            if let Some(slot) = results.get_mut(i) {
                *slot = result;
            }
        }
        let serialized = start.elapsed() >= delay * u32::try_from(n)?;
        drop(client);
        let counts = server.close().await;
        let (accepted, max_open) = counted(&[counts])?;
        Ok(Outcome {
            statuses: results.iter().map(|(status, _)| *status).collect(),
            errors: results.into_iter().map(|(_, error)| error).collect(),
            accepted,
            max_open,
            serialized: Some(serialized),
            timeout: None,
            within_limit: None,
        })
    }

    /// `sequenceCase`: one GET to each of `servers` servers in `order`, `pause` before each but
    /// the first.
    async fn sequence(
        case: &Value,
        servers: usize,
        order: &[usize],
        pause: Duration,
    ) -> Result<Outcome, BoxError> {
        let mut running = Vec::new();
        for _ in 0..servers {
            running.push(keep_alive_server(Duration::ZERO).await?);
        }
        let client = loopback_client(settings(case)?, 0)?;
        let mut statuses = Vec::new();
        let mut errors = Vec::new();
        for (i, &n) in order.iter().enumerate() {
            if i > 0 {
                sleep(pause).await;
            }
            let port = running.get(n).ok_or("no such server")?.port();
            let req = request("GET", &format!("http://127.0.0.1:{port}/l"))?;
            match client.send(req, None, None).await {
                Ok(response) => {
                    statuses.push(i64::from(response.status().as_u16()));
                    drop(response.bytes().await?);
                    errors.push(String::new());
                }
                Err(e) => {
                    statuses.push(0);
                    errors.push(e.to_string());
                }
            }
        }
        drop(client);
        let mut counts = Vec::new();
        for server in running {
            counts.push(server.close().await);
        }
        let (accepted, max_open) = counted(&counts)?;
        Ok(Outcome {
            statuses,
            errors,
            accepted,
            max_open,
            serialized: None,
            timeout: None,
            within_limit: None,
        })
    }

    /// `handshakeCase`: an https GET to a server that accepts and never answers the
    /// `ClientHello`, under `deadline`; `limit` is the bound the error is expected within.
    async fn handshake(
        case: &Value,
        deadline: Duration,
        limit: Duration,
    ) -> Result<Outcome, BoxError> {
        let server = RawServer::start(|mut conn| async move { conn.drain().await }).await?;
        let port = server.port();
        let pki = TestPki::new()?;
        let client = Builder::new(settings(case)?, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .build()?;
        let req = request("GET", &format!("https://127.0.0.1:{port}/l"))?;
        let start = Instant::now();
        let (status, error, timeout, elapsed) =
            match client.send(req, Some(start + deadline), None).await {
                Ok(response) => (
                    i64::from(response.status().as_u16()),
                    String::new(),
                    false,
                    start.elapsed(),
                ),
                Err(e) => (0, e.to_string(), e.is_timeout(), start.elapsed()),
            };
        drop(client);
        let counts = server.close().await;
        let (accepted, max_open) = counted(&[counts])?;
        Ok(Outcome {
            statuses: vec![status],
            errors: vec![with_ports(&error, &[(port, "PORT")])],
            accepted,
            max_open,
            serialized: None,
            timeout: Some(timeout),
            within_limit: Some(elapsed >= limit && elapsed < 2 * limit),
        })
    }

    // Go: net/http/transport.go Transport.queueForDial and decConnsPerHost (MaxConnsPerHost).
    #[test]
    fn per_host_limit_waits() -> TestResult {
        run(async {
            let doc = golden("limits.json")?;
            for name in ["max_conns_per_host", "unlimited"] {
                let (case, go) = go_outcome(&doc, name)?;
                let rust = concurrent(&case, 3).await?;
                check_eq(&rust, &go, name)?;
            }
            Ok(())
        })
    }

    // Go: net/http/transport.go Transport.tryPutIdleConn (MaxIdleConns, MaxIdleConnsPerHost,
    // IdleConnTimeout).
    #[test]
    fn no_global_idle_cap() -> TestResult {
        run(async {
            let doc = golden("limits.json")?;
            let idle = Duration::from_millis(1500);
            let cases: [(&str, usize, &[usize], Duration); 4] = [
                ("max_idle_global", 2, &[0, 1, 0], Duration::ZERO),
                ("idle_timeout", 1, &[0, 0], idle),
                ("no_idle_timeout", 1, &[0, 0], idle),
                ("no_idle_per_host", 1, &[0, 0], Duration::ZERO),
            ];
            let mut differ = Vec::new();
            for (name, servers, order, pause) in cases {
                let (case, go) = go_outcome(&doc, name)?;
                let rust = sequence(&case, servers, order, pause).await?;
                if rust != go {
                    differ.push((name, rust.accepted, go.accepted));
                }
            }
            // reqwest caps idle connections per host only, so the first server's connection
            // stays idle beside the second's and the third request reuses it.
            check_eq(
                &differ,
                &vec![("max_idle_global", vec![1, 1], vec![2, 1])],
                "the cases whose connections differ (accepted: rust, go)",
            )
        })
    }

    // Go: net/http/transport.go persistConn.addTLS (TLSHandshakeTimeout) and
    // golang.org/x/net/context/ctxhttp Do.
    #[test]
    fn handshake_bounded_by_deadline() -> TestResult {
        run(async {
            let doc = golden("limits.json")?;
            // Go gives up after the 1 s TLS handshake timeout, an error bidder.go does not count
            // as a timeout; the clients have no such timeout, so the request's deadline ends the
            // handshake, as a timeout.
            let (case, go) = go_outcome(&doc, "tls_handshake_timeout")?;
            let second = Duration::from_secs(1);
            let rust = handshake(&case, 2 * second, 2 * second).await?;
            check_eq(
                &go.errors,
                &vec![
                    r#"Get "https://127.0.0.1:PORT/l": net/http: TLS handshake timeout"#.to_owned(),
                ],
                "Go's handshake timeout",
            )?;
            check_eq(
                &(go.timeout, go.within_limit),
                &(Some(false), Some(true)),
                "Go's error is not a timeout and came after the 1 s handshake timeout",
            )?;
            check_eq(
                &rust,
                &Outcome {
                    errors: vec!["context deadline exceeded".to_owned()],
                    timeout: Some(true),
                    ..go
                },
                "the clients' handshake ends at the 2 s deadline, not at 1 s",
            )?;

            // A deadline shorter than Go's handshake timeout ends the handshake in both.
            let (short_case, go_short) = go_outcome(&doc, "handshake_within_deadline")?;
            let limit = Duration::from_millis(300);
            let rust_short = handshake(&short_case, limit, limit).await?;
            check_eq(&rust_short, &go_short, "handshake_within_deadline")
        })
    }
}
